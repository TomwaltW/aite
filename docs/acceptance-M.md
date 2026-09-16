# 人工验收 M1–M6 剧本（真实飞书测试群）

对应 `docs/dev-spec-2026-09-09.md` §2.4。那张表只写了「操作 → 期望」，本文把它变成
照着做就能跑的东西：每条 M 拆成**操作 / 在哪看 / 期望 / 不对时查哪**四段。

这是 P0 的最后一关，跟自动化测试最大的不同是：**出了问题没有断言告诉你哪一行红了**，
只有群里一条没回的消息、一张卡在 working 的卡片。所以每条 M 都配了「在哪看」——
把「看起来不对」变成「哪个环节不对」，靠的是日志、证据链、`docker ps` 这几个窗口。

> **2026-09-12（RΩ）：命令已换成 Rust + Go 两进程的口径。** 原文写的是 Python 单进程
> （`python -m aite.app` / `scripts/preflight.py` / `scripts/evidence_show.py`），那棵树已经删了。
> 判据、期望、排障表基本没动 —— 换的主要是敲什么命令。**唯一改了判据的是 §0.4 与 §8 第 4 条**：
> 卡片上的按钮从 RΩ 起一个都不渲染（Go SDK 在长连接上丢弃非 event 帧，点了不会有反应），
> 「停止」改走 `!stop`、「证据」改走 `aite evidence show`。
>
> **本轮多出来的一件事：现在要起两个进程。** edge 拿着飞书长连接和 Docker，
> core 拿着路由、会话、worker。M6 的重启因此要分三种情形各验一遍（见 §6）。
>
> **2026-09-12（V3）：本文第一次被人照着从头核过一遍。** RΩ 那一轮是「换敲什么命令」的
> 批量替换，没人真跑。V3 逐条跑了一遍，改掉的是**照着敲会卡住的地方**：
>
> - **§0.2 起飞段重写** —— 原来教人先 `cd edge` 再 `go run ./cmd/aite-edge`，
>   照那样敲**两个进程永远连不上**，而且两边日志都不报错。
>   现在两条命令都在仓库根跑，理由与判据都写在里面。
> - **§0.1 / §0.3 / §7** —— preflight 的期望换成真输出；观察窗从三个变四个
>   （`feishu.*` 全在 **edge** 那个终端，原文让你盯着 core 那个窗等两行永远不出现的日志）；
>   §7 那张表加了「哪个进程」一列，并改掉三个**代码里根本不存在**的日志名。
> - **「纯文本回复（不是卡片）」这句在协议层是假的** —— core 发的每一条文本到飞书都是
>   `msg_type=interactive`。真的那一半是「没有 checklist 进度卡片」。见 §0.5。
> - **§3.7 的两条待核实变成了两个编号步骤**：(b) 排在 M1 之前跑（§0.1），(a) 就是 M4。
>
> 本轮**没改任何代码**。走查里撞见的代码侧问题按轨转出去了，文中带 `<!-- 台账 -->`
> 注释的那几行就是记账，对应的轨修掉后连注释一起删。
>
> **2026-09-12（W1）：V1–V6 六轨合进 main 之后，本文被合并造出的失真校过一遍。**
> V3 那一轮写的时候 V1 和 V5 还没并进来，所以有几段「各自都对、合起来自相矛盾」。改的是：
>
> - **§0.4 / §M1 排障表 / §7 —— `!status` 与 `!stop` 拆成两半。** V5 只修了 `!status`
>   那一半（控制面的 `running` 并进 `status_tasks`），`!stop` 和卡片 stop 按钮走的
>   `resolve_stop_target` / `resolve_task` **原地没动**。当时这两条命令对「什么算活跃」
>   意见不一致，比改之前更费解 —— 原来「已知记账，别去查沙箱」那一行按半边重写了。
>   **（这条已被 W2 收口，见下。）**
>
> **2026-09-13（AA2）：`!restart` 跟上了那两条，最后一处分家的口径收了。**
> `cmd_restart` 归档会话时用的还是 `list_active_tasks`，交付中（`Answering`）的任务
> 整个看不见 —— 而它那段注释点名要防的两件事（结果落进已归档的会话、继续出现在
> `!status` 里），一个正在交付的任务恰好两条都中。现在它查的是同一份 `status_tasks`，
> 分流共用同一条规则（`StopTarget::of_existing`）：能停的照停，**停不掉的不碰，
> 改成在回帖里点名**（新文案见下表）。为什么不是「换个列表、照停不误」：实测过两头，
> 那只会造出「终止了 1 个进行中的任务」这句假话 —— 答复照发、库里的 `Cancelled`
> 随后被 worker 的 `finish()` 盖回 `Delivered`。改的是 §0.4 本节这段与下面那张表
> （从三行变四行）、§M1 排障表（多一行）、§7 末那一节（「两条使用口径」→「三条」）；
> `plane.rs` 的行号引用跟着全文重核了一遍（本轨在它上游加了 27 行）。
>
> **2026-09-12（W2）：`!stop` 跟上了 `!status`，上面那条记账销了。**
> `resolve_stop_target` / `resolve_task` 改走同一个 `status_tasks`，
> 并把「交付中（`Answering`）停不了」写成**明面上的约定**：找得到、回一句
> 「任务 #A17 正在把答复发给你，停不了了。」，不再回「没有这个任务」。
> 改的是 §0.4 那段引用框、§M1 排障表那两行、§7 末「两条使用口径」，外加 §M3 排障表
> 和 §8 第 4 条里同一句话的两处复述；`plane.rs` / `app.rs` 的行号引用跟着重核了一遍。
> - **§0.1 的 `--help`、§0.2.4 的 `--traceback` / `--grace`** —— V6 ④a/④b/④c 的实际结果，
>   四个 `--help` 写法逐个实测。
>   **（2026-09-13：§0.1 那张表已被 W3 ③ 取代 —— 不带 `--` 的写法也修好了，
>   `main.rs` 的四个 `trailing_var_arg` variant 各加了 `disable_help_flag`。
>   八个写法全是 stdout + 退出码 0，带不带 `--` 逐字节相同。那张表现在记的是 W3 的结果。）**
> - **§0.2.5 新增：compose 起飞。** 原来是「占位，等 V1」，V1 已合入，本轮真起了一遍核的。
>   **其中一条结论变了**：仓库不再 `./:/app` bind mount。
> - **行号引用逐个当场核过**。合并后漂了一大片（V2 给 `preflight.rs` 加了 849 行、
>   V5 给 `plane.rs` 加了 144 行），凡写成 `文件:行号` 的都重新 `sed -n` 看过。
>
> **`<!-- 台账 -->` 的约定改了一条**：V5 那种「只修一半」的情形下，注释不删，改成
> **只指仍未修的那一半**并写明归哪轨。整段删掉会把 V5 修对的那一半一起删了。
>
> **2026-09-13（AA1）：两个容器降权到非 root，只动 §0.2.5。**
> compose 起飞多了三个环境变量与一个 `mkdir -p data`（Linux 上必须做，macOS 上不用），
> W1 那张四行实测表加了第五行「宿主机写得进 `./data` 吗」。
> **这一条修掉的是 §0.2 那条路上的一个真坑**：在这之前，Linux 上 compose 跑过一次
> 之后宿主机直跑 `aite run` 会因为落盘目录归 root 而起不来
> （实测 `preflight --offline` 第 `[7/7]` 组 `FAIL`、退出码 1）。
> §0.1 / §0.3 与 M1–M6 的判据一个字没动。

---

## 0. 通用前置

### 0.1 起飞前 60 秒：外部依赖自检

```bash
core/target/debug/aite preflight                 # 七组检查，全 OK → 退出码 0
core/target/debug/aite preflight --offline       # 只跑不需要网络/docker 的 1、2、7
core/target/debug/aite preflight --json          # 机器可读
```

四个参数：`--config PATH`（默认 `config/aite.yaml`，不存在则退到 `config/aite.example.yaml`）、
`--offline`、`--json`、`--chat-id ID`。

> ℹ️ **`--help` 带不带那个 `--` 都行了（W3 ③ 修的）。** 从前不加 `--` 会被 clap 截胡，
> 打出来的是一句不含任何真实选项的 `[ARGS]...`，**而退出码还是 0** —— 坏掉的帮助和好的
> 帮助在脚本里长得一模一样。四个写法实测如下：
>
> | 写法 | 打出什么 | 走哪 | 退出码 |
> |---|---|---|---|
> | `aite preflight --help` | ✅ 手写用法，四个参数全在 | stdout | 0 |
> | `aite preflight -- --help` | ✅ 同上，与上一行逐字节相同 | stdout | 0 |
> | `aite run --help` | ✅ 手写用法，三个参数全在 | stdout | 0 |
> | `aite run -- --help` | ✅ 同上，与上一行逐字节相同 | stdout | 0 |
>
> 改的是 `core/crates/app/src/main.rs`：四个转发型子命令（run / evals / evidence /
> preflight）各加一个 `#[command(disable_help_flag = true)]`，把 `--help` / `-h` 从 clap
> 手里还给各自的手写解析器。`-h` 与 `--help` 一视同仁；`aite evals --help` /
> `aite evidence --help` 从前同病，一并修了；`aite contracts` 的参数是真 clap 子命令，
> 本来就没这个病、也没被波及。
>
> **带 `--` 的那两个写法逐字节没变** —— 那是 V6 ④b 修好的那半，W3 拿改动前后的
> stdout / stderr / 退出码逐个 `cmp` 过（顺带核了顶层 `aite --help` 与 `aite contracts`
> 的四个写法、四条业务路径，都没变）。回归在 `core/crates/app/tests/cli_smoke.rs`：
> 八个写法各一条、`-h` 四条，外加一条「带不带 `--` 打的必须是同一份」防「只修一半」。

`--offline` 的真输出长这样（实测原样；`config/aite.yaml` 由 `config/aite.example.yaml`
复制而来，环境里只设了 `AITE_MODEL_API_KEY`。唯一的改动是那两行 NOTE 的正文很长，
在这里省成一句，原文逐字抄在 §0.1.1）：

```
Aite 起飞前自检
  配置：config/aite.yaml
  模式：--offline（只跑 1/2/7）
  时间：2026-09-12T16:32:01+08:00

[1/7] OK   配置可加载       platform=feishu · model.provider=openai_compat · sandbox.image=aite-sandbox:p0
[2/7] WARN 环境变量齐       3/4 个未设置：FEISHU_APP_ID FEISHU_APP_SECRET FEISHU_BOT_OPEN_ID（已设置：AITE_MODEL_API_KEY） —— --offline 下不作判据
           └ 怎么补：真机起飞前去掉 --offline 重跑一次，这几项必须是 OK
[3/7] SKIP 飞书凭证有效     --offline：不碰网络
[4/7] SKIP 飞书身份对得上   --offline：不碰网络
       NOTE §3.7 待核实      §3.7(a) …（见 0.1.1）
       NOTE §3.7 待核实      §3.7(b) …（见 0.1.1）
[5/7] SKIP 模型端点通       --offline：不碰网络
[6/7] SKIP 沙箱可用         --offline：不碰 docker
[7/7] OK   落盘目录可写     3 个路径都落得下去（data data/evidence data/artifacts 待建，起飞时自动 mkdir）：data/aite.db · data/evidence · data/artifacts

------------------------------------------------------------------------
汇总：OK 2 · WARN 1 · FAIL 0 · SKIP 4（共 7 项，过了 7 项）
全部没红，可以起飞。（--offline 只验了 1/2/7，真机起飞前请全跑一遍）
```

