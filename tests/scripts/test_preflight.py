"""`scripts/preflight.py` 的自测。

钉的是派单点名的四条，外加几条同样值钱的：

* 全通 → 退出码 0
* 某项缺失 → 退出码 1，且 FAIL 行里点名了缺的那个东西
* `--offline` 不碰网络也不碰 docker
* **密钥取值不出现在任何输出里**（拿假密钥跑一遍，断言它没进 stdout/stderr）
* 第 6 组的容器一定被收掉 —— 连探针炸掉的异常路径也收
* 任何一项自己炸掉都不阻断后面的检查
* `--json` 每项都有 name / status / detail

外部依赖全是替身（见本目录 conftest）：不连网、不起容器，所以这些用例不需要 `-m docker`。
"""
from __future__ import annotations

import json

import httpx
import pytest
import respx
from docker.errors import DockerException


def token_url(pf) -> str:
    return f"{pf.DEFAULT_DOMAIN}/open-apis/auth/v3/tenant_access_token/internal"


def bot_info_url(pf) -> str:
    return f"{pf.DEFAULT_DOMAIN}{pf.PATH_BOT_INFO}"


def history_url(pf) -> str:
    return f"{pf.DEFAULT_DOMAIN}/open-apis/im/v1/messages"


def mock_feishu_ok(pf, secrets, *, open_id: str | None = None) -> None:
    respx.post(token_url(pf)).mock(
        return_value=httpx.Response(
            200,
            json={"code": 0, "msg": "ok", "tenant_access_token": secrets.token, "expire": 7200},
        )
    )
    respx.get(bot_info_url(pf)).mock(
        return_value=httpx.Response(
            200,
            json={
                "code": 0,
                "msg": "ok",
                "bot": {
                    "open_id": open_id or secrets.bot_open_id,
                    "app_name": "Aite",
                    "activate_status": 1,
                },
            },
        )
    )


def rows(text: str) -> list[str]:
    return [line for line in text.splitlines() if line.startswith("[")]


def row_for(text: str, title: str) -> str:
    hit = [line for line in rows(text) if title in line]
    assert hit, f"输出里没有「{title}」这一行：\n{text}"
    return hit[0]


ALL_TITLES = (
    "配置可加载", "环境变量齐", "飞书凭证有效", "飞书身份对得上",
    "模型端点通", "沙箱可用", "落盘目录可写",
)


# ---------------------------------------------------------------------------
# 全通 / 缺项
# ---------------------------------------------------------------------------

@respx.mock
def test_all_green_exits_zero(pf, secrets, run_preflight, write_config, tmp_path, capsys):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg)])
    out = capsys.readouterr().out

    assert result.code == 0, out
    assert "过了 7 项" in out and "FAIL 0" in out
    for title in ALL_TITLES:
        assert "OK" in row_for(out, title), out
    assert result.model.completions.calls, "第 5 组没真发出去那次最小 chat"
    assert result.docker.created, "第 6 组没真起容器"


@respx.mock
def test_missing_env_var_fails_and_names_it(
    pf, secrets, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)
    env = {k: v for k, v in secrets.env.items() if k != "FEISHU_BOT_OPEN_ID"}

    result = run_preflight(["--config", str(cfg)], env=env)
    out = capsys.readouterr().out

    assert result.code == 1, out
    env_row = row_for(out, "环境变量齐")
    # 「未设置」那一半只能有它一个，否则「点名了缺的东西」这条断言等于没断言。
    assert "FAIL" in env_row, env_row
    assert "1/4 个未设置：FEISHU_BOT_OPEN_ID" in env_row, env_row
    assert "已设置：FEISHU_APP_ID FEISHU_APP_SECRET AITE_MODEL_API_KEY" in env_row, env_row
    # 缺 open_id 时第 4 组也该红：M1 静默不响应就是这么来的。
    assert "FAIL" in row_for(out, "飞书身份对得上")


