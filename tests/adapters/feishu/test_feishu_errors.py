"""失败面（§3.3 里 adapter 那几行）。

| 场景 | 期望 |
|---|---|
| 429 / 5xx | 退避重试 3 次（0.5s / 1s / 2s），仍失败抛 `PlatformError(retryable=True)` |
| 4xx（非 429） | 不重试，抛 `PlatformError(retryable=False)` |
"""
from __future__ import annotations

import httpx
import pytest
import respx

from aite.adapters.feishu import RETRY_DELAYS, FeishuApiClient, FeishuPlatform, PlatformError
from aite.contracts import OutboundText

DOMAIN = "https://open.feishu.cn"
CHAT_ID = "oc_chat_p0_demo_0001"
URL_TOKEN = f"{DOMAIN}/open-apis/auth/v3/tenant_access_token/internal"
URL_MESSAGES = f"{DOMAIN}/open-apis/im/v1/messages"

#: §3.3 写死的三个间隔。
EXPECTED_RETRY_DELAYS = [0.5, 1.0, 2.0]


def make_platform(fake_clock) -> FeishuPlatform:
    api = FeishuApiClient(
        app_id="cli", app_secret="s", domain=DOMAIN,
        sleep=fake_clock.sleep, clock=fake_clock,
    )
    return FeishuPlatform(bot_open_id="ou_bot", api=api)


def mock_token(router: respx.Router) -> None:
    router.post(URL_TOKEN).mock(
        return_value=httpx.Response(
            200, json={"code": 0, "tenant_access_token": "t-fake", "expire": 7200}
        )
    )


async def send(platform: FeishuPlatform) -> None:
    await platform.send_text(OutboundText(chat_id=CHAT_ID, text="hi"))


# ---------------------------------------------------------------------------
# 退避间隔本身
# ---------------------------------------------------------------------------

def test_retry_delays_are_frozen_by_the_spec() -> None:
    assert list(RETRY_DELAYS) == EXPECTED_RETRY_DELAYS


# ---------------------------------------------------------------------------
# 429 / 5xx：重试 3 次后 retryable=True
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("status", [429, 500, 502, 503])
async def test_retryable_status_retries_three_times_then_raises(status: int, fake_clock) -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_MESSAGES).mock(
            return_value=httpx.Response(status, json={"code": 99991400, "msg": "too fast"})
        )

        with pytest.raises(PlatformError) as excinfo:
            await send(make_platform(fake_clock))

        assert route.call_count == 4, "一次原始请求 + 三次重试"
        assert fake_clock.slept == EXPECTED_RETRY_DELAYS
        assert excinfo.value.retryable is True
        assert excinfo.value.http_status == status


async def test_retry_stops_as_soon_as_it_succeeds(fake_clock) -> None:
    """第二次就成功了就别再等剩下的 1s / 2s。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_MESSAGES).mock(
            side_effect=[
                httpx.Response(503, json={"code": 1, "msg": "busy"}),
                httpx.Response(200, json={"code": 0, "data": {"message_id": "om_ok"}}),
            ]
        )

        result = await make_platform(fake_clock).send_text(
            OutboundText(chat_id=CHAT_ID, text="hi")
        )

        assert result.message_id == "om_ok"
        assert route.call_count == 2
        assert fake_clock.slept == [0.5]


async def test_transport_error_is_retryable(fake_clock) -> None:
    """连不上和对面挂了，对调用方是一回事。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_MESSAGES).mock(side_effect=httpx.ConnectError("拔网线了"))

        with pytest.raises(PlatformError) as excinfo:
            await send(make_platform(fake_clock))

        assert route.call_count == 4
        assert fake_clock.slept == EXPECTED_RETRY_DELAYS
        assert excinfo.value.retryable is True
        assert excinfo.value.code == "transport_error"


# ---------------------------------------------------------------------------
# 4xx（非 429）：不重试，retryable=False
# ---------------------------------------------------------------------------