七组分别是：① 配置可加载（问的是「这份配置**起得来**吗」：yaml 解析得出来、
`worker.system_prompt_path` 指得到、`platform` / `model.provider` 不需要注入、
`storage.sqlite_path` 上已经有的那个文件真能当库打开）
② 环境变量齐 ③ 飞书凭证有效 ④ 飞书身份对得上
⑤ 模型端点通 ⑥ 沙箱可用 ⑦ 落盘目录可写。**任一 FAIL 就别往下走** —— M1–M6 里
八成的「没反应」都是这七项里的某一项没配好，在这里花 60 秒比在群里瞎试便宜得多。
（`platform: fake` 那一档下 ②③④ 是 SKIP —— fake 平台不连飞书，那三组的判据不适用。
真机验收用的是 `platform: feishu`，碰不到这一档。）

第 ④ 组尤其要过：它拿 token 查机器人自身信息、和 `FEISHU_BOT_OPEN_ID` 的取值比对。
**配错了应用时 M1 会完全静默**（收得到事件但认不出 @ 的是自己），没有任何报错。

> ⚠️ **`--offline` 全绿 ≠ core 起得来。** 实测：拿 `config/aite.example.yaml` 原样跑
> `--offline`，上面那份输出是 `FAIL 0`，但 `aite run` 会退出码 2 ——
> `aite 起不来：模型配置不完整：ModelConfig.base_url 是空的：填百炼 / 智谱的 OpenAI 兼容端点`。
> 管 `base_url` / `model` 有没有填的是**第 5 组**，而 `--offline` 跳过它。
> 所以真机起飞前那一遍**必须不带 `--offline`**。
>
> ⚠️ **同族的另一个口子（2026-09-12 当场咬过人，现已补上）**：那天 `--offline` 报
> 「全部没红，可以起飞」，`aite run` 紧接着退出码 2 —— 配置里的 `worker.system_prompt_path`
> 还指着当天删掉的 Python 树（`aite/worker/prompts/`），而**七组里当时没有一组碰这个字段**，
> 去掉 `--offline` 全跑一遍也照样是 OK（实测）。现在它归**第 1 组**：yaml 解析得出来、
> 且 `worker.system_prompt_path` 指到的文件真的在，读不到就是 FAIL，「怎么补」直接给出
> 该改成的路径和这条病史。这一项不碰网络也不碰 docker，**`--offline` 下照样跑**。
>
> ⚠️ **第三个同族口子（2026-09-13 当场咬过人，现已补上）**：`platform: fake` 的配置
> `--offline` 报「全部没红，可以起飞」退出 0，`aite run` 退出码 2
> （`config.platform=fake 时必须由调用方注入平台实现`）。`model.provider: scripted` 同病。
> 这两个取值**只能被注入着用**，而 `aite run` 不注入任何实现 —— 七组当时一样没有一组
> 问「这个取值自己起得来吗」。现在也归**第 1 组**，与上一条一次报齐，`--offline` 下照样跑。
> 同一份 fake 配置从前还在要飞书凭证（②③④ 全红），而 fake 平台压根不连飞书 ——
> 那一档下现在 ②③④ 一并 SKIP。
>
> ⚠️ **第四个同族口子（2026-09-13 当场咬过人，现已补上）**：`storage.sqlite_path` 指着一个
> **已经存在、但内容不是 SQLite** 的文件（备份文件、半截下载、被别的东西占了名字）。
> `--offline` 与全跑**都全绿**，而 `aite run` 退出码 2 ——
> `aite 起不来：建表失败（…）：sqlite: file is not a database`。
> **第 ⑦ 组接不住它**：那一组问的是三个路径的最近已存在祖先**目录**写不写得进去，
> 不是这个**文件**是不是个库 —— 排障时别拿「⑦ 是绿的」当反证，改后这两件事仍然分开报。
> 现在归**第 1 组**，与上两条一次报齐，`--offline` 下照样跑。判据是**纯读**
> （开一次库 + 逼它读一次文件头，读完就关，一个字节不写）；**文件不在不算问题**，
> 那是正常路径 —— 起飞时自己建一个空库。
>
> > **探不出来的那一半，先说在这儿**：文件是个好库、但它自己只读（或所在卷只读）时
> > `aite run` 的建表照样会炸，而第 1 组只读、读得动就算过，第 ⑦ 组问的又是目录 ——
> > 两组都看不见。真机上遇到「preflight 全绿而 `aite run` 报建表失败」，先查这个。

#### 0.1.1 第 0 步：先把 §3.7(b) 的结论拿到手（排在 M1 之前，60 秒）

规范 §3.7 留了两个问号，preflight 会在第 4 组之后各打一行 NOTE。**(b) 这条现在就能出结论**，
而它决定 **M5 演不演**、以及 `docs/demo-3min.md` §5 那段加演进不进 3 分钟版 ——
在 M5 才发现权限没批，比在这里花 60 秒贵得多。

```bash
# 不带 --chat-id：只会告诉你「问不出来」
core/target/debug/aite preflight --offline
#   NOTE §3.7 待核实   §3.7(b) 群历史是否要「获取群组中所有消息」敏感权限 —— 未核实。
#   飞书没有「列出本应用已授权范围」的免权限接口，光靠凭证问不出来；
#   带 --chat-id <测试群 chat_id> 重跑，我就直接调一次群历史给你结论（M5 靠它）。

# 带上测试群的 chat_id（**不能带 --offline**，它要真调一次飞书）：
core/target/debug/aite preflight --chat-id <测试群 chat_id>
```

带 `--chat-id` 时 preflight 会**真调一次群历史**（`GET /open-apis/im/v1/messages`，
`container_id_type=chat&page_size=1`，只取 1 页 1 条）。两种结局的原文写死在代码里
（`core/crates/app/src/preflight.rs:801-854` 的 `probe_history_scope`），逐字抄在这儿，你看到的就是它：

- **读得到** →
  > `§3.7(b) 实测：这套凭证**读得到**群历史（chat 返回 N 条，只取了 1 页 1 条）。也就是说当前已授予的权限足够 M5；至于是不是「获取群组中所有消息」那条在起作用，接口不回权限来源，问不出来 —— 但对起飞而言结论已经够用。`

  → **M5 照跑**，`demo-3min.md` §5 那段加演可选。

- **读不到** →
  > `§3.7(b) 实测：这套凭证读**不到**群历史（code=… msg=…）。去开放平台补权限（读取群历史消息；若提示需要「获取群组中所有消息」则它就是必需的敏感权限，要走审核），补完重跑本项。M5「汇总本群本周开放事项」在此之前一定过不了。`

  → **M5 先别做**，去开放平台补权限；补完重跑本项再说。`demo-3min.md` §5 那段加演砍掉
  （`demo-3min.md` §5 的「翻车怎么救」里已有这条应对）。

§3.7(a)（话题里不带 @ 的回复会不会投递）**起飞前查不了**，它就是 M4 那个实验 ——
preflight 的那行 NOTE 自己也这么说：

> `§3.7(a) 只有 @ 权限时话题内不带 @ 的回复是否投递 —— 未核实，且起飞前查不了：要真在话题里发一条不带 @ 的消息、看事件有没有投递才知道，M4 就是那个实验。当前 FEISHU_P0.supports_passive_listen=False（保守取值），M4 请带 @ 先走通。`

### 0.2 起飞（**两个进程，都在仓库根跑**）

> **2026-09-12（V3）改：两条命令都必须在仓库根敲。** 原文写的是先 `cd edge`、
> 再 `go run ./cmd/aite-edge --config ../config/aite.yaml`，照那样敲
> **两个进程永远连不上**，而且两边日志都不报错、core 还会照常起飞 ——
> 群里每一条消息石沉大海。复现与判据见 0.2.3。

#### 0.2.1 先编 edge 的二进制

```bash
cd edge && go build -o bin/aite-edge ./cmd/aite-edge && cd ..
```

> ⚠️ **`make build` 不产出这个二进制。** `Makefile:24-26` 的 build 是
> `cd edge && go build ./...` —— Go 在**包列表多于一个**时只做编译检查、**丢弃产物**。
> 实测：主仓 `make build` 跑过无数遍，`edge/bin/` 至今不存在（`ls edge/bin` →
> `No such file or directory`），而 `Makefile:88` 的 clean 里还写着 `rm -rf edge/bin`。
> 要二进制就得自己给 `-o`。（镜像里的正确写法在 `docker/edge/Dockerfile:21`：
> `cd edge && go build -o /out/aite-edge ./cmd/aite-edge`。V1 改成真镜像之后
> compose 不再在容器里现编，所以那条写法已经不在 `docker-compose.yml` 里了。）
> `edge/bin/` 在 `.gitignore:21`，不入库。

#### 0.2.2 开两个终端，**两个的当前目录都是仓库根**

```bash
# 终端 A —— edge（Go）：飞书长连接 + Docker 沙箱
edge/bin/aite-edge --config config/aite.yaml

# 终端 B —— core（Rust）：路由 / 会话 / worker / 证据
core/target/debug/aite run --config config/aite.yaml
```

不想编二进制，`go run` 那条路也留着 —— 但**同样在仓库根跑**，注意 `./edge/...` 这个前缀：

```bash
go run ./edge/cmd/aite-edge --config config/aite.yaml
```

**为什么必须在仓库根**（不写理由，下一个人还会 `cd edge`）：config 里的相对路径按契约
**全部相对仓库根** —— `edge.edge_socket` / `edge.core_socket`、`worker.system_prompt_path`、
`storage.*`。`edge/cmd/aite-edge/main.go:72` 那个 flag 的帮助文本自己就写着「相对仓库根」。
坑在两边**解析方式不一样**：

- **core** 拿 `std::env::current_dir()` 当 repo_root（`core/crates/app/src/app.rs:175-179`），
  再把 socket 解析成**绝对路径**（`core/crates/edge-client/src/lib.rs:109-116` 的 `resolve()`）；
- **edge** 把配置里的裸相对路径**原样**交给 `ListenUnix`（`main.go:102`），而 `ListenUnix`
  第一件事是 `os.MkdirAll(filepath.Dir(path), 0o755)`（`edge/internal/server/server.go:21-24`）
  —— **不管 cwd 在哪都先把目录静默建出来，一声不吭**。

所以在 `edge/` 里起 edge，socket 落到 `edge/data/run/`；core 去连 `<仓库根>/data/run/` ——
各拿各的 socket。**这个坑 RΩ 在 compose 上已经踩过并修好了**，`docker-compose.yml` 抬头
注释第 2 条把教训写死在文件里；compose 修了，人手起飞的 runbook 当时没跟上，本轨补。

**谁先起都行**（§2.1 启动顺序无关）：两边都是懒连接 + 1→2→…→30s 退避重连。
core 起飞时会问一次 edge 的 `GetStatus`（最多 5 次、每次间隔 1s，
`core/crates/app/src/app.rs:43` 的 `EDGE_STATUS_ATTEMPTS = 5`）：

- 答得上且 `contract_version` 一致 → 日志 `aite.edge_status`，接着起飞；
- 答得上但版本不一致 → **拒绝起飞**，两边版本都印出来（两个进程要一起升）；
- 五秒内答不上（edge 还没起）→ 记一行 `aite.edge_unreachable` **照常起飞**，
  edge 起来后自动恢复。

#### 0.2.3 起对了长什么样 / 起错了长什么样

**起对了** —— 下面两段是**本机实测**原样粘的，跑的时候**没有飞书凭证**
（`FEISHU_*` 三项都没设、model 填的是占位值），所以有两栏跟真机不一样，
已在各自那一行标出来。core 这边（路径是绝对的）：

