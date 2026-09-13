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

	"github.com/docker/docker/api/types/container"

	pb "aite/edge/gen/aitepb"
	"aite/edge/internal/config"
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

// ---- inspectStamps / newestStamp / isOrphanIdle --------------------------
//
// 这三个是 orphans 的判据本体（W3 ②）。orphans 自己要问 daemon，只能留在 `docker`
// tag 下；判定这半边抽出来之后就归这个门禁管了 —— 它决定 reaper 收不收一个上一次
// 进程留下的容器，判错的两个方向都很贵：收早了把别人正在用的容器删掉，收不动就把
// 容器永远留在机器上（B4 那条「reap 之后 docker ps -a 必须为空」就是它兜的）。

// inspectOf 造一个 inspect 结果。三个时间戳按 Created / StartedAt / FinishedAt 给。
func inspectOf(created, startedAt, finishedAt string) container.InspectResponse {
	return container.InspectResponse{
		ContainerJSONBase: &container.ContainerJSONBase{
			Created: created,
			State:   &container.State{StartedAt: startedAt, FinishedAt: finishedAt},
		},
	}
}

func TestInspectStampsTakesAllThreeInOrder(t *testing.T) {
	got := inspectStamps(inspectOf("c", "s", "f"))
	want := []string{"c", "s", "f"}
	if len(got) != len(want) {
		t.Fatalf("该取三个：%q", got)
	}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("顺序该是 Created/StartedAt/FinishedAt，实际 %q", got)
		}
	}
}

func TestInspectStampsOnMissingFieldsTakesWhatIsThere(t *testing.T) {
	// ContainerJSONBase 缺了：一个都取不到，且返回的是空切片不是 nil
	// （下游 range 都能扛 nil，但别让「空」和「nil」在断言里混着看）。
	empty := inspectStamps(container.InspectResponse{})
	if empty == nil || len(empty) != 0 {
		t.Fatalf("ContainerJSONBase 缺失时该是空切片：%#v", empty)
	}
	// State 缺了（容器刚建、inspect 里没 State）：只剩 Created 那一个。
	only := inspectStamps(container.InspectResponse{
		ContainerJSONBase: &container.ContainerJSONBase{Created: "2026-09-11T10:00:00Z"},
	})
	if len(only) != 1 || only[0] != "2026-09-11T10:00:00Z" {
		t.Fatalf("State 缺失时只该剩 Created：%q", only)
	}
}

func TestInspectStampsPassesSentinelAndGarbageThroughVerbatim(t *testing.T) {
	// inspectStamps 不解析，只搬运：零值哨兵与非法格式都要原样带出去，
	// 判死归 parseDockerTime。它要是自己动手过滤，newestStamp 就再也分不清
	// 「没有这个字段」和「这个字段是坏的」。
	got := inspectStamps(inspectOf("0001-01-01T00:00:00Z", "不是时间", ""))
	want := []string{"0001-01-01T00:00:00Z", "不是时间", ""}
	for i := range want {
		if got[i] != want[i] {
			t.Fatalf("第 %d 个该原样带出 %q，实际 %q", i, want[i], got[i])
		}
	}
}

func TestNewestStampPicksTheLatestNotTheFirst(t *testing.T) {
	// 顺序刻意反着给：Created 最新、FinishedAt 最旧。挑错方向这条就红。
	got, ok := newestStamp(inspectOf(
		"2026-09-11T12:00:00Z",
		"2026-09-11T10:00:00Z",
		"2026-09-11T08:00:00Z",
	))
	if !ok {
		t.Fatal("三个都是合法时间，该 ok")
	}
	if got.Hour() != 12 {
		t.Fatalf("该挑最新那个（12:00），实际 %v", got)
	}
}

func TestNewestStampSkipsTheUnparsableOnes(t *testing.T) {
	// 唯一一个能解析的排在最后，且前两个分别是零值哨兵和垃圾。
	got, ok := newestStamp(inspectOf("0001-01-01T00:00:00Z", "garbage", "2026-09-11T09:30:00Z"))
	if !ok {
		t.Fatal("有一个合法时间就该 ok")
	}
	if got.Hour() != 9 || got.Minute() != 30 {
		t.Fatalf("该落在那个合法时间上，实际 %v", got)
	}
}