@pytest.mark.parametrize("status", [400, 403, 404, 422])
async def test_client_error_is_not_retried(status: int, fake_clock) -> None:
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_MESSAGES).mock(
            return_value=httpx.Response(status, json={"code": 230001, "msg": "invalid params"})
        )

        with pytest.raises(PlatformError) as excinfo:
            await send(make_platform(fake_clock))

        assert route.call_count == 1, "4xx 重试多少次都是这个结果"
        assert fake_clock.slept == [], "不该有任何退避等待"
        assert excinfo.value.retryable is False
        assert excinfo.value.code == 230001
        assert excinfo.value.http_status == status


async def test_business_error_on_http_200_is_not_retryable(fake_clock) -> None:
    """飞书常在 HTTP 200 里回业务错误码；那也是参数/权限问题，重试没意义。"""
    async with respx.mock(assert_all_called=False) as router:
        mock_token(router)
        route = router.post(URL_MESSAGES).mock(
            return_value=httpx.Response(200, json={"code": 230002, "msg": "bot not in chat"})
        )

        with pytest.raises(PlatformError) as excinfo:
            await send(make_platform(fake_clock))

        assert route.call_count == 1
        assert excinfo.value.retryable is False
        assert excinfo.value.code == 230002
        assert "bot not in chat" in str(excinfo.value)


async def test_bad_credentials_do_not_retry(fake_clock) -> None:
    """应用凭证不对，重试 3 次也还是不对。"""
    async with respx.mock(assert_all_called=False) as router:
        router.post(URL_TOKEN).mock(
            return_value=httpx.Response(200, json={"code": 10003, "msg": "invalid app_secret"})
        )

        with pytest.raises(PlatformError) as excinfo:
            await send(make_platform(fake_clock))

        assert excinfo.value.retryable is False
        assert excinfo.value.code == 10003


# ---------------------------------------------------------------------------
# token 过期
# ---------------------------------------------------------------------------

async def test_expired_token_is_refreshed_once(fake_clock) -> None:
    """401 换一张 token 再打一次；这一次不吃 §3.3 的三次退避额度。"""
    async with respx.mock(assert_all_called=False) as router:
        token_route = router.post(URL_TOKEN).mock(
            side_effect=[
                httpx.Response(200, json={"code": 0, "tenant_access_token": "t-old", "expire": 7200}),
                httpx.Response(200, json={"code": 0, "tenant_access_token": "t-new", "expire": 7200}),
            ]
        )
        send_route = router.post(URL_MESSAGES).mock(
            side_effect=[
                httpx.Response(401, json={"code": 99991663, "msg": "token expired"}),
                httpx.Response(200, json={"code": 0, "data": {"message_id": "om_ok"}}),
            ]
        )

        result = await make_platform(fake_clock).send_text(
            OutboundText(chat_id=CHAT_ID, text="hi")
        )

        assert result.message_id == "om_ok"
        assert token_route.call_count == 2
        assert send_route.call_count == 2
        assert send_route.calls[1].request.headers["Authorization"] == "Bearer t-new"
        assert fake_clock.slept == []


async def test_token_is_cached_across_calls(fake_clock) -> None:
    async with respx.mock(assert_all_called=False) as router:
        token_route = router.post(URL_TOKEN).mock(
            return_value=httpx.Response(
                200, json={"code": 0, "tenant_access_token": "t-fake", "expire": 7200}
            )
        )
        router.post(URL_MESSAGES).mock(
            return_value=httpx.Response(200, json={"code": 0, "data": {"message_id": "om"}})
        )

        platform = make_platform(fake_clock)
        for _ in range(5):
            await send(platform)

        assert token_route.call_count == 1


# ---------------------------------------------------------------------------
# PlatformError 本身
# ---------------------------------------------------------------------------

def test_platform_error_signature_matches_the_spec() -> None:
    """§3.3 写的就是 `PlatformError(code, retryable=True)`。"""
    error = PlatformError("bad", retryable=True)
    assert error.code == "bad"
    assert error.retryable is True
    assert error.message == ""
    assert isinstance(error, Exception)
