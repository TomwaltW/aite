"""飞书开放平台 HTTP 客户端（httpx）。

为什么不用 lark_oapi 自带的 API client：它底层是同步的 `requests`
（`lark_oapi/core/http/transport.py`），而 §3.2 的 `PlatformPort` 全是 async，
且 §6 的 T1 判据要求「用 respx 断言 HTTP 方法与路径」—— respx 拦的是 httpx，
拦不到 requests。所以出站走 httpx，长连接才用 SDK（`ws.Client`，见 connection.py）。

路径不是猜的：逐条对过 SDK 里生成的请求类（它们就是官方 OpenAPI 描述文件的产物），
见每个常量上的注释。

失败面按 §3.3：
* 429 / 5xx / 传输层错误 → 退避重试 3 次（0.5s / 1s / 2s），仍失败抛 retryable=True
* 4xx（非 429）           → 不重试，抛 retryable=False
"""
from __future__ import annotations

import asyncio
import logging
import time
from collections.abc import Awaitable, Callable
from typing import Any

import httpx

from .errors import PlatformError
from .ratelimit import TokenBucket

logger = logging.getLogger(__name__)

DEFAULT_DOMAIN = "https://open.feishu.cn"

#: §3.3：429/5xx 退避重试 3 次。
RETRY_DELAYS: tuple[float, ...] = (0.5, 1.0, 2.0)

#: 各接口路径 —— 与 lark_oapi 生成的请求类逐字对齐。
PATH_TENANT_TOKEN = "/open-apis/auth/v3/tenant_access_token/internal"
PATH_MESSAGES = "/open-apis/im/v1/messages"                      # CreateMessageRequest / ListMessageRequest
PATH_MESSAGE = "/open-apis/im/v1/messages/{message_id}"          # PatchMessageRequest（PATCH）
PATH_MESSAGE_REPLY = "/open-apis/im/v1/messages/{message_id}/reply"
PATH_MESSAGE_REACTIONS = "/open-apis/im/v1/messages/{message_id}/reactions"
PATH_MESSAGE_RESOURCE = "/open-apis/im/v1/messages/{message_id}/resources/{file_key}"
PATH_FILES = "/open-apis/im/v1/files"
PATH_IMAGES = "/open-apis/im/v1/images"
PATH_DOCX_DOCUMENT = "/open-apis/docx/v1/documents/{document_id}"
PATH_DOCX_RAW_CONTENT = "/open-apis/docx/v1/documents/{document_id}/raw_content"
PATH_WIKI_NODE = "/open-apis/wiki/v2/spaces/get_node"

#: token 过期前多久就提前换新的。
_TOKEN_SAFETY_SEC = 60