```
INFO aite_edge_client::link: edge.connecting socket=<仓库根>/data/run/aite-edge.sock
INFO aite_edge_client::link: edge.connected socket=<仓库根>/data/run/aite-edge.sock
INFO aite.app: aite.edge_status edge_version=0.0.1 contract_version=p0.2 edge_platform=feishu platform_connected=false sandbox_ok=true
                                                           ↑ 真机上这里必须是 platform_connected=true
INFO aite.app: aite.up platform=feishu model=demo-placeholder sandbox=aite-sandbox:p0 sqlite=data/aite.db evidence=data/evidence edge=<仓库根>/data/run/aite-edge.sock
                                       ↑ 真机上是你 config 里那个真模型名
INFO aite_edge_client::ingress: ingress.listening socket=<仓库根>/data/run/aite-core.sock
```

edge 这边（实测，原样）：

```
level=INFO msg=edge.takeoff version=0.0.1 contract_version=p0.2 platform=feishu edge_socket=data/run/aite-edge.sock core_socket=data/run/aite-core.sock image=aite-sandbox:p0
level=INFO msg=edge.grpc_listening socket=data/run/aite-edge.sock contract_version=p0.2
level=INFO msg=edge.sandbox_ok
```

两件要当场核的事：

1. **`aite.up` 那一行「接了谁」先抄下来** —— platform / model / sandbox 镜像 / sqlite 路径 /
   evidence 目录 / edge socket，六栏。后面每条 M 的排障都从它开始，尤其是 evidence 目录，
   `aite evidence show` 要用。
2. **`aite.edge_status` 里的 `platform_connected` 必须是 `true`**。它是 edge 那边飞书长连接
   的状态；`false` = 长连接还没建上（凭证不对 / 网络不通），而 core **照样起飞、
   看起来一切正常**。上面那份实测就是 `false` 的样子 —— 同一时刻 edge 那个终端在刷
   `feishu.connect_failed` + `feishu.reconnecting`。
   **这一栏是 false 就别往 M1 走**，先回 §0.1 把不带 `--offline` 的 preflight 跑绿。

**起错了**（实测：edge 在 `edge/` 里起、core 在仓库根起）—— core 这边：

```
INFO  aite_edge_client::link: edge.connecting socket=<仓库根>/data/run/aite-edge.sock
WARN  aite_edge_client::link: edge.reconnecting socket=<仓库根>/data/run/aite-edge.sock error=transport error retry_in_sec=1
WARN  aite.app: aite.edge_unreachable contract_version 这一轮没比成；edge 起来后下一发 RPC 自动恢复 socket=<仓库根>/data/run/aite-edge.sock attempts=5 error=[grpc_unavailable] No such file or directory (os error 2)
```

而 edge 那边**一切正常**，日志里那行还写着 `edge_socket=data/run/aite-edge.sock`：

```
level=INFO msg=edge.grpc_listening socket=data/run/aite-edge.sock contract_version=p0.2
```

⚠️ **两边日志放一起看不出问题** —— core 打的是绝对路径、edge 打的是裸相对路径，
而后者正好是前者的尾巴，肉眼会以为是同一个文件。**唯一便宜可靠的判据是看目录**：

```bash
ls edge/data          # 期望：No such file or directory
ls data/run           # 期望：aite-edge.sock（起了 core 之后还有 aite-core.sock）
```

> `git status` **不是判据**，别用。`edge/data/` 确实没被 `.gitignore` 挡着
> （`.gitignore` 只有根锚定的 `/data/`(:11) 和 `/edge/bin/`(:21)），但里面只有一个 unix
> socket 时 git 什么都不显示 —— git 不跟踪 socket，只有 socket 的目录在 git 眼里是空目录。
> 实测：`git status --short --untracked-files=all` 是空的，往里丢一个普通文件才冒出 `?? edge/data/`。

`aite.edge_unreachable` 之后 core 会**照常起飞**（§2.1 启动顺序无关），所以它不报错、不退出 ——
它会看起来一切正常，然后每一条群消息都石沉大海。这正是最难查的那一种。

#### 0.2.4 停机、参数、compose

- 停：Ctrl-C（SIGINT/SIGTERM 走同一条优雅退出路径）。**再按一次是硬退**（退出码 130）。
  实测 core 侧优雅退出打这四行：
  ```
  WARN aite.app: aite.signal 收到，开始优雅退出（再来一次立即硬退） signal="SIGTERM"
  INFO aite.app: aite.stopping grace=20.0 pending=0
  INFO aite_edge_client::ingress: ingress.stopped socket=<仓库根>/data/run/aite-core.sock
  INFO aite.app: aite.down
  ```
  edge 侧打这三行（**`edge.counters` 是 edge 侧计数器唯一的出口**，见 §7 末）：
  ```
  level=INFO msg=edge.signal signal=SIGTERM note=开始优雅退出
  level=INFO msg=edge.counters events.sent=0 ingress.invalid=0 ingress.errors=0 ingress.reconnects=0
  level=INFO msg=edge.down
  ```
- `--grace SEC`：优雅退出的宽限期，**0–86400 秒**，默认 20（对齐 compose 的
  `stop_grace_period`）。出界的值在门口就被挡：实测 `--grace 100000` 回一行
  `--grace 要是 0 到 86400 之间的有限秒数，收到 "100000"`、退出码 2。
- `--traceback`：**别指望它给你错误链**，它只是把同一句话用 Debug 再包一层引号
  （`eprintln!("{e:?}")`，`core/crates/app/src/cli.rs:161-163`）。`StartupError` 是
  `#[error("{0}")]` 的 newtype、没有 `source()`，所以那一行就是把人话套进 `StartupError("…")`。
  起不来时真正有用的是它默认就打的那两行：一句人话 + 「用的配置是 …」。
  （`--help` 里原先承诺的「完整错误链」名不副实，V6 ④c 已改成说实话 ——
  现在写的是「起不来时在人话后面多打一行错误值的 Debug 形」。）
- **compose 起**：见下面 §0.2.5，它是一套独立的形态，判据与手起飞不一样。

#### 0.2.5 compose 起飞（V1 起是真镜像，不再挂仓库现编）

```bash
make compose-up        # = compose-build（建三个镜像）+ docker compose up -d + compose-ps
make compose-ps        # 看谁就绪，STATUS 一栏带 healthy / Restarting
make compose-logs      # = docker compose logs -f --tail=200 core edge
make compose-down      # 停（要连命名卷一起收自己加 -v，先确认没别人在用 project aite）
```

**先建镜像这一步不能跳。** V1 把 compose 从「把仓库挂进去、在容器里现编」换成了两个
多阶段真镜像（`docker/core/Dockerfile`、`docker/edge/Dockerfile`，运行层是
debian:trixie-slim + 一个二进制）—— 镜像不在就起不来。`make compose-build` 走的是
`docker compose --profile images build`，**连 `aite-sandbox:p0` 一起建**（它在 `images`
profile 里，`docker compose up` 和 `config --services` 都看不见它）。

**没有飞书凭证的机器要给 edge 换一份配置。** 两个 service 默认吃同一份 `config/aite.yaml`
（真机验收就是这个形态），但 `platform` 这个字段对两边含义相反：core 配 `fake` 直接拒绝起飞，
edge 配 `feishu` + 假凭证则 `platform.Start` 返错 → 进程退出 → 撞上 `restart: unless-stopped`
就是崩溃循环。所以本机冒烟要另给 edge 一份 `platform: fake` 的配置：

```bash
sed -e 's#^platform: feishu.*#platform: fake#' config/aite.yaml > config/aite.ci-edge.yaml
AITE_EDGE_CONFIG=config/aite.ci-edge.yaml docker compose up -d
```

> `.gitignore` 只挡了 `config/aite.yaml` 一个文件（`:14`），**`aite.ci-edge.yaml` 会冒在
> `git status` 里** —— 它是冒烟用的临时件，用完删掉。CI 那边同样是现造两份
> （`.github/workflows/ci.yml:97-104`），checkout 里本来就没有配置。

**两个容器以非 root 跑（2026-09-13 起）。Linux 上起飞前要多做两件事：**

```bash
mkdir -p data                                              # ① 先建出来，别让 dockerd 建
AITE_UID=$(id -u) AITE_GID=$(id -g) \
AITE_DOCKER_GID=$(stat -c '%g' /var/run/docker.sock) \
AITE_EDGE_CONFIG=config/aite.ci-edge.yaml docker compose up -d
```

- **① `mkdir -p data`**：`data/` 不入库，不先建的话 bind mount 时由 dockerd 建成
  `root:root 0755`，非 root 的容器写不进去 —— 实测症状是
  `aite 起不来：建不出目录 data/evidence：Permission denied`，配 `restart: unless-stopped`
  就是崩溃循环。先建出来它就归当前用户，与 `user:` 的取值对得上。
- **② `AITE_UID` / `AITE_GID`**：镜像里的默认身份是 `1001:1001`（GitHub runner 的取值），
  但 `./data` 是 bind mount、宿主机那边归当前用户，uid 每台机器都不一样，只能传进去。
  **两个 service 必须解析出同一个 uid** —— 它们互相 `connect` 对方的 unix socket，而
  `connect` 要 socket 文件的**写权限**，socket 是 `srwxr-xr-x`、只有属主有写位。
  只降一个的形态**看着是健康的**（两边 healthcheck 都转 `healthy`）但实际是聋的：
  实测 `edge.connected` / `aite.edge_status` 两行日志消失，core 打
  `aite.edge_unreachable … Permission denied (os error 13)`。
- **② `AITE_DOCKER_GID`**：只有 edge 用，它要读 `/var/run/docker.sock`。这个 gid 每台
  宿主机不一样（Docker Desktop 上是 0，Linux 上一般是 `docker` 组），**默认值 0 在 Linux
  上是错的**。传错不是静默失败：`aite preflight` 第 6 组 FAIL 并点名沙箱不可用。
- **macOS 上这三条都不用管**，也**验不出来**：Docker Desktop 的 bind mount 过 VirtioFS，
  会双向翻译 uid（本机实测：容器里看见的是它自己的 uid、宿主机看见的是当前用户），
  换什么身份跑都写得进。判据在 CI（Linux runner）那一侧。
- 换了 `AITE_UID` 之后要 `docker compose down -v`：`run:` 那个命名卷是 `1777`（sticky），
  上一个 uid 留下的残留 socket 新 uid 删不掉。会响，不是静默。

> `make compose-up` **不传这三个变量**。macOS 上没影响；Linux 上先 `export` 再 `make`。

**healthcheck 两边判据不一样**（`docker-compose.yml` 里两个 `healthcheck:` 小节；
**刻意不写行号** —— 2026-09-13 加 `user:` / `group_add:` 时这两个数就漂了一次）：

- **edge** 探标准 gRPC health（`grpc-health-probe`，探针二进制烤在镜像里），
  与三个业务服务同一个 socket；
- **core 没有** gRPC health service，判据是 `nc -U -z` **真 connect** ingress socket ——
  `ingress.listening` 是起飞最后一步，connect 得上就等于前面每步都过了。
  **刻意不用 `test -S`**：SIGKILL / panic / OOM 那条路不走 `ingress.stop()`，socket 文件
  留在命名卷里，`test -S` 返回 0 是**假绿**。

**日志轮转**：两个 service 都配了 `max-size: 10m` / `max-file: 5`。默认 json-file 驱动无上限，
配上 `restart: unless-stopped`，崩溃循环时日志涨得很快。

**下面五条都是实测、不是推断，但分两批**：前四条是 2026-09-12（W1）在本机真起了一遍 compose 核过的；**第五条（宿主机写得进）是 2026-09-13（AA1）容器降权那一轮补的** —— W1 那一轮两个进程还都以 root 跑，它当时不成立。

