// 沙箱这一包里**不需要 daemon** 的那几个函数的测试（RΩ 补，审核记账 R2）。
//
// `clip` / `diffFiles` / `parseDockerTime` / `reapVictims` 都是纯函数，却只在
// `docker` build tag 下被跑到 —— 而 CI 上没有 daemon，整份 `docker_test.go` 压根
// 不编译。于是这几个函数在门禁里等于裸奔：改错了也没人红。
//
// 本文件**不打 tag**，`go test ./...` 就会跑。它不碰 daemon、不起容器。
package sandbox

import (
	"context"
	"errors"
	"strings"
	"testing"
	"time"

	pb "aite/edge/gen/aitepb"
)

// ---- clip ---------------------------------------------------------------

func TestClipKeepsShortTextIntact(t *testing.T) {
	got, truncated := clip("短的", 100)
	if got != "短的" || truncated {
		t.Fatalf("没超限不该动：got=%q truncated=%v", got, truncated)
	}
}

func TestClipCutsByRuneNotByte(t *testing.T) {
	// 30 个汉字 = 90 字节。按字节切会把某个字切成半截（UTF-8 会出问号）。
	text := strings.Repeat("字", 30)
	got, truncated := clip(text, 20)
	if !truncated {
		t.Fatal("超限了却说没截断")
	}
	if !strings.HasSuffix(got, truncMarker) {
		t.Fatalf("截断后要以标记收尾：%q", got)
	}
	if got != strings.Repeat("字", 20-len([]rune(truncMarker)))+truncMarker {
		t.Fatalf("截断位置不对：%q", got)
	}
	if strings.ContainsRune(got, '�') {
		t.Fatalf("切在了 rune 中间：%q", got)
	}
}

func TestClipWithLimitSmallerThanTheMarker(t *testing.T) {
	// limit 比标记本身还短：keep 会算成负数，不许 panic
	got, truncated := clip("一二三四五", 1)
	if !truncated {
		t.Fatal("该截断")
	}
	if got != truncMarker {
		t.Fatalf("只剩标记：%q", got)
	}
}

// ---- diffFiles ----------------------------------------------------------

func TestDiffFilesReportsOnlyNewOrChanged(t *testing.T) {
	before := map[string]fileMeta{
		"/work/keep.txt":  {10, 100},
		"/work/touch.txt": {10, 100},
		"/work/gone.txt":  {10, 100},
	}
	after := map[string]fileMeta{
		"/work/keep.txt":  {10, 100}, // 一模一样 → 不报
		"/work/touch.txt": {10, 200}, // mtime 变了 → 报
		"/work/grow.txt":  {20, 300}, // 新增 → 报
	}
	got := diffFiles(before, after)
	want := []string{"/work/grow.txt", "/work/touch.txt"} // 按路径排序
	if len(got) != len(want) {
		t.Fatalf("条数不对：%v", entryPaths(got))
	}
	for i, e := range got {
		if e.Path != want[i] {
			t.Fatalf("第 %d 条不对：%v（期望 %v）", i, entryPaths(got), want)
		}
	}
	if got[1].Size != 10 {
		t.Fatalf("size 该带回来：%d", got[1].Size)
	}
	// 删掉的不出现（inventory §3「files_out 只报本次」）
	for _, e := range got {
		if e.Path == "/work/gone.txt" {
			t.Fatal("删掉的文件不该出现在 files_out 里")
		}
	}
}

func TestDiffFilesOnEmptyBeforeReportsEverything(t *testing.T) {
	got := diffFiles(map[string]fileMeta{}, map[string]fileMeta{"/work/a": {1, 2}})
	if len(got) != 1 || got[0].Path != "/work/a" {
		t.Fatalf("第一次 exec 该把 /work 下的都报出来：%v", entryPaths(got))
	}
}

func TestDiffFilesReturnsEmptySliceNotNil(t *testing.T) {
	// protojson 对 nil 和空切片的输出不同；契约里 files_out 是 repeated，空就是空数组
	got := diffFiles(map[string]fileMeta{}, map[string]fileMeta{})
	if got == nil {
		t.Fatal("空结果要是空切片，不是 nil")
	}
	if len(got) != 0 {
		t.Fatalf("不该有内容：%v", entryPaths(got))
	}
}

