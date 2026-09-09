"""场景 `expect:` 的断言 DSL。

每条断言是一个 mapping，`check` 选类型，其余键是参数。检查函数返回 `None` 表示通过，
返回字符串表示失败原因（要写清「期望什么、实际什么」，报告里就靠这句话）。

比较子统一是 `equals` / `min` / `max`，可以同时给。例：

```yaml
expect:
  - {check: platform_calls, method: send_card, equals: 1}
  - {check: platform_calls, method: update_card, min: 3}
  - {check: cards, distinct_equals: 1}
  - {check: text, where: last, contains: 完成}
  - {check: file, index: 0, magic: 89504e470d0a1a0a}
  - {check: gateway_calls, name: run_python, min: 1}
  - {check: distinct_matches, source: last_text, pattern: 'om_[a-z0-9]+', min: 3}
```
"""
from __future__ import annotations

import re
from collections.abc import Callable
from typing import Any

from .wiring import Deps

CheckFn = Callable[[Deps, dict[str, Any]], str | None]
REGISTRY: dict[str, CheckFn] = {}


class CheckError(ValueError):
    """断言本身写错了（未知 check、缺参数）—— 与「断言没通过」区分开。"""


def check(name: str) -> Callable[[CheckFn], CheckFn]:
    def deco(fn: CheckFn) -> CheckFn:
        REGISTRY[name] = fn
        return fn

    return deco


def _compare(label: str, actual: int, spec: dict[str, Any]) -> str | None:
    """按 equals / min / max 比一个整数。一个都没给就是断言写漏了。"""
    if not {"equals", "min", "max"} & set(spec):
        raise CheckError(f"{label}: 至少要给 equals / min / max 之一")
    if "equals" in spec and actual != spec["equals"]:
        return f"{label} 期望 == {spec['equals']}，实际 {actual}"
    if "min" in spec and actual < spec["min"]:
        return f"{label} 期望 >= {spec['min']}，实际 {actual}"
    if "max" in spec and actual > spec["max"]:
        return f"{label} 期望 <= {spec['max']}，实际 {actual}"
    return None


def _need(spec: dict[str, Any], key: str) -> Any:
    if key not in spec:
        raise CheckError(f"check={spec.get('check')!r} 缺少必填键 {key!r}")
    return spec[key]


# --------------------------------------------------------------------------

@check("platform_calls")
def _platform_calls(deps: Deps, spec: dict[str, Any]) -> str | None:
    """出站/入站方法被调了几次。method 取 send_text / send_card / update_card /
    send_file / add_reaction / read_history / read_document / download_file。"""
    method = _need(spec, "method")
    return _compare(f"platform.{method}", deps.platform.calls.count(method), spec)


@check("outbound_total")
def _outbound_total(deps: Deps, spec: dict[str, Any]) -> str | None:
    """所有出站动作之和。09_bot_ignored 用 {equals: 0}。"""
    return _compare("出站动作总数", deps.platform.outbound_count, spec)


@check("cards")
def _cards(deps: Deps, spec: dict[str, Any]) -> str | None:
    """卡片：发了几张（distinct_equals）、更新了几次（updates_min/updates_equals）、
    最后一张的终态（final_status）。"""
    if "distinct_equals" in spec and deps.platform.card_count != spec["distinct_equals"]:
        return (
            f"卡片张数期望 == {spec['distinct_equals']}，实际 {deps.platform.card_count}"
            f"（card_id：{sorted(deps.platform.cards)}）"
        )
    if "updates_min" in spec and deps.platform.update_count < spec["updates_min"]:
        return f"update_card 次数期望 >= {spec['updates_min']}，实际 {deps.platform.update_count}"
    if "updates_equals" in spec and deps.platform.update_count != spec["updates_equals"]:
        return f"update_card 次数期望 == {spec['updates_equals']}，实际 {deps.platform.update_count}"
    if "final_status" in spec:
        snaps = deps.platform.card_snapshots()
        if not snaps:
            return f"卡片终态期望 {spec['final_status']}，实际一张卡片都没发"
        if snaps[-1].status != spec["final_status"]:
            return f"卡片终态期望 {spec['final_status']}，实际 {snaps[-1].status}"
    if not {"distinct_equals", "updates_min", "updates_equals", "final_status"} & set(spec):
        raise CheckError("check=cards 至少要给 distinct_equals / updates_min / updates_equals / final_status")
    return None