| 判据 | 结论 |
|---|---|
| 看日志 | `docker compose logs -f`（**不是**手起飞那两个终端）。要 grep 或往回执里贴，再接一层剥 ANSI：`docker compose logs --no-color core \| perl -pe 's/\e\[[0-9;]*m//g'` —— `--no-color` 只关 compose 自己那个 `core-1 \|` 前缀，管不到应用吐的字节 |
| 两个 socket | 都在命名卷 `run:` 里。**实测**：容器内 `/app/data/run/` 有 `aite-core.sock` + `aite-edge.sock`，宿主机 `data/run/` 是**空目录**（命名卷比 `./data` 那层 bind mount 更深，盖住了它）。**§0.2.3 那条 `ls data/run` 的判据在 compose 下不适用** |
| 仓库还挂不挂 | **不挂了。** 挂载表只有两条 bind：`./config → /app/config`（ro）与 `./data → /app/data`（rw），外加 `run:` 命名卷。容器里 `/app` 只有 `config` / `core` / `data` / `evals` 四项，**没有 `README.md`、没有 `docs/`** —— `core/crates`（取 `platform.md`）和 `evals`（取场景 yaml）是镜像 build 时 `COPY` 进去的，**冻在镜像里**，改宿主机的仓库不会改到容器里跑的那份 |
| `aite evidence show` 还在不在宿主机跑得了 | **照跑，结论没变，但理由换了** —— 靠的是 `./data:/app/data` 这条 bind mount，不是原来那条 `./:/app`。实测：容器写的 `data/aite.db` / `data/evidence` / `data/artifacts` 在宿主机上直接看得见，宿主机 `core/target/debug/aite evidence show --list` 读得到 |
| 宿主机**写得进** `./data` 吗 | **2026-09-13 起写得进；在那之前写不进。** 容器降权之前两个进程都以 root 跑，Linux 上产物是 `root:root`（0755/0644）—— 读得动，但写不进、删不掉、也没法在 `./data` 里新建。真后果在 §0.2 那条路上：**compose 跑过一次之后，宿主机直跑 `aite run` 起不来**，实测 `aite preflight --offline` 第 `[7/7]` 组 `FAIL 落盘目录可写`、退出码 1。降权之后同一发是 `[7/7] OK`、退出码 0。两条都是 CI 的硬门禁（`compose-smoke` 的第 7、8 步），不是承诺 |

⚠️ **`docker compose exec` 不过 ENTRYPOINT。** 镜像的 `ENTRYPOINT` 是 `aite`，但那只对
`run` 生效：`docker compose run --rm core preflight --offline` 能跑，而
`docker compose exec core evidence show --list` 报 `executable file not found in $PATH` ——
`exec` 那条路要自己把 `aite` 写出来：`docker compose exec core aite evidence show --list`。

⚠️ 别贴 `docker compose config` 的完整输出给任何人：它会把 `${VAR}` 解析成**取值**。
要校验语法走 `make compose-config`，它已经是 `env -u FEISHU_APP_ID …` 清掉四个密钥变量
之后再跑 `config -q` 的口径（`Makefile:56-60`，与 `.github/workflows/ci.yml` 同源）；
要给人看服务名用 `docker compose config --services`。

**§0.3 那四个观察窗在 compose 形态下分别是：**

| 窗口 | 手起飞 | compose |
|---|---|---|
| core 日志 | `aite run` 那个终端 | `docker compose logs -f core`（或 `make compose-logs` 一次跟两个） |
| edge 日志 | `aite-edge` 那个终端 | `docker compose logs -f edge` |
| 证据时间线 | `aite evidence show --list` | **在宿主机上照敲原命令**（`./data` 是 bind mount）。要在容器里看则是 `docker compose exec core aite evidence show --list` |
| 沙箱 | `docker ps --filter label=aite.task` | **一模一样，在宿主机敲**。沙箱是 edge 用宿主机 daemon 起的**兄弟容器**，不在 compose project 里 —— 所以 `docker compose ps` 里**看不到它们**，别去那儿找 |

### 0.3 四个观察窗

跑 M 之前把这四个窗口都开好，出了问题不用现找。
**日志是两个窗，不是一个** —— 这不是小事：M2 整条要看的 `feishu.*` 全在 **edge** 那边，
按旧文档只开 core 那个窗，你会盯着一个永远不出现那两行的窗口。

| 窗口 | 命令 | 看什么 |
|---|---|---|
| **core 日志（Rust）** | `aite run` 那个终端 | `aite.*` / `control.*` / `worker.*` / `ingress.slow_callback`、`ingress.handle_failed`。§7 上半张表 |
| **edge 日志（Go）** | `aite-edge` 那个终端 | `feishu.*` / `edge.*` / `sandbox.*` / `ingress.reconnecting`、`ingress.reconnected`。§7 下半张表 |
| 证据时间线 | `core/target/debug/aite evidence show --list` | 最近的任务、终态、链是否完整 |
| 沙箱 | `docker ps --filter label=aite.task` | 有没有容器、是不是该收没收 |

**每条 M 做完的固定收尾**：

```bash
# 1. 找到刚才那个任务（时间倒序，第一行就是）
core/target/debug/aite evidence show --list

# 2. 看它的时间线
core/target/debug/aite evidence show <task_id>
```

> `--config` 的默认值就是 `config/aite.yaml`（`core/crates/evidence/src/cli.rs:1320-1322`
> 的 `default_value`），**在仓库根跑根本不用带**。只有不在仓库根跑、或要指另一份配置时才写
> `--config <path>` —— 单价是从它里面读的，指错了花费那一栏就是 0。
> （`aite evidence show --help` 能打出全部七个参数：`--dir` / `--root` / `--config` /
> `--list` / `--only` / `--tail` / `--json`，退出码 0。**别写成 `-- --help`** ——
> 那样 `--help` 会被当成 task_id，报「找不到证据：data/evidence/--help/events.jsonl 不在」、退出码 2。）

时间线末尾那行 `hash 链` 是这份证据可不可信的判据。链断了 → 退出码 1，
并指出断在第几行、期望什么、实际什么。**链断了就别拿这份证据当验收依据**，
先确认目录有没有被人手改过、备份脚本有没有漏拷 `payloads/`。

`--list` 里任一任务链断了，整条命令也退出码 1，所以可以直接 `&&` 串在脚本里。

> 输出**开头那几行里**有一行 `目录 <证据目录>`（`任务 …` / `目录 …` 两行，
> `core/crates/evidence/src/cli.rs:976-977`）。**别数「第几行」** —— 前面可能先插一行
> `提示：…`（`cli.rs:1279` / `:1284` 走 `show <task_id>` 那条，`cli.rs:1399` 走 `--list` 那条）：
> 配置读不到、或者单价是 0 的时候就会有。

### 0.4 卡片上没有按钮，改成一行提示

**2026-09-12（RΩ）改**：卡片上**一个按钮都不渲染**（「停止」「证据」都没有），
改成在卡片末尾加一行文字提示。原因是 lark-oapi-go v3.12.0 在长连接上把非 event 帧
整条丢弃（`ws/client_message.go:79`），`card.action.trigger` 到不了任何 handler ——
渲染出来的按钮点了**一定**没反应，比不渲染更糟。

于是这两件事改成这么做：

| 本来想点的按钮 | 现在怎么做 |
|---|---|
| 「停止」 | **在卡片所在的那条话题里回复** `!stop <任务号>`；或在群里发 `@我 !stop <任务号>` |
| 「证据」 | 走 `aite evidence show --list` 找任务，再 `aite evidence show <task_id>` —— 开头那几行里的 `目录 <证据目录>` 就是原来那个按钮会回帖的路径 |

**卡片上那行提示的原文**（`edge/internal/feishu/cards.go:183-193`，`#A17` 处是真任务号）：

> 要停这个任务：在本话题里回复 !stop #A17，或在群里发「@我 !stop #A17」。（既不 @ 我、也不在本话题里的命令会被丢弃，不会有任何回应。）

**照抄它，不要转述** —— 文档里的说法和卡片上的说法不一致，用户会以为自己看错了。
（`cards_test.go` 有一条断言钉着提示里必须出现「话题」或「@」。）

⚠️ 两个限定，别搞错：

- **只有「进行中」的卡片有这行提示。** 它挂在 `actions` 里含 `STOP` 这个条件上
  （`cards.go:169` 的注释 + `:183-193` 的循环），**终态卡片（delivered / failed / cancelled）
  没有这一行** —— 已经结束的任务没什么可停的。
- **停止命令必须满足投递条件**：路由 R5 收 `!` 命令的条件是「这条消息 @ 了机器人」
  **或**「这条消息在一个已有会话的话题里」。两个都不满足（比如在群主输入框里干发一条
  `!stop #A17`）时，R6 要 thread 命中、R7 要 @，全不命中，最后落到 R8「其余丢弃」——
  只 bump 一次 `events.ignored`，**你那边零回复、零表情，看起来就像没人收到**。
  卡片是 `reply_in_thread=true` 发进任务话题的（`platform.go:394-396` 里 `SendCard`
  的 `in_thread` 写死 `true`），所以「在话题里回复」这条路**一定**走得通。

> ⚠️ **第一步就 `final` 的短任务，交付中的那几秒停不掉 —— 但 `!stop` 会明说，
> 不会告诉你「没有这个任务」；`!restart` 也会明说，不会假装替你停掉了它。**
>
> 机理还是原来那条：`deliver()` 在 W3 那一路（第一步就 `final`、从没发过卡片）把状态置成
> `Answering`（`core/crates/worker/src/agent.rs:645`），而 `Answering` **不在**
> `ACTIVE_TASK_STATUSES`（= created / planning / working，
> `core/crates/contracts/src/session.rs:33`）里，所以 `list_active_tasks` 空掉。
> 四条路现在查的是**同一份**列表：
>
> | 走哪条路 | 查的是什么 | 交付中的短任务 |
> |---|---|---|
> | `!status` | `status_tasks`：`list_active_tasks` **+ 控制面自己的 `running`**（过滤终态 + 用 `get_session` 过滤到本群），`plane.rs:558-591` | **列得出来**（V5 补的） |
> | `!stop <任务号>` | `resolve_stop_target`：同一个 `status_tasks`，`plane.rs:775-790` | 找得到，回「任务 #A17 正在把答复发给你，停不了了。」 |
> | 卡片 stop 按钮那条路 | `resolve_task`：同一个 `status_tasks`，`plane.rs:792-809` | 同上（P0 不渲染按钮，见本节开头） |
> | `!restart` | `cmd_restart`：同一个 `status_tasks`，分流同一条 `StopTarget::of_existing`，`plane.rs:683-736` | **一个字都不碰它**，改在回帖里点名：「任务 #A17 正在把答复发给你，停不了 —— 结果仍会回到原来那条话题里。」（AA2 补的） |
>
> **「交付中停不了」是刻意写下的约定，不是查不到**（W2 收的口）。理由在 worker：
> 取消标志位只在每一步的**开头**被看一眼（`agent.rs:117` 是全仓唯一一处），
> 而 `deliver()` 是最后一步之后的一段直路 —— 发文件 → 发答复 → 写 delivered 证据 →
> 收卡片 → `finish()`，**中间一个取消点都没有**。真去「停」它只会造出假话：
> 答复照发，`Cancelled` 随后被 `finish()` 的 `Delivered` 盖掉，回帖却说停了。
>
> **排障时要认得出这个形状**：任务在 `!status` 的列表里，`!stop` 回的是「正在把答复发给你」。
> 这是对的。**不是环境问题，别去查沙箱**；再等一两秒它就落 `delivered` 了。
> 回「没有这个任务」才不对 —— 那说明两条命令的口径又岔开了。
>
> **`!restart` 这一路多一条要认**：它会把旧会话归档，而那个交付中的任务**还在跑**，
> 答复与产物照样落进旧话题。所以「已重开会话」之后又从旧话题冒出一段答复是**对的**，
> 回帖里那句点名就是提前告诉你这件事。反过来，`!restart` 说「终止了 N 个进行中的任务」
> 却把交付中的那个算进了 N —— 那是假话，是 bug（AA2 收的口）。
>
> 发过卡片的正常任务置的是 `Working`，仍在活跃集里，`!stop` 照常停得掉，不受影响。