@respx.mock
def test_missing_feishu_creds_block_checks_three_and_four(
    pf, run_preflight, write_config, tmp_path, capsys
):
    """本机（没凭证）跑出来的就是这个分支 —— FAIL 且说清是前置没满足。"""
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg)], env={})
    out = capsys.readouterr().out

    assert result.code == 1
    assert respx.calls.call_count == 0, "凭证都没有还去发了请求"
    for title in ("飞书凭证有效", "飞书身份对得上"):
        row = row_for(out, title)
        assert "FAIL" in row and "前置未满足" in row and "FEISHU_APP_ID" in row, row
    assert "FAIL" in row_for(out, "模型端点通")
    # 不依赖凭证的两组照样得跑出结论。
    assert "OK" in row_for(out, "沙箱可用")
    assert "OK" in row_for(out, "落盘目录可写")


@respx.mock
def test_missing_image_names_the_build_command(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path, sandbox={"image": "aite-sandbox:nope"})

    result = run_preflight(["--config", str(cfg)], docker=fakes.Docker(images=()))
    out = capsys.readouterr().out

    assert result.code == 1
    assert "aite-sandbox:nope" in row_for(out, "沙箱可用")
    assert "docker build -t aite-sandbox:nope docker/sandbox" in out, out


@respx.mock
def test_bot_open_id_mismatch_is_fail_without_printing_values(
    pf, secrets, run_preflight, write_config, tmp_path, capsys
):
    other = "ou_SOMEOTHERBOT99999999999999999999"
    mock_feishu_ok(pf, secrets, open_id=other)
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg)])
    captured = capsys.readouterr()
    blob = captured.out + captured.err

    assert result.code == 1
    identity = row_for(captured.out, "飞书身份对得上")
    assert "FAIL" in identity and "对不上" in identity
    # 对不上的时候最想打出来的就是那两个取值 —— 正因如此这里要钉死：一个都不许打。
    assert secrets.bot_open_id not in blob
    assert other not in blob


# ---------------------------------------------------------------------------
# --offline
# ---------------------------------------------------------------------------

@respx.mock
def test_offline_touches_neither_network_nor_docker(
    run_preflight, write_config, tmp_path, capsys
):
    """respx 一条路由都没注册：真发 HTTP 就会炸。docker 工厂被叫到会记一笔。"""
    cfg = write_config(tmp_path)
    touched: list[str] = []

    def docker_factory():
        touched.append("docker")
        raise DockerException("不该被叫到")

    result = run_preflight(["--config", str(cfg), "--offline"], docker=docker_factory)
    out = capsys.readouterr().out

    assert result.code == 0, out
    assert touched == [], "--offline 下碰了 docker"
    assert respx.calls.call_count == 0, "--offline 下发了 HTTP 请求"
    for title in ("飞书凭证有效", "飞书身份对得上", "模型端点通", "沙箱可用"):
        assert "SKIP" in row_for(out, title), out
    for title in ("配置可加载", "环境变量齐", "落盘目录可写"):
        assert "SKIP" not in row_for(out, title), out


@respx.mock
def test_offline_with_no_credentials_still_exits_zero(
    secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    """CI 和没凭证的机器上要能跑 —— 缺变量降成 WARN，但一个名字都不少报。"""
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg), "--offline"], env={}, docker=fakes.boom_docker)
    out = capsys.readouterr().out

    assert result.code == 0, out
    env_row = row_for(out, "环境变量齐")
    assert "WARN" in env_row
    for name in secrets.env:
        assert name in env_row, env_row


# ---------------------------------------------------------------------------
# 红线：密钥取值不外泄
# ---------------------------------------------------------------------------

