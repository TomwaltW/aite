"""ToolGateway 的实现骨架（owner: T3，契约见 aite/contracts/ports.py §3.2）。"""
from ..contracts import ToolCallRequest, ToolContext, ToolResult, ToolSpec


class P0ToolGateway:
    """ToolGateway（T3）。P0 的 catalog 就是 GATEWAY_TOOLS 原样。"""

    def catalog(self, ctx: ToolContext) -> list[ToolSpec]:
        raise NotImplementedError("T3")

    async def call(self, ctx: ToolContext, req: ToolCallRequest) -> ToolResult:
        raise NotImplementedError("T3")