### 0.5 「一条纯文本回复（不是卡片）」—— 这句话要分成两半读

本文和 `docs/demo-3min.md` 原来都写着「随后线程里出现一条**纯文本**回复（不是卡片）」。
**这句话对了一半、错了一半**，而错的那一半在协议层是硬事实：

- ✅ **对的那半：没有 checklist 进度卡片。** W3 规定第一步就 `final` 的 Answering 路径
  不走 `send_card` —— 不会出现那张带 ⬜/✅ 待办、footer 在涨的卡片。这是真的。
- ❌ **错的那半：「不是卡片消息」/「是纯文本」。** core 发的**每一条文本**到飞书都是
  一张卡片：`SendText` 走 `DumpsCard(BuildMarkdownCard(text))`，`msg_type` 传的是
  `"interactive"`（`edge/internal/feishu/platform.go:379-390`）。

这是**故意的、正确的实现**，理由写在 `edge/internal/feishu/cards.go:239-243`：
`OutboundText.text` 的契约是 markdown，而飞书里唯一真能渲染 markdown 的载体就是
卡片的 markdown 元素 —— `msg_type=text` 会把 `**粗体**`、列表、链接原样当字面量吐出来。
错的是文档，**不是代码**。

所以本文一律这么写：**「没有 checklist 卡片」**，不写「不是卡片」。

> **视觉层还差一眼真机核实。** `BuildMarkdownCard` 拼出来的是一张**只有一个 markdown
> 元素、没有 header** 的卡片（`cards.go:244-249`）。它在飞书里到底长得像个普通文本气泡，
> 还是明显带卡片边框，只能在真飞书里看一眼。**M1 跑到的时候顺手看一下**，
> 结论决定 `demo-3min.md` §4.2 那句要对着镜头念的旁白用哪一版（那边留了两版文案）。

---

## M1 · 群里 @Aite 有反应

> §2.4：测试群 @Aite 你好 → 2 秒内触发消息出现 👀 类表情回应

### 操作

在测试群里发一条：`@Aite 你好`

### 在哪看

- 群里：**你发的那条消息**上的表情回应（不是机器人新发一条消息）。
- `aite evidence show --list`：应该多出一个任务。
- core 日志：没有 `ingress.slow_callback` / `ingress.handle_failed` / `control.dispatch_failed`。

### 期望

1. **2 秒内**你那条消息上出现 👀（飞书 `EYES` 表情）。这是 R7 的 `add_reaction(ack)`，
   在建会话之后、入队之前发出，所以它先于任何回复出现。
2. 随后线程里出现一条回复，**没有 checklist 卡片**（没有 ⬜/✅ 待办、没有在涨的 footer）。
   这是 W3：第一步就 `final` 的 Answering 路径不发卡片。
   ⚠️ 注意它**在协议层仍然是一条卡片消息**（`msg_type=interactive`，只含一个 markdown
   元素、没有 header）—— 见 §0.5。**顺手记一下它在飞书里长什么样**，`demo-3min.md`
   §4.2 的旁白等这个结论。
3. `aite evidence show <task_id>` 的时间线形如：

   ```
   0  task_created     #A1 「你好」 by=ou_… chat=oc_…
   1  event_received   message msg=om_… mentioned=True event=evt_…
   2  model_call       <模型名> step=0 finish=stop token in=… out=… ¥…
   3  tool_call        final(reply=…)
   ★  4  delivered     已交付 · 产出 0 个 · 1 步
   ```

   末尾 `hash 链 OK`，退出码 0。

### 不对时查哪（按最可能排序）

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 没表情、没回复、`--list` 也没有新任务 | 事件根本没到进程 | 开放平台「事件订阅 → 推送记录」看这条有没有推出来；没有就是权限/订阅没配（§3.7 清单）；有就先看 **edge** 那个窗的 `feishu.*`（长连接连上了没），再看 core 起飞那行 `aite.edge_status` 的 `platform_connected` 是不是 `true`（§0.2.3） |
| 同上，但推送记录里有 | 认不出 @ 的是自己 → R7 不命中 → R8 丢弃 | `FEISHU_BOT_OPEN_ID` 配的是不是这个应用的 open_id。跑 preflight 第 ④ 组 |
| 有表情，没回复 | 模型这一步炸了 | core 日志 `worker.model_failed`；`aite evidence show <task_id>` 看 `model_call` 那条的 `finish_reason`；跑 preflight 第 ⑤ 组 |
| 有表情有回复，但超过 2 秒才出现表情 | 回调里被塞了重活 | core 日志 `ingress.slow_callback`（>1s 就 WARN，带 `elapsed=`） |
| 交付的那几秒 `!stop` 回「任务 #A17 正在把答复发给你，停不了了。」 | **刻意的约定，不是故障** | 这一路是 W3 的 Answering 状态，`deliver()` 里没有取消点（`agent.rs:117` 是全仓唯一一处取消检查），所以它真停不掉，只能等它落 `delivered`。详见 §0.4 那段引用框。**别去查沙箱** |
| 交付的那几秒 `!stop` 回「**没有这个任务**」 | **这是真故障，不是记账** | W2 之后 `!stop` / 卡片按钮与 `!status` 查的是同一份 `status_tasks`，列得出来的任务不许被这一句说成不存在。真撞上了：`aite evidence show <task_id>` 看它是不是已经落了终态（终态会被 `status_tasks` 主动滤掉，那时回「没有这个任务」是对的）；不是终态就记下 `!status` 与 `!stop` 的原文，这是 bug |
| `!restart` 之后，已归档的旧话题里又冒出一段答复 | **刻意的约定，不是故障** | 那是一个交付中（`Answering`）的任务，`!restart` 停不掉它（同 `!stop`，理由见上上行），所以只点名不动手。回帖里应当有一句「任务 #A17 正在把答复发给你，停不了 —— 结果仍会回到原来那条话题里。」。**没有那句、或者回的是「终止了 N 个进行中的任务」而 N 把它算了进去 → 这才是 bug**（AA2 收的口，`plane.rs` 的 `cmd_restart`） |
| 交付的那几秒 `!status` 也查不到它 | **这是真故障，不是记账** | V5 之后 `status_tasks` 把控制面的 `running` 并了进来，交付中的短任务**应该**列得出来（§0.4）。列不出来说明并的那一半没生效：core 日志看这个任务的 `task_created` 在不在、`aite evidence show <task_id>` 看它是不是已经落了终态（终态会被 `status_tasks` 主动滤掉，那是对的）。两者都不是 → 记下 `!status` 的原文与 `--list` 输出，这是 bug |
| 机器人自己触发了自己 | R1 没拦住 | 不该发生（`sender_kind != human` 直接丢）。真出现了记下来，这是 bug 不是环境问题 |

---

## M2 · 断网重连，且只处理一次

> §2.4：拔网线 30 秒再插回 → 服务自动重连；断网期间群里发的 @ 在重连后被处理且只处理一次

### 操作

1. 关掉机器的网络（拔网线 / 关 Wi-Fi），**两个进程都不要停**。
2. 断网期间在群里发**一条** `@Aite 断网期间这条`。
3. 等 30 秒，恢复网络。

### 在哪看

> ⚠️ **这一条主要看 edge 那个终端，不是 core 那个。** 飞书长连接在 edge（Go）手里，
> `feishu.*` 全打在那边。core 那个窗在这一条里基本是安静的。

- **edge 日志**（Go，logfmt 形状）：
  ```
  level=WARN msg=feishu.connect_failed attempt=N err="…"
  level=WARN msg=feishu.reconnecting attempt=N delay_sec=N
  level=INFO msg=feishu.reconnected after=N
  ```
  字段名是 **`delay_sec`**（不是 `delay`），成功那条是 **`after=N`**（不是 `after=N attempts`）。
  退避是 1s→2s→…→30s 封顶、无限重试（§3.3）。连接中途掉了还会先打一条
  `level=WARN msg=feishu.connection_lost err=…`。
- `aite evidence show --list`：断网期间那条消息应该**只**对应**一个**任务。
- 计数器 `events.duplicate`：平台重连后重推同一事件时 +1（R2）。
  ⚠️ **core 侧计数器没有查看入口**（§7 末），所以这条只能靠「`--list` 里只有一个任务」反证。

### 期望

1. 断网期间 edge 日志持续打 `feishu.reconnecting`，**两个进程都不退出**。
2. 恢复网络后 edge 打出 `feishu.reconnected after=N`。
3. 断网期间那条 @ 被处理，群里有回复。
4. **只有一个任务**：`--list` 里对应那条消息的任务只有一条，不是两条。
   平台重推同一个 `event_id` 时 R2 靠 `seen_event` 静默丢弃。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 恢复网络后没有 `feishu.reconnected` | 重连循环挂了 | 看 edge 那个窗有没有 `feishu.reconnecting` 在持续打；完全没有就是读循环已经死了 —— 记下日志末尾，这是 bug |
| 断网期间那条 @ 完全没被处理 | 平台没重推 | 飞书的补推不保证；换成「断网 10 秒」再试一次。连续两次都不补推，就是平台行为，记进结论、不算代码问题 |
| **同一条消息出了两个任务** | R2 去重没生效 | `aite evidence show` 看这两个任务的 `event_received` 那条，`event=` 是不是同一个 `event_id`。是 → `seen_event` 没落库（查 sqlite 路径可写、`storage.sqlite_path` 配得对不对，preflight 第 ⑦ 组）；不是 → 平台推了两个不同 event_id，属于平台行为 |
| 进程直接退了 | 未捕获异常漏出去了 | §3.3 要求进程不退出。抓日志末尾的栈，这是 bug |
| 断网期间 core 那个窗在刷 `edge.reconnecting` | 你把 core↔edge 的 socket 也一起搞断了 | 那是**进程间**的链路，不是飞书长连接。拔网线不该影响它 —— 如果它也在重连，先看 edge 进程是不是被你一起关了 |

---

## M3 · CSV 画图：卡片、产物、沙箱回收

> §2.4：@Aite 把这个 CSV 画成月度趋势图（附 CSV）→ 线程里出现 checklist 卡片，
> 过程中卡片至少更新 3 次且不新增消息；结果 PNG 回到线程；5 分钟后 `docker ps` 无该任务容器

这是六条里最重的一条，它同时验 W3/W4/W5/W7 四条 worker 规则。

### 操作

1. 准备一个小 CSV（两列就行：`month,amount`，12 行），**别用大文件** ——
   这一步验的是链路，不是性能。
   （想省事就用 `core/target/debug/aite evals demo-fixture csv` 生成的
   `/tmp/aite-demo/sales.csv`，24 个月、演示也用它。）
2. 群里发：`@Aite 把这个 CSV 画成月度趋势图`，**同一条消息带上 CSV 附件**。

### 在哪看

- 群里那条线程：卡片消息**只有一条**，内容在变。
- `docker ps --filter label=aite.task`：任务跑的时候应该看得见一个容器。
- 跑完后：`aite evidence show <task_id>`。

### 期望