@respx.mock
@pytest.mark.parametrize("argv_extra", [[], ["--json"]])
def test_secret_values_never_reach_output(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys, argv_extra
):
    """拿假密钥跑一遍，断言它没出现在 stdout/stderr 里。

    这里故意让上游把凭证**回显进错误消息**（真实网关偶尔就这么干），
    验的不只是「代码里没去打它」，而是 Redactor 那道兜底闸真的在工作。
    """
    respx.post(token_url(pf)).mock(
        return_value=httpx.Response(
            200,
            json={"code": 0, "msg": "ok", "tenant_access_token": secrets.token, "expire": 7200},
        )
    )
    respx.get(bot_info_url(pf)).mock(
        return_value=httpx.Response(
            200,
            json={
                "code": 99991663,
                "msg": f"bad token {secrets.token} for secret {secrets.app_secret}",
            },
        )
    )
    cfg = write_config(tmp_path)
    model = fakes.ModelClient(error=RuntimeError(f"401 invalid api key {secrets.model_key}"))

    result = run_preflight(["--config", str(cfg), *argv_extra], model=model)
    captured = capsys.readouterr()
    blob = captured.out + captured.err

    assert result.code == 1
    for secret in secrets.all:
        assert secret not in blob, f"{secret[:6]}… 漏进输出了"
    # 反过来钉一条：确实是被抹掉的，而不是这几段文本压根没走到输出。
    assert "的取值已隐去" in blob, blob


def test_redactor_scrubs_nested_extra(pf):
    """`extra` 是嵌套结构，脱敏要递归下去 —— JSON 转义会让字符串替换漏掉。"""
    redactor = pf.Redactor()
    secret = 'se"cret\\value'
    redactor.add(secret, "AITE_MODEL_API_KEY")

    scrubbed = redactor.scrub_obj({"a": [secret], "b": {"c": f"x {secret} y"}})

    assert secret not in json.dumps(scrubbed, ensure_ascii=False)
    assert "的取值已隐去" in scrubbed["a"][0]


def test_redactor_leaves_short_values_alone(pf):
    """一两个字符的「密钥」不做全局替换，否则正常输出会被打成马赛克。"""
    redactor = pf.Redactor()
    redactor.add("ab", "SHORT")

    assert redactor.scrub("about") == "about"


# ---------------------------------------------------------------------------
# 第 6 组：容器一定被收掉
# ---------------------------------------------------------------------------

@respx.mock
def test_sandbox_releases_container_on_success(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)
    docker = fakes.Docker()

    result = run_preflight(["--config", str(cfg)], docker=docker)
    out = capsys.readouterr().out

    assert result.code == 0, out
    assert docker.created, "没起容器"
    assert docker.live == {}, f"跑完还剩容器：{list(docker.live)}"
    assert any(ev[0] == "remove" for ev in docker.events)
    assert docker.closed, "docker client 没关"
    assert "容器已收干净" in row_for(out, "沙箱可用")


@respx.mock
def test_sandbox_releases_container_when_probe_blows_up(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    """异常路径也要收 —— 探针炸掉时容器照样得没。"""
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)

    def exploding_probe():
        raise DockerException("exec 挂了")

    docker = fakes.Docker(probe=exploding_probe)
    result = run_preflight(["--config", str(cfg)], docker=docker)
    out = capsys.readouterr().out

    assert result.code == 1
    assert "FAIL" in row_for(out, "沙箱可用")
    assert docker.created, "没起容器，这条用例就没验到收尾"
    assert docker.live == {}, f"探针炸了以后还剩容器：{list(docker.live)}"
    # 后面那组照样要跑到。
    assert "OK" in row_for(out, "落盘目录可写")


@respx.mock
def test_sandbox_fails_when_imports_missing(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)
    docker = fakes.Docker(
        probe=lambda: fakes.ExecResult(1, b"", b"ModuleNotFoundError: No module named 'docx'")
    )

    result = run_preflight(["--config", str(cfg)], docker=docker)
    out = capsys.readouterr().out

    assert result.code == 1
    sandbox_row = row_for(out, "沙箱可用")
    assert "FAIL" in sandbox_row and "docx" in sandbox_row
    assert "docker build" in out
    assert docker.live == {}, "import 没跑通也得把容器收掉"


