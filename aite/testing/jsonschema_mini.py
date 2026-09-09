"""够用就好的 JSON Schema 子集校验器。

只覆盖 §3.1 冻结的那几份 `ToolSpec.parameters` 实际用到的关键字：
type / properties / required / items / minItems / maxItems / minimum / maximum / enum，
外加把 `default` 填进结果。**不引第三方依赖**（§3.0 冻结了依赖表，加一条就得停下来报告）。

返回值是「补齐默认值之后的 arguments」，出错则抛 SchemaError，消息直接可以塞进
ToolResult 的 error.message 给模型看。
"""
from __future__ import annotations

from typing import Any

_TYPE_CHECKS: dict[str, Any] = {
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
    "string": lambda v: isinstance(v, str),
    "boolean": lambda v: isinstance(v, bool),
    # bool 是 int 的子类，整数/数字校验必须先把它排掉
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, int | float) and not isinstance(v, bool),
    "null": lambda v: v is None,
}


class SchemaError(ValueError):
    """参数不合 schema。消息带路径，模型看得懂改哪儿。"""


def validate(value: Any, schema: dict[str, Any], *, path: str = "arguments") -> Any:
    """按 schema 校验并返回补过默认值的副本。"""
    expected = schema.get("type")
    if expected is not None:
        check = _TYPE_CHECKS.get(expected)
        if check is None:
            raise SchemaError(f"{path}: schema 用了不支持的 type={expected!r}")
        if not check(value):
            got = type(value).__name__
            raise SchemaError(f"{path}: 期望 {expected}，实际 {got}（{value!r}）")

    if "enum" in schema and value not in schema["enum"]:
        raise SchemaError(f"{path}: 必须是 {schema['enum']} 之一，实际 {value!r}")

    if expected == "object":
        return _validate_object(value, schema, path)
    if expected == "array":
        return _validate_array(value, schema, path)
    if expected in ("integer", "number"):
        if "minimum" in schema and value < schema["minimum"]:
            raise SchemaError(f"{path}: 不能小于 {schema['minimum']}，实际 {value}")
        if "maximum" in schema and value > schema["maximum"]:
            raise SchemaError(f"{path}: 不能大于 {schema['maximum']}，实际 {value}")
    return value


def _validate_object(value: dict[str, Any], schema: dict[str, Any], path: str) -> dict[str, Any]:
    props: dict[str, Any] = schema.get("properties", {})
    required: list[str] = schema.get("required", [])
    missing = [k for k in required if k not in value]
    if missing:
        raise SchemaError(f"{path}: 缺少必填参数 {missing}（收到 {sorted(value)}）")

    out: dict[str, Any] = {}
    for key, item in value.items():
        sub = props.get(key)
        out[key] = item if sub is None else validate(item, sub, path=f"{path}.{key}")
    for key, sub in props.items():
        if key not in out and "default" in sub:
            out[key] = sub["default"]
    return out


def _validate_array(value: list[Any], schema: dict[str, Any], path: str) -> list[Any]:
    if "minItems" in schema and len(value) < schema["minItems"]:
        raise SchemaError(f"{path}: 至少 {schema['minItems']} 项，实际 {len(value)} 项")
    if "maxItems" in schema and len(value) > schema["maxItems"]:
        raise SchemaError(f"{path}: 最多 {schema['maxItems']} 项，实际 {len(value)} 项")
    sub = schema.get("items")
    if sub is None:
        return list(value)
    return [validate(v, sub, path=f"{path}[{i}]") for i, v in enumerate(value)]