1. **卡片只有一条**。W3：第一次出现非 `final` 的 tool_call 时才 `send_card`；
   之后一律 `update_card`。「不新增消息」= 从你那条 @ 到最终文本回复之间，
   线程里新增的消息是 **1 张 checklist 卡片 + N 个产物文件 + 1 条文本回复**
   （M3 里 N=1，共 3 条），**checklist 卡片自始至终只有那一条、不重复出现**。
   ⚠️ 那条「文本回复」在协议层也是一条卡片消息（只含 markdown、没有 header），
   见 §0.5 —— 数消息条数时它算 1 条，但它不是第二张 checklist 卡片。
2. **卡片至少更新 3 次**。怎么数：
   - 肉眼：盯着卡片，checklist 的项从 ⬜ 逐个变 ✅，footer 的「已用 N 步 · ¥X.XX」在涨。
   - 证据侧的必要条件：`aite evidence show <task_id> --only checklist_op` 至少 3 行。
     每次 `checklist_check` 都写一条 evidence（W8），而卡片更新由它驱动。
   - ⚠️ **`update_card` 本身不写 evidence，也没有日志**，所以「真的调了 3 次」
     只能靠肉眼确认，证据里查不到。见 §8。
     （W4 会把 500ms 内的多次变更合并成一次 `update_card`，所以
     `checklist_op` 条数 ≥ 实际更新次数，是必要非充分条件。）
3. **PNG 回到同一线程**：文件消息的 `reply_to` 是话题 root（W5）。
4. 时间线里能看到这一串：

   ```
   tool_call        download_attachment(file_key=…, dest=/work/…)
   tool_result      download_attachment → ok …ms
   tool_call        run_python(code=…, timeout_sec=…)
   tool_result      run_python → ok …ms
   artifact         「月度趋势」 image/png …B sha=xxxxxxxx
   ★ delivered      已交付 · 产出 1 个 · N 步
   ```

5. **沙箱回收**：任务结束后容器**不会立刻消失**（W7 交给 reaper）。
   - reaper 每 **60 秒**跑一次（`core/crates/control/src/plane.rs:28`
     的 `REAPER_INTERVAL_SEC = 60.0`），回收空闲超过 `config.sandbox.idle_sec`
     （默认 **300 秒**）的容器。
   - 所以最坏情况是 **300 + 60 = 360 秒**。§2.4 写的「5 分钟后」踩在边界上：
     **建议等满 6 分钟再判**，5 分整还在的不算失败。
   - 回收时 core 日志打 `control.reaped`，计数器 `sandbox.reaped` +N。

   ```bash
   docker ps --filter label=aite.task          # 6 分钟后：空
   docker ps -a --filter label=aite.task       # 连停掉的也算：空
   ```

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 没出卡片，直接回了一段文字 | 模型一步就 `final` 了，没调工具 | 这是 W3 的 Answering 路径，**不算错**，但说明模型没去读附件。`--only tool_call` 看有没有 `download_attachment`；没有就是提示词/模型的问题。（这一路交付中 `!status` 列得出它、`!stop` 会明说停不掉，见 §0.4） |
| 卡片出来了，一直停在 working | 某个工具卡住 | `--only tool_call,tool_result` 看最后一条：只有 `tool_call` 没有 `tool_result` = 正卡在那一步；有 `tool_result` 且 `FAIL[timeout]` = 超时（`run_python` 用请求里的 `timeout_sec` + 余量，其余工具默认 60s，`core/crates/gateway/src/gateway.rs:238-248` 的 `budget()`） |
| `tool_result → FAIL[sandbox]` | Docker 不可用 / 镜像不在 | `docker images aite-sandbox`；不在就 `docker build -t aite-sandbox:p0 docker/sandbox`。连续 2 次 sandbox 失败 → 任务直接 failed（§3.3）。**edge** 那个窗会有 `sandbox.*` 的错 |
| `tool_result → FAIL[upstream]`（下载附件） | adapter 下载失败 | 附件是不是过期了/太大；换个小文件重试 |
| 回帖里有「产物 x 未找到」 | 模型给的 path 不在 /work 下或不存在 | §3.3 规定跳过该产物、任务仍 delivered。`--only artifact` 看实际写出去几个；`delivered` 那行的 `产物缺失 N 个` |
| 卡片更新次数不够 3 次 | 模型没建足够的 checklist 项 | `--only checklist_op` 数条数。少于 3 条是模型行为，不是链路故障 —— 换个更需要分步的任务重试 |
| 6 分钟后容器还在 | reaper 没跑 / 释放失败 | **core** 日志找 `control.reap_failed`（ERROR）、`control.release_failed`（WARN）、`gateway.release_failed`（WARN）；**edge** 日志找 `sandbox.release_failed`（WARN）。都没有就看 reaper 那条协程是不是根本没起（回到 §0.2.3 的起飞日志） |
| 群里有两条 checklist 卡片 | W3/W4 的合并没生效 | 这是 bug，把两条卡片的消息 id 和 `aite evidence show` 输出一起记下来 |

---

## M4 · 线程里追问，命中同一会话（= §3.7(a) 的实验）

> §2.4：线程里追问「再按季度画一张」（不带 @ 或带 @，取决于 3.7 的权限核实结果）
> → 命中同一 `session_id`（日志可查），沙箱重建，第二张图回到同一线程

### 这条 M 同时是 §3.7(a) 的那个实验

§3.7 的两条待核实项里，**(b) 在 §0.1.1 已经出结论了**；**(a) 只能在群里做实验**，
而这条实验就是 M4 的第 1 步 —— preflight 那行 NOTE 自己就这么说的（§0.1.1 末尾）。

**(a) 只有「接收 @ 消息」权限时，话题里不带 @ 的回复会不会投递给应用？**
决定 `PlatformCapabilities.supports_passive_listen` 的运行时取值
（契约默认 `FEISHU_P0.supports_passive_listen = False`，拿到「获取群组中所有消息」
后由 adapter 在运行时改成 `True`）。

**(a) 与路由逻辑无关**：R6 在 R7 之前求值，且**不要求 `mentioned`** ——
只要事件到得了进程、`anchor.thread_id` 命中 `find_session_by_thread`，
带不带 @ 都续接同一个会话。差别**只在投递**。

### 操作（五步，顺序别换 —— 这个顺序本身就是实验）

**前置**：M3 的任务必须已经彻底 `delivered`。没交付完就追问，R6 会把它当成 steer
并进上一个任务，不新建 —— 这一条就没了。

1. **在 M3 那条话题（卡片所在的线程）里回复** `再按季度画一张`，**不 @**。
2. **等 30 秒**，盯三个地方：群里有没有 👀、`aite evidence show --list` 有没有新任务、
   core 日志有没有动静。
3. **30 秒内有反应** → 走「情况 A」，跳到第 5 步。
   **30 秒内没反应** → 走「情况 B」，先做第 4 步再做第 5 步。
4. **判「没投递」还是「投递了被进程丢了」**（这一步是 (a) 结论的唯一判据，别跳）：
   1. 打开飞书开放平台 → 你这个自建应用 → **「事件订阅」→「推送记录 / 调试」**。
   2. 按时间找第 1 步那条消息（`im.message.receive_v1`）。
   3. **推送记录里没有这条** = 平台压根没投递 → **§3.7(a) 结论 =「不投递」**，
      权限问题，代码没错。
   4. **推送记录里有这条** = 投递了但进程丢了它 → R6 没命中，**这是 bug**：
      **停下来**，把这条消息的 `event_id` / `message_id` / `root_id`、
      M3 那个任务的 `task_created` 一行、core 日志同一时刻的片段一起记下来。
   5. ⚠️ **进程侧帮不上忙，不要指望在日志里找答案。** 被 R8 丢弃的事件只 bump 一次
      `events.ignored`（`core/crates/control/src/plane.rs:451-452`），**INFO 级没有任何日志**，
      而 core 侧计数器没有对外查看入口（§7 末、§8 第 3 条）。所以只有开放平台那份推送记录
      能分开这两种情况。
5. **补一条带 @ 的**：`@Aite 再按季度画一张`，还是发在同一条话题里。
   （情况 A 下这一步是为了确认带 @ 也照样走 R6；情况 B 下这是唯一能往下走的路。）

### 在哪看

- `aite evidence show --list`：应该多出**一个新任务**（不是新会话）。
- 新任务时间线第 0 条 `task_created` 里的 `session=` 字段，要和 M3 那个任务的一致。

  ```bash
  core/target/debug/aite evidence show <M3的task_id> --only task_created
  core/target/debug/aite evidence show <M4的task_id> --only task_created
  # 两行的 session= 必须相同
  ```

  用 `--json` 更好比：`…--json | jq -r '.events[0].fields.session_id'`
- `docker ps --filter label=aite.task`：M3 的容器多半已经被 reaper 收了，
  这一轮会**重建**一个新的（容器 id 不同）。

### 期望

**情况 A —— 不带 @ 就有反应**（`supports_passive_listen` 实际为 True）：

1. 第 1 步就触发任务，走 R6 续接。
2. 新任务的 `session_id` 与 M3 相同，`task_no` 比 M3 的**大**
   （租户内原子递增，中间有别的任务就会跳号，不一定正好 +1）。
3. 第二张图回到**同一条话题**里。
4. 结论：§3.7(a) = **不带 @ 也投递**。

**情况 B —— 不带 @ 没反应、带 @ 才有**（`supports_passive_listen` 为 False）：

1. 第 1 步 30 秒无任何动静：没表情、没回复、`--list` 没有新任务，
   且第 4 步在开放平台推送记录里**也没有**这条。
2. 第 5 步（带 @）触发任务，**同样走 R6**（thread_id 命中优先于 mentioned），
   所以 `session_id` 仍与 M3 相同、`task_no` 比 M3 的大。
3. 第二张图回到同一条话题里。
4. 结论：§3.7(a) = **不带 @ 不投递**。

**两种情况共同的判据**（这条 M 真正验的是它，与 (a) 的结论无关）：
`session_id` 相同 + 第二张图回到同一线程 + 沙箱重建。

### 两条结论分别该怎么继续

| §3.7(a) 的结论 | 对 M4 / M6 的影响 | 对演示的影响 | 代码要不要改 |
|---|---|---|---|
| **不带 @ 也投递** | M4 第 1 步就成；M6 的追问可以不带 @ | 第三幕可以不带 @，但 `demo-3min.md` §4.4 建议**仍然带 @** —— 理由是别在镜头前赌 | `FEISHU_P0.supports_passive_listen` 该置 `True`。**这是一条要转出去的代码改动**（不属于本剧本），记进结论交总管路由 |
| **不带 @ 不投递** | M4 / M6 全程带 @；飞书群的使用说明要写清「**话题里追问也要 @**」 | 第三幕照现在的台词演，无改动 | 保持 `False`，无改动 |

**这两条结论只写进本文和 `docs/demo-3min.md`，不回写 `docs/dev-spec-2026-09-09.md`** ——
§3.7 原文自己就写着「结果写在 dispatch 里，不改本文」。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 带 @ 也没反应 | 不是话题里的回复，是新消息 | 飞书里「回复」和「在群里另发一条」不是一回事。确认发的是**对卡片那条消息的回复**（线程内）。⚠️ 这里说的是**飞书自己的「回复」入口**（长按消息 / 悬浮菜单里那个），不是卡片上的按钮 —— 卡片上一个按钮都没有（§0.4） |
| 有反应，但 `session_id` 变了 | `anchor.thread_id` 没命中 | `--only task_created,event_received` 看新任务的 `msg=`；R7 会把这条消息自己变成新话题 root。多半是回复挂错了父消息 |
| 有反应，但图回到了群里而不是线程 | `reply_to` 没带话题 root | W5 要求 `reply_to = 话题 root`。这是 bug |
| 第一步就有反应，但回了「未知命令」 | 文本以 `!` 开头了 | R5 先于 R6 判定。别用 `!` 开头（可用命令：`!status` `!stop <任务号>` `!restart` `!new`） |
| 追问被当成了 steer（合并进当前任务）而不是新任务 | M3 的任务还在跑 | R6：会话有活跃 task → 排队为 steer 消息。**等 M3 彻底 delivered 再做 M4** |