@respx.mock
def test_docker_daemon_down_is_one_fail_row_not_a_crash(
    pf, secrets, fakes, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg)], docker=fakes.boom_docker)
    out = capsys.readouterr().out

    assert result.code == 1
    sandbox_row = row_for(out, "沙箱可用")
    assert "FAIL" in sandbox_row and "Docker daemon" in sandbox_row
    assert "Docker Desktop" in out, "没给出怎么补"
    # 第 7 组在它后面，必须照样跑到 —— 一项失败不阻断后面的检查。
    assert "OK" in row_for(out, "落盘目录可写")


# ---------------------------------------------------------------------------
# 一项失败不阻断后面的检查
# ---------------------------------------------------------------------------

@respx.mock
def test_every_check_still_runs_when_feishu_is_unreachable(
    pf, run_preflight, write_config, tmp_path, capsys
):
    respx.post(token_url(pf)).mock(side_effect=httpx.ConnectError("网络不通"))
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg)])
    out = capsys.readouterr().out

    assert result.code == 1
    assert "FAIL" in row_for(out, "飞书凭证有效")
    assert "SKIP" in row_for(out, "飞书身份对得上")
    # 后面三组一个都不能少跑。
    assert "OK" in row_for(out, "模型端点通")
    assert "OK" in row_for(out, "沙箱可用")
    assert "OK" in row_for(out, "落盘目录可写")
    assert result.model.completions.calls and result.docker.created


def test_unexpected_crash_becomes_one_fail_row(
    pf, fakes, run_preflight, write_config, tmp_path, capsys, monkeypatch
):
    """预料外的异常也只红一行，不许把整轮掀翻。"""
    cfg = write_config(tmp_path)

    def exploding_storage(_cfg):
        raise ZeroDivisionError("谁也没想到")

    monkeypatch.setattr(pf, "check_storage", exploding_storage)
    result = run_preflight(["--config", str(cfg), "--offline"], docker=fakes.boom_docker)
    out = capsys.readouterr().out

    assert result.code == 1
    storage_row = row_for(out, "落盘目录可写")
    assert "FAIL" in storage_row and "ZeroDivisionError" in storage_row
    assert "汇总：" in out, "汇总还是要打出来"


# ---------------------------------------------------------------------------
# 配置 / 落盘 / --json / --chat-id / --help
# ---------------------------------------------------------------------------

def test_bad_config_fails_but_still_reports_seven_rows(
    fakes, run_preflight, tmp_path, capsys
):
    bad = tmp_path / "aite.yaml"
    bad.write_text("platform: 不存在的平台\n", encoding="utf-8")

    result = run_preflight(["--config", str(bad), "--offline"], docker=fakes.boom_docker)
    out = capsys.readouterr().out

    assert result.code == 1
    assert "FAIL" in row_for(out, "配置可加载")
    assert len(rows(out)) == 7, out


def test_storage_fails_when_ancestor_not_writable(
    fakes, run_preflight, write_config, tmp_path, capsys
):
    locked = tmp_path / "locked"
    locked.mkdir()
    locked.chmod(0o500)
    try:
        cfg = write_config(
            tmp_path,
            storage={
                "sqlite_path": str(locked / "data" / "aite.db"),
                "evidence_dir": str(locked / "data" / "evidence"),
                "artifacts_dir": str(locked / "data" / "artifacts"),
            },
        )
        result = run_preflight(["--config", str(cfg), "--offline"], docker=fakes.boom_docker)
        out = capsys.readouterr().out
    finally:
        locked.chmod(0o700)

    assert result.code == 1
    assert "FAIL" in row_for(out, "落盘目录可写")


def test_storage_ok_when_parent_missing_but_creatable(
    fakes, run_preflight, write_config, tmp_path, capsys
):
    """判据是「落得下去」不是「已经在」：EvidenceWriter / store 都会自建目录。"""
    cfg = write_config(tmp_path)
    assert not (tmp_path / "data").exists()

    result = run_preflight(["--config", str(cfg), "--offline"], docker=fakes.boom_docker)
    out = capsys.readouterr().out

    assert result.code == 0
    storage_row = row_for(out, "落盘目录可写")
    assert "OK" in storage_row and "待建" in storage_row