class FeishuApiClient:
    """带鉴权、限速、退避重试的飞书 REST 客户端。"""

    def __init__(
        self,
        *,
        app_id: str,
        app_secret: str,
        domain: str = DEFAULT_DOMAIN,
        client: httpx.AsyncClient | None = None,
        rate_per_min: int = 60,
        retry_delays: tuple[float, ...] = RETRY_DELAYS,
        sleep: Callable[[float], Awaitable[None]] = asyncio.sleep,
        clock: Callable[[], float] = time.monotonic,
        timeout: float = 30.0,
    ) -> None:
        self.app_id = app_id
        self.app_secret = app_secret
        self.domain = domain.rstrip("/")
        self._retry_delays = retry_delays
        self._sleep = sleep
        self._clock = clock
        self._owns_client = client is None
        self._client = client or httpx.AsyncClient(timeout=timeout)
        self._bucket = TokenBucket(rate_per_min, clock=clock, sleep=sleep)
        self._token: str | None = None
        self._token_expires_at = 0.0
        self._token_lock = asyncio.Lock()

    # -- 生命周期 ---------------------------------------------------------

    async def aclose(self) -> None:
        if self._owns_client:
            await self._client.aclose()

    # -- 鉴权 -------------------------------------------------------------

    async def tenant_access_token(self) -> str:
        """拿 tenant_access_token，带过期缓存。"""
        async with self._token_lock:
            if self._token and self._clock() < self._token_expires_at:
                return self._token
            resp = await self._raw_request(
                "POST",
                PATH_TENANT_TOKEN,
                json={"app_id": self.app_id, "app_secret": self.app_secret},
                authed=False,
            )
            body = _json_body(resp)
            code = body.get("code", 0)
            if code != 0:
                # 应用凭证不对，重试多少次都是这个结果。
                raise PlatformError(code, body.get("msg", ""), retryable=False,
                                    http_status=resp.status_code)
            token = body.get("tenant_access_token") or ""
            if not token:
                raise PlatformError("no_token", "响应里没有 tenant_access_token", retryable=False)
            expire = int(body.get("expire") or 7200)
            self._token = token
            self._token_expires_at = self._clock() + max(expire - _TOKEN_SAFETY_SEC, 60)
            return token

    def invalidate_token(self) -> None:
        self._token = None
        self._token_expires_at = 0.0

    # -- 请求 -------------------------------------------------------------

    async def request(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, Any] | None = None,
        json: dict[str, Any] | None = None,
        files: dict[str, Any] | None = None,
        data: dict[str, Any] | None = None,
        rate_limited: bool = False,
        binary: bool = False,
    ) -> Any:
        """发一次请求，返回飞书响应体里的 `data`（`binary=True` 时返回原始字节）。

        `rate_limited=True` 的调用先过令牌桶 —— 出站消息类接口才受
        `outbound_rate_per_min` 约束，读接口不占这个额度。
        """
        if rate_limited:
            await self._bucket.acquire()

        resp = await self._raw_request(
            method, path, params=params, json=json, files=files, data=data
        )

        # token 过期：换一张再打一次，不算进 §3.3 的三次退避里。
        if resp.status_code == 401:
            self.invalidate_token()
            resp = await self._raw_request(
                method, path, params=params, json=json, files=files, data=data
            )

        if resp.status_code >= 400:
            raise _error_from_response(resp)

        if binary:
            return resp.content

        body = _json_body(resp)
        code = body.get("code", 0)
        if code != 0:
            # HTTP 2xx 但业务码非 0：参数/权限问题，重试没意义（§3.3 的 4xx 一档）。
            raise PlatformError(code, body.get("msg", ""), retryable=False,
                                http_status=resp.status_code)
        return body.get("data") or {}

    async def _raw_request(
        self,
        method: str,
        path: str,
        *,
        params: dict[str, Any] | None = None,
        json: dict[str, Any] | None = None,
        files: dict[str, Any] | None = None,
        data: dict[str, Any] | None = None,
        authed: bool = True,
    ) -> httpx.Response:
        """带 §3.3 退避重试的一次 HTTP 往返。4xx（非 429）直接返回给调用方判。"""
        url = self.domain + path
        attempts = len(self._retry_delays) + 1
        last_exc: Exception | None = None

        for attempt in range(attempts):
            headers = {}
            if authed:
                headers["Authorization"] = f"Bearer {await self.tenant_access_token()}"
            try:
                resp = await self._client.request(
                    method, url, params=params, json=json, files=files,
                    data=data, headers=headers,
                )
            except httpx.HTTPError as exc:
                # 传输层错误按 5xx 一档处理：连不上和对面挂了，对调用方是一回事。
                last_exc = exc
                if attempt < len(self._retry_delays):
                    await self._sleep(self._retry_delays[attempt])
                    continue
                raise PlatformError("transport_error", str(exc), retryable=True) from exc

            if resp.status_code == 429 or resp.status_code >= 500:
                if attempt < len(self._retry_delays):
                    logger.warning(
                        "feishu.retry method=%s path=%s status=%s attempt=%s",
                        method, path, resp.status_code, attempt + 1,
                    )
                    await self._sleep(self._retry_delays[attempt])
                    continue
                raise _error_from_response(resp, retryable=True)

            return resp

        raise PlatformError("unreachable", str(last_exc or ""), retryable=True)  # pragma: no cover


def _json_body(resp: httpx.Response) -> dict[str, Any]:
    try:
        body = resp.json()
    except ValueError:
        return {}
    return body if isinstance(body, dict) else {}


def _error_from_response(resp: httpx.Response, *, retryable: bool | None = None) -> PlatformError:
    """HTTP 响应 → PlatformError。retryable 默认按 §3.3 由状态码决定。"""
    body = _json_body(resp)
    code = body.get("code", resp.status_code)
    message = body.get("msg") or resp.text[:200]
    if retryable is None:
        retryable = resp.status_code == 429 or resp.status_code >= 500
    return PlatformError(code, message, retryable=retryable, http_status=resp.status_code)