---

## M5 · 汇总群历史，引用真实消息

> §2.4：@Aite 汇总本群本周开放事项 → 回复引用到 ≥3 条真实群消息

**前置：§0.1.1 那一步必须先出「读得到」的结论。** 读不到就先去开放平台补权限，
在那之前这条 M 一定过不了 —— 别拿它当链路故障查。

**先在群里制造素材**：至少 5–6 条不同人发的、带明确待办口吻的消息
（「X 那个还没弄完」「Y 下周之前给我」之类），否则模型没得引。
省事的办法是 `core/target/debug/aite evals demo-fixture history` ——
它在 `/tmp/aite-demo/history.txt` 里给了 8 条现成的（其中 2 条是故意的干扰项），
按文件里的说明发进群，**一条都不要 @Aite**。

### 操作

群里发：`@Aite 汇总本群本周开放事项`

### 在哪看

- 群里的回复正文：里面应该带得出**具体的消息内容**，能和群里真实存在的消息对上。
- `aite evidence show <task_id> --only tool_call,tool_result`：
  应该有一条 `read_group_history`。

### 期望

1. 时间线里有 `tool_call read_group_history(...)` 且对应的 `tool_result → ok`。
2. 回复正文里能对上 **≥3 条**真实群消息。
   - W1 给模型的群历史格式是 `[message_id] 姓名: 文本`，且**只保留 `sender_kind == "human"`**
     （`core/crates/worker/src/context.rs:70-82`）。
   - 提示词要求它引用时带上 `[message_id]`（`core/crates/worker/prompts/platform.md:54`），
     所以回复里会夹着一串 `[om_xxxx]`。**它们在飞书里不可点**，是给你逐条核对用的。
   - 逐条核：回复里提到的每件事，都能在群里找到那条原始消息。
     **模型编出来的算不通过** —— 这条 M 验的就是「它真的读到了群历史」。
3. 群历史窗口是 `config.feishu.history_window`（默认 50 条）。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| `tool_result → FAIL[denied]` 或 4xx | §3.7(b)：缺「获取群组中所有消息」敏感权限 | 回 §0.1.1 带 `--chat-id` 重跑一次 preflight，它会直接给你那句结论；再去开放平台看这个权限的审批状态 |
| `read_group_history` 返回了，但内容是空的 | 群里没有符合条件的消息 | 拉历史只留 human；机器人自己发的不算。先在群里补几条真人消息 |
| core 日志 `worker.read_history_failed` | 拉历史抛异常了 | 看栈。W1 里拉历史失败是软失败（继续跑，只是上下文里没有群历史），所以任务仍会 delivered —— **别被「有回复」骗了**，一定要核 `tool_result` |
| 回复里的事项在群里找不到 | 模型编的 | 不是链路故障。`--only tool_call,tool_result` 确认历史真的拉到了；拉到了还编，是提示词/模型的问题（W9 明确要求「不得声称做了没做的事」） |
| 压根没调 `read_group_history` | 模型没想到要用 | 换个更明确的说法重试（「读一下最近的群消息，汇总…」）。仍不调 = 工具目录/提示词问题 |

---

## M6 · 重启后旧线程还能续接

> §2.4：`systemctl restart` / 杀进程重启后在旧线程追问 → 仍能续接

### 操作

现在是两个进程，所以 **M6 要跑三遍**（spec §4.4）：只重启 edge / 只重启 core / 两个都重启。
每一遍都是同样的五步，区别只在第 2–4 步重启的是谁。

1. 记下 M3/M4 那条话题。
2. **停掉要重启的那个**（Ctrl-C，或 `kill <pid>`；SIGTERM 走同一条优雅退出路径）。
   **刚起飞就按也可以**，不必等 `aite.up` 出来 —— 信号落在起飞半路照样走优雅退出
   （2026-09-13 之前不是这样：那时会卡住，第 3 步的 `pgrep` 一直能看到它，只剩 `kill -9`）。
3. **确认它真的没了**：`pgrep -fl 'aite run'` / `pgrep -fl aite-edge`。
4. 重新起飞（命令见 §0.2.2，**注意仍然在仓库根**）。
5. 在**同一条旧话题**里追问：`@Aite 刚才那张图换成柱状的`（按 M4 的结论决定带不带 @）。

| 这一遍重启谁 | 另一边应该发生什么 | 额外要看的 |
|---|---|---|
| **只重启 edge** | core 一直活着，会话与任务号都在内存 + 库里。**core 日志**出现 `edge.reconnecting` → `edge.connected`（⚠️ core 侧**没有**「重连成功」那个专用名字：首次连上和重连成功打的都是 `edge.connected`。`core/crates/edge-client/src/link.rs:9` 的注释写明全部只有三个名字 —— `edge.connecting` / `edge.connected` / `edge.reconnecting`。见 §7） | 重连之后追问照常办；断开期间群里发的消息由飞书重推，**只处理一次**（去重在 core） |
| **只重启 core** | edge 一直活着，长连接没断。**edge 日志**出现 `ingress.reconnecting` → `ingress.reconnected socket=… reconnects=N`（**第一次连上打的是 `ingress.connected`，不算重连**，计数从 0 起） | 这一遍才是「会话从 SQLite 里读回来」的正题，下面的期望 1–3 说的就是它 |
| **两个都重启** | 两边都从零开始，先起谁都行 | 上面两行的判据合起来都要成立 |

### 在哪看

- 重启后的起飞日志：`aite.up` 里 `sqlite=` 那一段要和重启前**是同一个文件**。
- `aite evidence show --list`：新任务的 `session_id` 要和旧任务相同。

### 期望

1. 新任务的 `session_id` 与重启前那条话题的会话相同 —— 会话在 SQLite 里，
   进程重启不丢（`find_session_by_thread` 按 `anchor.thread_id` 查）。
2. `task_no` 在旧编号上继续递增，不从 `#A1` 重来。
3. 证据链接得上：新任务是**新的** `{evidence_dir}/{task_id}/` 目录
   （证据按 task 分目录，不按 session），链从 GENESIS 重新起，这是对的。

### 顺手多验一条：**杀 core 时正在跑的那个任务会被收场**

这是 RΩ 新接上的一条，原来的剧本里没有 —— 别把它当成 bug。

用 `kill -9 <core 的 pid>`（**硬杀**，不走优雅退出）制造一个「崩溃时正在干活」的任务，
然后重新起飞。起飞会把它收干净，**在 `platform.start()` 之前**：

- core 日志：`aite.orphans n=1` → 逐个收场；
- 库里那个任务落 `failed`，`result_summary` 是「进程重启前该任务仍在执行，已终止。请重新发起。」；
- **群里那条话题多一条回帖**：`任务 #A3：进程重启前该任务仍在执行，已终止。请重新发起。`；
- **那张停在「进行中」的卡片被原地改成 failed**（不新发卡片）——
  这是 core 主动 `update_card`，与「卡片上没有按钮」无关，✅ 正常；
- 证据链补一条 `failed`（`by=startup_recovery`）并 finalize。

```bash
core/target/debug/aite evidence show <被杀掉的task_id>
# 期望：最后一条是 failed，manifest 有了，hash 链 OK，退出码 0
```

如果是**优雅退出**（Ctrl-C）而不是硬杀，在跑的任务会在宽限期内自己善终，
落 `delivered` 或 `cancelled`，不会走上面这条收残局的路。

> **「未 finalize」不等于「证据损坏」** —— 这是排障时最容易误判的一处。
> 硬杀之后、重新起飞**之前**去看那个目录，它就是「未 finalize」的样子，退出码仍然是 0。

### 不对时查哪

| 症状 | 最可能的原因 | 具体动作 |
|---|---|---|
| 重启后追问变成了新会话（`session_id` 不同） | SQLite 换文件了 | 比对重启前后 `aite.up` 里的 `sqlite=`；**相对路径 + 换了工作目录最常见** —— 也就是 §0.2 那个坑的另一个形态，确认你重启时的 cwd 还是仓库根 |
| 重启后追问完全没反应 | 进程没真起来 / 没连上 | 起飞日志那几行（§0.2.3）；`aite preflight` 第 ③ 组。先确认 `aite.edge_status` 出现了而不是 `aite.edge_unreachable` |
| `task_no` 从 `#A1` 重来 | `next_task_no` 的计数表没落库 | 编号存在 SQLite 的 `task_counters` 表里，重启该接得上。回退到 `#A1` = sqlite 换文件了（同上一行），或计数没提交 —— 贴 `--list` 输出 |
| 重启前那个跑到一半的任务，重启后自己接着跑了 | 不该发生 | P0 没有任务**恢复**（只有**收场**）。它应该被起飞时收成 `failed` 并在群里回一句，而不是接着跑。真自己跑起来了，记下来 |
| 硬杀之后重启，群里没有那句「已终止」 | 收残局这一步没走到 | core 日志找 `aite.orphans` / `aite.recover_failed` / `aite.orphan_notice_failed`。一个孤儿收不掉不该连累别人，也不该让进程起不来 —— 进程起来了但没回帖，看这三个日志名哪个出现了 |
| Ctrl-C 按不动 | 优雅退出卡住了 | **再按一次是硬退**（T7 的实现里第二次信号立即硬退）。要记下第一次为什么没退 |

---

## 7. 日志关键字速查

**先看这一栏：「哪个进程」。** 两个进程两个终端两套日志格式 ——
core 是 Rust 的 tracing（`INFO aite.app: aite.up platform=…`），
edge 是 Go 的 slog logfmt（`level=INFO msg=edge.takeoff version=…`）。
拿着 core 的关键字去 edge 那个窗里找，或者反过来，是这一节最容易犯的错。

### core（Rust，`aite run` 那个终端）

| 关键字 | 级别 | 说明 |
|---|---|---|
| `aite.up` | INFO | 起飞成功，六栏「接了谁」。§0.2.3 |
| `aite.edge_status` | INFO | 问到了 edge；`platform_connected` 要是 `true` |
| `aite.edge_unreachable` | WARN | 5×1s 都没问到 edge，版本门禁这一轮没生效但照常起飞 —— **§0.2 那个坑的第一现场** |
| `aite.orphans n=N` | WARN | 起飞时发现上一条命没跑完的任务，逐个收场。M6 要看的 |
| `aite.recover_failed` / `aite.orphan_notice_failed` | ERROR | 残局没查出来 / 收场的回帖没发出去（都不阻断起飞） |
| `aite.signal` / `aite.stopping` / `aite.down` | WARN / INFO / INFO | 优雅退出三连 |
| `ingress.slow_callback event=… elapsed=…` | WARN | 回调超过 1s，违反 §3.3 第一行 |
| `ingress.handle_failed event=… kind=… error=…` | ERROR | 路由里抛异常，已吞掉不让链路挂掉。⚠️ **这个名字两边都有**，edge 也打一条同名的 |
| `ingress.listening` / `ingress.stopped` | INFO | core 侧 `aite-core.sock` 的起停 |
| `edge.connecting` | INFO | 开始拨 edge 的 socket |
| `edge.connected` | INFO | 拨通了。**首次连上和重连成功用的是同一个名字** |
| `edge.reconnecting socket=… error=… retry_in_sec=N` | WARN | 拨不通，退避重拨（1→2→…→30s 封顶） |
| `control.dispatch_failed` | ERROR | 派发任务失败 |
| `control.no_worker` | WARN | 没接 worker（组装漏了，回到 §0.2） |
| `control.reaped` | INFO | reaper 收了容器。M3 第 5 条要看的 |
| `control.reap_failed` | ERROR | 回收失败，容器会留着 |
| `control.release_failed` | WARN | 取消任务时沙箱没还回去 |
| `gateway.release_failed` | WARN | 网关那一侧沙箱没还回去 |
| `preflight.release_failed` | WARN | 只在 `aite preflight` 第 6 组里出现 |
| `worker.model_failed` | ERROR | 模型调用炸了（已重试 2 次，2s/5s） |
| `worker.read_history_failed` | ERROR | 拉群历史失败（软失败，任务继续）。M5 要看的 |
| `worker.artifact_failed` | ERROR | 取产物失败 |
| `worker.fail_notice_failed` | ERROR | 连「任务失败」的回帖都没发出去 |
| `worker.unhandled` | ERROR | 未捕获异常，任务 failed 但进程活着 |