@respx.mock
def test_json_output_shape(pf, secrets, run_preflight, write_config, tmp_path, capsys):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg), "--json"])
    payload = json.loads(capsys.readouterr().out)

    assert result.code == 0
    assert payload["ok"] is True and payload["total"] == 7
    assert [c["name"] for c in payload["checks"]] == [
        "config", "env", "feishu_token", "feishu_identity", "model", "sandbox", "storage"
    ]
    for check in payload["checks"]:
        assert {"name", "status", "detail"} <= set(check)
        assert check["status"] in {"ok", "fail", "warn", "skip"}
    # §3.7 那两条待核实项要在，且如实标成没核实。
    assert {n["name"] for n in payload["notes"]} == {
        "feishu_passive_listen", "feishu_group_history_scope"
    }
    assert all(n["status"] in {"unverified", "unverifiable"} for n in payload["notes"])


@respx.mock
def test_json_carries_model_and_sandbox_evidence(
    pf, secrets, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    cfg = write_config(tmp_path)

    run_preflight(["--config", str(cfg), "--json"])
    payload = json.loads(capsys.readouterr().out)
    by_name = {c["name"]: c for c in payload["checks"]}

    model_extra = by_name["model"]["extra"]
    # 0.8 元/Mtok * 7 + 2.0 * 2 = 9.6e-6
    assert model_extra["input_tokens"] == 7 and model_extra["output_tokens"] == 2
    assert model_extra["cost_cny"] == pytest.approx(9.6e-6)
    sandbox_extra = by_name["sandbox"]["extra"]
    assert sandbox_extra["packages"]["pandas"] == "2.2.3"
    assert sandbox_extra["leftover_containers"] == []


@respx.mock
def test_chat_id_probes_group_history(
    pf, secrets, run_preflight, write_config, tmp_path, capsys
):
    """给了 --chat-id 就实测一次群历史，把 §3.7(b) 从「未核实」变成有结论。"""
    mock_feishu_ok(pf, secrets)
    respx.get(history_url(pf)).mock(
        return_value=httpx.Response(
            200, json={"code": 0, "msg": "ok", "data": {"items": [{"message_id": "om_1"}]}}
        )
    )
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg), "--json", "--chat-id", "oc_test"])
    payload = json.loads(capsys.readouterr().out)

    assert result.code == 0
    note = next(n for n in payload["notes"] if n["name"] == "feishu_group_history_scope")
    assert note["status"] == "verified" and "读得到" in note["detail"]


@respx.mock
def test_chat_id_probe_reports_permission_failure(
    pf, secrets, run_preflight, write_config, tmp_path, capsys
):
    mock_feishu_ok(pf, secrets)
    respx.get(history_url(pf)).mock(
        return_value=httpx.Response(200, json={"code": 99991672, "msg": "permission denied"})
    )
    cfg = write_config(tmp_path)

    result = run_preflight(["--config", str(cfg), "--json", "--chat-id", "oc_test"])
    payload = json.loads(capsys.readouterr().out)

    # 探测结果只是提示行，不改判据 —— 七组都过了就还是 0。
    assert result.code == 0
    note = next(n for n in payload["notes"] if n["name"] == "feishu_group_history_scope")
    assert note["status"] == "verified" and "读**不到**" in note["detail"]


def test_env_var_names_covers_every_env_field(pf):
    cfg = pf.load_config(pf._resolve(pf.EXAMPLE_CONFIG_PATH))

    assert dict(pf.env_var_names(cfg)) == {
        "feishu.app_id_env": "FEISHU_APP_ID",
        "feishu.app_secret_env": "FEISHU_APP_SECRET",
        "feishu.bot_open_id_env": "FEISHU_BOT_OPEN_ID",
        "model.api_key_env": "AITE_MODEL_API_KEY",
    }


def test_help_exits_zero(pf, capsys):
    with pytest.raises(SystemExit) as excinfo:
        pf.main(["--help"])

    assert excinfo.value.code == 0
    assert "起飞前自检" in capsys.readouterr().out