@check("text")
def _text(deps: Deps, spec: dict[str, Any]) -> str | None:
    """发出去的文本消息。where: any（默认）/ last / all；contains / not_contains / matches。"""
    texts = deps.platform.texts()
    where = spec.get("where", "any")
    if where == "last":
        pool = texts[-1:]
    elif where in ("any", "all"):
        pool = texts
    else:
        raise CheckError(f"check=text 的 where 只能是 any / last / all，收到 {where!r}")

    if "contains" in spec:
        want = str(spec["contains"])
        hit = [t for t in pool if want in t]
        ok = (bool(pool) and len(hit) == len(pool)) if where == "all" else bool(hit)
        if not ok:
            return f"没有{'每条' if where == 'all' else ''}文本包含 {want!r}；实际文本：{pool or '(一条都没有)'}"
    if "not_contains" in spec:
        bad = str(spec["not_contains"])
        hit = [t for t in pool if bad in t]
        if hit:
            return f"文本不该包含 {bad!r}，但出现在：{hit}"
    if "matches" in spec:
        pattern = re.compile(str(spec["matches"]))
        if not any(pattern.search(t) for t in pool):
            return f"没有文本匹配 /{spec['matches']}/；实际文本：{pool or '(一条都没有)'}"
    if not {"contains", "not_contains", "matches"} & set(spec):
        raise CheckError("check=text 至少要给 contains / not_contains / matches")
    return None


@check("distinct_matches")
def _distinct_matches(deps: Deps, spec: dict[str, Any]) -> str | None:
    """在文本里数「不重复的匹配」有几个。05 用它验回复引用了 >=3 个 message_id。"""
    source = spec.get("source", "last_text")
    texts = deps.platform.texts()
    if source == "last_text":
        pool = texts[-1:]
    elif source == "all_texts":
        pool = texts
    else:
        raise CheckError(f"check=distinct_matches 的 source 只能是 last_text / all_texts，收到 {source!r}")
    pattern = re.compile(str(_need(spec, "pattern")))
    found: set[str] = set()
    for t in pool:
        found.update(pattern.findall(t))
    hint = f"（命中：{sorted(found)}；文本：{pool or '(一条都没有)'}）"
    result = _compare(f"/{spec['pattern']}/ 的不重复命中数", len(found), spec)
    return None if result is None else result + hint


@check("file")
def _file(deps: Deps, spec: dict[str, Any]) -> str | None:
    """发出去的文件。index 默认 0；magic 是十六进制前缀（PNG = 89504e470d0a1a0a）。"""
    files = deps.platform.sent_files
    index = int(spec.get("index", 0))
    if not files or index >= len(files):
        return f"期望第 {index} 个 send_file，实际只发了 {len(files)} 个文件"
    f = files[index]
    if "magic" in spec:
        want = bytes.fromhex(str(spec["magic"]))
        got = f.data[: len(want)]
        if got != want:
            return f"文件 {f.name} 的前 {len(want)} 字节期望 {want.hex()}，实际 {got.hex()}"
    if "name_suffix" in spec and not f.name.endswith(str(spec["name_suffix"])):
        return f"文件名期望以 {spec['name_suffix']!r} 结尾，实际 {f.name!r}"
    if "mime" in spec and f.mime != spec["mime"]:
        return f"文件 mime 期望 {spec['mime']!r}，实际 {f.mime!r}"
    if "min_size" in spec and len(f.data) < int(spec["min_size"]):
        return f"文件 {f.name} 期望至少 {spec['min_size']} 字节，实际 {len(f.data)}"
    return None


@check("gateway_calls")
def _gateway_calls(deps: Deps, spec: dict[str, Any]) -> str | None:
    """某个 Gateway 工具被真正执行了几次（本地 checklist_* 不走 Gateway，用 model_tools 数）。"""
    name = _need(spec, "name")
    return _compare(f"gateway.{name}", deps.gateway.count(name), spec)


@check("gateway_result")
def _gateway_result(deps: Deps, spec: dict[str, Any]) -> str | None:
    """某个工具返回的 content：ok / contains / not_contains。index 默认 -1（最后一次）。"""
    name = _need(spec, "name")
    results = deps.gateway.results_of(name)
    if not results:
        return f"{name} 一次都没被调用过，没有结果可看"
    r = results[int(spec.get("index", -1))]
    if "ok" in spec and r.ok is not bool(spec["ok"]):
        detail = r.error.message if r.error else r.content[:120]
        return f"{name} 期望 ok={spec['ok']}，实际 ok={r.ok}（{detail}）"
    if "contains" in spec and str(spec["contains"]) not in r.content:
        return f"{name} 的结果里没有 {spec['contains']!r}；实际：{r.content[:200]!r}"
    if "not_contains" in spec and str(spec["not_contains"]) in r.content:
        return f"{name} 的结果里不该出现 {spec['not_contains']!r}；实际：{r.content[:200]!r}"
    if "error_code" in spec:
        got = r.error.code if r.error else None
        if got != spec["error_code"]:
            return f"{name} 期望 error_code={spec['error_code']}，实际 {got}"
    return None