func entryPaths(entries []*pb.FileEntry) []string {
	out := make([]string, 0, len(entries))
	for _, e := range entries {
		out = append(out, e.Path)
	}
	return out
}

// ---- parseDockerTime ----------------------------------------------------

func TestParseDockerTimeAcceptsRFC3339Nano(t *testing.T) {
	got, ok := parseDockerTime("2026-09-11T10:20:30.123456789Z")
	if !ok {
		t.Fatal("合法时间该认")
	}
	if got.Year() != 2026 || got.Nanosecond() != 123456789 {
		t.Fatalf("解析结果不对：%v", got)
	}
	if got.Location() != time.UTC {
		t.Fatalf("该归一化到 UTC：%v", got.Location())
	}
}

func TestParseDockerTimeRejectsTheZeroSentinel(t *testing.T) {
	// Docker 用 0001-01-01 表示「没发生过」—— 当成「没有时间」而不是很久以前，
	// 否则 reaper 会把一个刚建好、还没启动过的容器当成超时孤儿收掉。
	for _, value := range []string{
		"0001-01-01T00:00:00Z",
		"0001-01-01T00:00:00.000000000Z",
		"",
		"   ",
		"不是时间",
	} {
		if _, ok := parseDockerTime(value); ok {
			t.Fatalf("%q 不该被当成有效时间", value)
		}
	}
}

// ---- reapVictims --------------------------------------------------------

func TestReapVictimsKeepsGoingAfterOneFailure(t *testing.T) {
	// 这条钉的就是审核记的那个坑：中间一个 Release 失败时，**已经真删掉**的 id
	// 不许随 error 一起丢掉 —— core 收不到就不会清 task→sandbox_id 的账。
	boom := errors.New("daemon 抽风")
	var tried []string
	released, err := reapVictims(context.Background(), []string{"a", "b", "c"},
		func(_ context.Context, id string) error {
			tried = append(tried, id)
			if id == "b" {
				return boom
			}
			return nil
		})
	if err != nil {
		t.Fatalf("有释放成功的就不该抛错：%v", err)
	}
	if strings.Join(tried, ",") != "a,b,c" {
		t.Fatalf("一个失败就不扫了：%v", tried)
	}
	if strings.Join(released, ",") != "a,c" {
		t.Fatalf("released 该是 a,c：%v", released)
	}
}

func TestReapVictimsSurfacesTheErrorWhenNothingGotReleased(t *testing.T) {
	// daemon 整个不可达：一个都没收掉，这时必须把错误抛上去，
	// 不然 EdgeStatus 之外就没有任何地方说得出「reap 根本没干成」。
	boom := errors.New("connection refused")
	released, err := reapVictims(context.Background(), []string{"a", "b"},
		func(_ context.Context, _ string) error { return boom })
	if !errors.Is(err, boom) {
		t.Fatalf("一个都没收掉该抛错：%v", err)
	}
	if len(released) != 0 {
		t.Fatalf("没收掉就不该报 released：%v", released)
	}
}

func TestReapVictimsDedupsIds(t *testing.T) {
	// victims = 本进程记账的 + 扫标签捡回来的孤儿，两边会重叠
	calls := 0
	released, err := reapVictims(context.Background(), []string{"a", "a", "b", "a"},
		func(_ context.Context, _ string) error { calls++; return nil })
	if err != nil {
		t.Fatalf("不该抛错：%v", err)
	}
	if calls != 2 {
		t.Fatalf("同一个 id 只该 Release 一次，实际 %d 次", calls)
	}
	if strings.Join(released, ",") != "a,b" {
		t.Fatalf("released 该去重：%v", released)
	}
}

func TestReapVictimsOnEmptyInput(t *testing.T) {
	released, err := reapVictims(context.Background(), nil,
		func(_ context.Context, _ string) error { t.Fatal("不该被调"); return nil })
	if err != nil || len(released) != 0 {
		t.Fatalf("空输入该干净返回：%v %v", released, err)
	}
}

// ---- short --------------------------------------------------------------

func TestShortTrimsLongIdsAndLeavesShortOnes(t *testing.T) {
	long := strings.Repeat("f", 64)
	if got := short(long); len(got) != 12 {
		t.Fatalf("长 id 该截到 12 位：%q", got)
	}
	if got := short("abc"); got != "abc" {
		t.Fatalf("短 id 原样返回：%q", got)
	}
}