### edge（Go，`aite-edge` 那个终端）

| 关键字 | 级别 | 说明 |
|---|---|---|
| `edge.takeoff` / `edge.grpc_listening` / `edge.sandbox_ok` | INFO | 起飞三连。§0.2.3 |
| `edge.signal` / `edge.shutting_down` / `edge.counters` / `edge.down` | INFO | 优雅退出四连 |
| `feishu.connect_failed attempt=N err=…` | WARN | 建连失败（凭证错、网络不通都走这条） |
| `feishu.reconnecting attempt=N delay_sec=N` | WARN | 长连接在退避重连（1s→2s→…→30s 封顶）。字段名是 **`delay_sec`** |
| `feishu.reconnected after=N` | INFO | 重连成功。M2 要看的就是它。字段是 **`after`**，没有 `attempts` 这个词 |
| `feishu.connection_lost err=…` | WARN | 连着的长连接掉了（接着会打 reconnecting） |
| `ingress.connected socket=…` | INFO | 首次连上 core 的 `aite-core.sock`（**不计入重连**） |
| `ingress.reconnecting socket=… base_delay=… max_delay=…` | WARN | 到 core 的 gRPC 掉了，退避重拨 |
| `ingress.reconnected socket=… reconnects=N` | INFO | 重新连上 core。M6「只重启 core」那一遍要看的 |
| `ingress.invalid event=… kind=… err=…` | WARN | core 说这条事件非法，**不重推** |
| `ingress.handle_failed event=… kind=… err=…` | ERROR | 调 core 的 `HandleEvent` 失败，让平台重推。⚠️ **同名的一条在 core 那边也有** |
| `sandbox.acquired sandbox=… task=… image=…` | INFO | **容器起来了** —— M3 里容器在「下载附件」那一步就出现，看的就是它 |
| `sandbox.released sandbox=…` | INFO | 单个容器还回去了（任务收尾时） |
| `sandbox.reaped count=N idle_sec=N` | INFO | reaper 这一轮收了 N 个。⚠️ **这是 edge 侧的那一行**，core 侧那行叫 `control.reaped` |
| `sandbox.release_failed sandbox=… err=…` | WARN | 容器没删掉。M3「6 分钟后容器还在」要一起看 |
| `sandbox.ready_failed sandbox=… task=… err=…` | WARN | 容器起来了但没就绪 |
| `sandbox.exec_timeout sandbox=… timeout_sec=N` | **INFO** | 代码跑超时了 —— 级别是 INFO，**在一堆 INFO 里不显眼，别指望它跳出来** |

### 计数器：core 侧基本没有出口，edge 侧只在退出时给一次

- **core 侧**（`ControlPlane` / `Ingress`）：`events.handled` / `events.duplicate` /
  `events.nonhuman` / `events.ignored` / `events.steer` / `events.edited` /
  `events.deleted` / `events.dropped` / `sandbox.reaped` / `ingress.errors` / `ingress.slow`。
  **没有对外查看入口** —— `counters()`（`core/crates/control/src/plane.rs:1436`）全仓没有
  任何非测试调用方，`!status` 只回任务列表、一个计数器都不回（`cmd_status`，
  `plane.rs:614-637`；它列的是什么见 §0.4）。见 §8 第 3 条。
  - 唯一的例外：`events.dropped` 不为 0 时，`!status` 的回复末尾会多一句
    「⚠ 本进程启动以来有 N 条事件没接住…」（`cmd_status` 里的两处 `dropped_note()` 调用，
    `plane.rs:617` 与 `:634`；函数体在 `plane.rs:754-763`）。
    **只有这一个计数器漏了出来，`events.ignored` 没有。**
- **edge 侧**：进程**退出时**打一行（实测）
  `level=INFO msg=edge.counters events.sent=0 ingress.invalid=0 ingress.errors=0 ingress.reconnects=0`
  （`edge/cmd/aite-edge/main.go:196-199`）。想看就得停一次 edge —— 跑 M 的过程中拿不到。

### `!status` / `!stop` / `!restart` 的三条使用口径

- `!stop` 在**本群只有一个任务**时可以省略任务号（`plane.rs:775-790`）；「只有一个」按
  `!status` 列出来的那份算。给了任务号的话 `#A17` / `a17` / ` #a17 ` 都收
  （`normalize_task_no` 去空格 + 补 `#` + 转大写）。
- 两条命令都要满足 R5 的投递条件（@ 机器人，或在已有会话的话题里），见 §0.4。
- **这几条路查的是同一份列表**（W2 收的口，AA2 补上最后一条）：`!status`、`!stop`、
  卡片 stop 按钮、`!restart` 都走 `status_tasks`（活跃集 **+ 控制面 `running`**，
  过滤终态 + 过滤到本群）。交付中的短任务因此**列得出来，也认得出来** —— 只是停不掉，
  `!stop` 会明说「正在把答复发给你，停不了了」，而不是「没有这个任务」。
  为什么停不掉见 §0.4 那段引用框。
- **`!restart` 与 `!stop` 的差别一句话**：`!stop` 是指着**一个**任务问「停它」，
  `!restart` 是换一个会话、顺手把旧会话名下**所有能停的**都停掉；
  两者对「能不能停」的判定完全同源（`StopTarget::of_existing`）。
  所以 `!restart` 的「终止了 N 个进行中的任务。」里的 N **只数真停掉的那些**，
  交付中的那些单独点名（`plane.rs` 的 `restart_while_delivering_text`）。

---

## 8. 跑这份剧本时发现的观测缺口

写剧本时发现有几件真机排障需要的事，现在**查不到**。这里只记录，
代码改动不属于本轨，留给下一轮：

1. **`created_at` 不进 hash 链。** `hash = chain_hash(prev_hash, payload_hash)`，
   而 `payload_hash` 只覆盖 payload —— 改掉 `events.jsonl` 里所有时间戳，
   链校验照样全绿。M2/M3 的时序判断建立在这些时间戳上，值得知道它没被保护。
2. **卡片的发送与更新不写 evidence，也没有日志。** M3 明确要求「卡片至少更新 3 次
   且不新增消息」，但 `send_card` / `update_card` 在证据里没有任何痕迹，
   只能靠肉眼数。建议加 `card_sent` / `card_updated` 两类事件（或复用 `checklist_op`）。
3. ~~**被丢弃的事件不留痕。**~~ **BB1 已销大半，剩下的那一半写在本条末尾。**

   原文：R1/R2/R8 丢弃事件时只加内存计数器，INFO 级别没有日志；**core 侧计数器没有
   查看入口**（`counters()` 全仓零调用方；只有 `events.dropped` 经 `!status` 尾巴
   漏出来一点），所以 M4 判「没投递 vs 投递了被丢」只能去开放平台看推送记录（M4 第 4 步）。
   ⚠️ edge 侧这一条早就不成立了：退出时会打一行 `edge.counters`
   （`events.sent` / `ingress.invalid` / `ingress.errors` / `ingress.reconnects`），见 §7 末。

   **BB1 之后（2026-09-15）：**
   - 三条丢弃规则各打一行 INFO，**一条规则一个名字**，都在 `aite.control` 这个 target 上：

     | 规则 | 日志名 | 字段 |
     |---|---|---|
     | R1 非真人 | `control.drop_nonhuman` | `event` `kind` `sender_kind` `sender` |
     | R2 重推去重 | `control.drop_duplicate` | `event` `kind` |
     | R8 其余丢弃 | `control.drop_ignored` | `event` `kind` `chat_type` `mentioned` `thread` |

     所以 M4 第 4 步现在可以先在 core 日志里 `grep <event_id>`：命中
     `control.drop_*` 就是**投递了被丢**（还能看出被哪条规则丢的），一条都不命中才是
     **没投递**，那时才需要去开放平台翻推送记录。
   - **core 侧计数器有出口了**：收尾时打一行 `aite.counters`，排在 `aite.down` 前面 ——
     退出四连成了 `aite.signal` / `aite.stopping` / `aite.counters` / `aite.down`，
     和 edge 那边对齐。真机实测（没接到任何事件的一次退出）：

     ```text
     INFO aite.app: aite.counters 本进程启动以来的计数器 plane=[] ingress=[]
     ```

     有数的时候方括号里是 `k=v` 空格分隔，两格分别是控制面与 Ingress 的计数器，例如
     `plane=[commands!status=1 events.ignored=1] ingress=[events.handled=2]`。
   - `!stop` 落库失败那一笔从 `events.dropped` 里**拆出来**了，自己叫
     `control.cancel_save_failed`（日志名同名）。`!status` 尾巴那句警告数的是两者之和，
     文案一个字没改。

   **还缺的那一半**：**跑着的时候**查不到 —— 要停一次进程才看得到 `aite.counters`，
   和 edge 侧是同一个形状。`!status` 尾巴现在仍然只漏「没接住」那一个数
   （`events.ignored` 之类一个都不说）；扩它要改 `wording.rs` 里逐字钉住的文案
   并连带改一批测试，是产品决定，BB1 没擅自扩，建议见 BB1 回执 ③。
4. **卡片上一个按钮都没有（RΩ 起）。** 契约 R3 的 `stop` / `evidence` 两个动作都还在，
   但 lark-oapi-go v3.12.0 收不到卡片回传帧，渲染出来的按钮点了一定没反应，
   所以现在一个都不渲染，改成卡片末尾一行文字提示（见 §0.4）。
   渲染那条路（`buildActions`，`cards.go:151-166`）没删，SDK 放开钩子后改回去即可。
   「停止」有等价的命令路径（`!stop`，注意投递条件）；「看证据」只能走 CLI。
   ⚠️ 但 `!stop` 与卡片 stop 按钮走的是**同一份**列表（`status_tasks`，W2 起）——
   两条路对交付中的短任务（Answering）给的是同一句「正在把答复发给你，停不了了」，见 §0.4。
5. **`checklist_op` 的 check/fail 只记 `id` 和 `state`，不带那一项的文本。**
   `aite evidence show` 已经通过回放前面的 `add` 事件把文本补了回来，
   但这意味着**单看一条 `checklist_op` 是读不懂的**，任何别的消费方都得自己回放。
6. **`tool_result` 只有 `content_hash`，没有摘要。** 工具失败时证据里只有
   `error` 的错误码（`timeout` / `sandbox` / `invalid_args` …），
   拿不到那一行具体的报错文本。排 M3 的沙箱问题时这一点最疼。
7. **沙箱 id 不进证据。** 任务和容器对不上号，M3 查「哪个容器该收没收」
   只能靠时间先后猜。