@check("model_tools")
def _model_tools(deps: Deps, spec: dict[str, Any]) -> str | None:
    """模型出过的 tool_call 名字计数（含本地 checklist_* 和 final）。"""
    name = _need(spec, "name")
    emitted = deps.model.tool_names_emitted()
    return _compare(f"模型出牌 {name}", emitted.count(name), spec)


@check("model_calls")
def _model_calls(deps: Deps, spec: dict[str, Any]) -> str | None:
    """ModelPort.chat 被调了几次 —— 08_step_limit 用它验步数上限。"""
    return _compare("ModelPort.chat 次数", deps.model.call_count, spec)


@check("sandbox_calls")
def _sandbox_calls(deps: Deps, spec: dict[str, Any]) -> str | None:
    """沙箱方法调用次数。07 用 {method: release, min: 1}。"""
    method = _need(spec, "method")
    return _compare(f"sandbox.{method}", deps.sandbox.calls.count(method), spec)


@check("store")
def _store(deps: Deps, spec: dict[str, Any]) -> str | None:
    """会话/任务的账：sessions / tasks / task_sessions（任务落在几个不同会话上）。"""
    fields = {
        "sessions": len(deps.store.sessions),
        "tasks": len(deps.store.tasks),
        "task_sessions": len(deps.store.distinct_session_ids_of_tasks),
        "seen_events": len(deps.store.seen),
    }
    checked = False
    for field, actual in fields.items():
        sub = {k[len(field) + 1 :]: v for k, v in spec.items() if k.startswith(f"{field}_")}
        if not sub:
            continue
        checked = True
        result = _compare(field, actual, sub)
        if result is not None:
            return result
    if not checked:
        raise CheckError(
            "check=store 至少要给 sessions_equals / tasks_equals / task_sessions_equals / "
            "seen_events_equals 之一（也支持 _min / _max）"
        )
    return None


@check("task")
def _task(deps: Deps, spec: dict[str, Any]) -> str | None:
    """任务终态。which: last（默认）/ first / any / all；status 是 TaskStatus 的值。"""
    tasks = deps.store.task_list
    if not tasks:
        return f"期望有任务处于 {spec.get('status')!r}，实际一个任务都没建"
    which = spec.get("which", "last")
    want = str(_need(spec, "status"))
    pool = {"last": tasks[-1:], "first": tasks[:1], "any": tasks, "all": tasks}.get(which)
    if pool is None:
        raise CheckError(f"check=task 的 which 只能是 last / first / any / all，收到 {which!r}")
    got = [str(t.status) for t in pool]
    ok = all(s == want for s in got) if which == "all" else any(s == want for s in got)
    if not ok:
        return f"任务状态（which={which}）期望 {want}，实际 {got}"
    return None


@check("evidence")
def _evidence(deps: Deps, spec: dict[str, Any]) -> str | None:
    """证据链：写了几条、链是否自洽。"""
    total = sum(len(c) for c in deps.evidence.chains.values())
    if "verified" in spec:
        bad = [tid for tid in deps.evidence.chains if not deps.evidence.verify(tid)]
        if bool(spec["verified"]) and bad:
            return f"这些任务的证据链验不过：{bad}"
    if {"equals", "min", "max"} & set(spec):
        return _compare("证据条数", total, spec)
    if "verified" not in spec:
        raise CheckError("check=evidence 至少要给 equals / min / max / verified")
    return None


# --------------------------------------------------------------------------

def run_checks(deps: Deps, expect: list[dict[str, Any]]) -> list[str]:
    """跑完全部断言，返回失败原因列表（空 = 全过）。"""
    failures: list[str] = []
    for i, spec in enumerate(expect):
        if not isinstance(spec, dict) or "check" not in spec:
            failures.append(f"expect[{i}] 不是带 check 键的 mapping：{spec!r}")
            continue
        fn = REGISTRY.get(spec["check"])
        if fn is None:
            failures.append(f"expect[{i}] 未知的 check={spec['check']!r}，可用：{sorted(REGISTRY)}")
            continue
        try:
            result = fn(deps, spec)
        except CheckError as exc:
            failures.append(f"expect[{i}] {exc}")
            continue
        if result is not None:
            failures.append(result)
    return failures