func TestNewestStampSaysSoWhenNothingParses(t *testing.T) {
	for name, insp := range map[string]container.InspectResponse{
		"字段全缺": container.InspectResponse{},
		"全是哨兵": inspectOf("0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z", "0001-01-01T00:00:00Z"),
		"全是垃圾": inspectOf("x", "y", "z"),
		"全是空串": inspectOf("", "", ""),
	} {
		if _, ok := newestStamp(insp); ok {
			t.Fatalf("%s：一个都解析不出时该 ok=false", name)
		}
	}
}

func TestIsOrphanIdleCollectsAContainerWithNoUsableStamps(t *testing.T) {
	// 时间戳一个都读不出来 → 收。宁可多收一个，也别把它永远留在机器上。
	now := time.Date(2026, 9, 11, 12, 0, 0, 0, time.UTC)
	if !isOrphanIdle(container.InspectResponse{}, now, time.Hour) {
		t.Fatal("时间戳读不出来的容器该收")
	}
}

func TestIsOrphanIdleGoesByTheNewestStamp(t *testing.T) {
	now := time.Date(2026, 9, 11, 12, 0, 0, 0, time.UTC)
	idle := 30 * time.Minute

	// Created 是 4 小时前（早就超了），但 FinishedAt 是 1 分钟前 —— 刚活动过，不该收。
	// 按最旧的判就会把它误收掉。
	busy := inspectOf("2026-09-11T08:00:00Z", "2026-09-11T08:00:01Z", "2026-09-11T11:59:00Z")
	if isOrphanIdle(busy, now, idle) {
		t.Fatal("最新时间戳在 idle 之内的容器不该收")
	}

	// 三个都在 idle 之外 → 收。
	stale := inspectOf("2026-09-11T08:00:00Z", "2026-09-11T08:00:01Z", "2026-09-11T09:00:00Z")
	if !isOrphanIdle(stale, now, idle) {
		t.Fatal("最新时间戳也超过 idle 的容器该收")
	}
}

func TestIsOrphanIdleAtTheExactBoundaryIsInclusive(t *testing.T) {
	// 判据是 `>=`：正好等于 idle 就收。这条钉住那个等号 ——
	// 改成 `>` 它就红，而线上表现只是 reaper 慢一个 tick（60s），肉眼看不出来。
	now := time.Date(2026, 9, 11, 12, 0, 0, 0, time.UTC)
	idle := time.Hour

	exactly := inspectOf("2026-09-11T11:00:00Z", "", "")
	if !isOrphanIdle(exactly, now, idle) {
		t.Fatal("空闲时长正好等于 idle 时该收（判据是 >=）")
	}

	oneNanoShort := inspectOf("2026-09-11T11:00:00.000000001Z", "", "")
	if isOrphanIdle(oneNanoShort, now, idle) {
		t.Fatal("差 1ns 没到 idle 就不该收")
	}
}

// ---- Release("") --------------------------------------------------------

// W3 ①：契约（edge 的 .proto 里 `rpc Release` 那一行）标着「幂等」，而空 id 从前会
// 走到 `ContainerRemove(ctx, "", …)`，拿回的不是 IsErrNotFound，于是报成
// SandboxInternal —— 契约、包头注释、函数注释三处都说幂等，只有实现不是。
//
// **这条能待在无 daemon 门禁里，靠的正是那个早返回在 `dockerClient()` 之前**：
// 把早返回删掉，这条测试就会一路走到 daemon —— 无论有没有 daemon 都红
// （有 daemon：空引用报 SandboxInternal；没有：连不上 socket）。
func TestReleaseOnAnEmptyIdIsANoop(t *testing.T) {
	d, err := NewDocker(config.Sandbox{})
	if err != nil {
		t.Fatalf("装配不该失败：%v", err)
	}
	for _, id := range []string{"", " ", "\t\n"} {
		if err := d.Release(context.Background(), id); err != nil {
			t.Fatalf("Release(%q) 该当成已释放，实际 %v", id, err)
		}
	}
}
