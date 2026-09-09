"""按 `ToolSpec.parameters` 校验 arguments（owner: T3）。

§3.2 的调用顺序里第三步是「按 ToolSpec.parameters 校验 arguments」。`parameters`
是 JSON Schema，但 §3.1 冻结的那 5 个 `GATEWAY_TOOLS` 只用到了它极小的一个子集：
`type` / `properties` / `required` / `default` / `minimum` / `maximum` /
`minItems` / `maxItems` / `items` / `enum`。

所以这里手写这个子集，不引 jsonschema —— §3.0 的依赖是冻结的，加一条要停下来报告，
而为了校验五个两三个字段的 schema 去动冻结文件不值得。支持的关键字之外的一律**忽略**
（不静默放过整个字段，只是不额外约束），这样以后 spec 往 schema 里加关键字时，
最坏情况是校验偏松，不会变成「认不出就整个拒掉」。

除 spec 写的之外多了一条本层策略：**不认识的参数直接拒**。JSON Schema 的默认语义是
允许额外字段，但这是给模型用的工具入口，模型把参数名写错时早点收到 invalid_args
比静默丢掉那个参数好。
"""
from typing import Any

__all__ = ["SchemaViolation", "validate_arguments"]


class SchemaViolation(ValueError):
    """arguments 不合 schema。Gateway 把它翻成 ToolErrorCode.invalid_args。"""


_TYPE_PREDICATES: dict[str, Any] = {
    # bool 是 int 的子类，必须显式排掉，否则 True 会被当成合法的 integer。
    "boolean": lambda v: isinstance(v, bool),
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, int | float) and not isinstance(v, bool),
    "string": lambda v: isinstance(v, str),
    "array": lambda v: isinstance(v, list),
    "object": lambda v: isinstance(v, dict),
    "null": lambda v: v is None,
}


def validate_arguments(schema: dict[str, Any], arguments: dict[str, Any]) -> dict[str, Any]:
    """校验并补默认值，返回可以直接喂给工具实现的 arguments。

    只做校验和填 `default`，不做类型转换 —— 字符串 "50" 不会被悄悄变成整数 50，
    模型给错类型就该拿到 invalid_args。
    """
    if not isinstance(arguments, dict):
        raise SchemaViolation(f"arguments 必须是对象，收到 {type(arguments).__name__}")

    declared_type = schema.get("type", "object")
    if declared_type != "object":
        raise SchemaViolation(f"工具的 parameters 顶层必须是 object，schema 写的是 {declared_type!r}")

    properties: dict[str, Any] = schema.get("properties") or {}
    required: list[str] = list(schema.get("required") or [])

    unknown = sorted(set(arguments) - set(properties))
    if unknown:
        known = sorted(properties) or ["（这个工具不接受任何参数）"]
        raise SchemaViolation(f"不认识的参数 {unknown}；可用的是 {known}")

    missing = [name for name in required if name not in arguments]
    if missing:
        raise SchemaViolation(f"缺少必填参数 {missing}")

    out: dict[str, Any] = {}
    for name, subschema in properties.items():
        if name in arguments:
            out[name] = _check(arguments[name], subschema, name)
        elif "default" in subschema:
            out[name] = subschema["default"]
    return out


def _check(value: Any, subschema: dict[str, Any], path: str) -> Any:
    if not isinstance(subschema, dict):        # schema 本身坏了，不是调用方的错
        raise SchemaViolation(f"{path} 的 schema 不是对象：{subschema!r}")

    expected = subschema.get("type")
    types = [expected] if isinstance(expected, str) else list(expected or [])
    if types:
        if any(t not in _TYPE_PREDICATES for t in types):
            raise SchemaViolation(f"{path} 的 schema 用了不支持的 type：{expected!r}")
        if not any(_TYPE_PREDICATES[t](value) for t in types):
            raise SchemaViolation(f"{path} 应该是 {'/'.join(types)}，收到 {_name_of(value)}")

    if "enum" in subschema and value not in subschema["enum"]:
        raise SchemaViolation(f"{path} 只能是 {subschema['enum']} 之一，收到 {value!r}")

    if isinstance(value, str):
        _check_string(value, subschema, path)
    if isinstance(value, int | float) and not isinstance(value, bool):
        _check_number(value, subschema, path)
    if isinstance(value, list):
        return _check_array(value, subschema, path)
    if isinstance(value, dict):
        return _check_object(value, subschema, path)
    return value


def _check_string(value: str, subschema: dict[str, Any], path: str) -> None:
    low = subschema.get("minLength")
    high = subschema.get("maxLength")
    if low is not None and len(value) < low:
        raise SchemaViolation(f"{path} 至少要 {low} 个字符，收到 {len(value)} 个")
    if high is not None and len(value) > high:
        raise SchemaViolation(f"{path} 最多 {high} 个字符，收到 {len(value)} 个")


def _check_number(value: float, subschema: dict[str, Any], path: str) -> None:
    checks = (
        ("minimum", lambda lo: value < lo, "不能小于"),
        ("maximum", lambda hi: value > hi, "不能大于"),
        ("exclusiveMinimum", lambda lo: value <= lo, "必须大于"),
        ("exclusiveMaximum", lambda hi: value >= hi, "必须小于"),
    )
    for key, violated, phrase in checks:
        bound = subschema.get(key)
        if bound is not None and violated(bound):
            raise SchemaViolation(f"{path} {phrase} {bound}，收到 {value}")


def _check_array(value: list[Any], subschema: dict[str, Any], path: str) -> list[Any]:
    low = subschema.get("minItems")
    high = subschema.get("maxItems")
    if low is not None and len(value) < low:
        raise SchemaViolation(f"{path} 至少要 {low} 项，收到 {len(value)} 项")
    if high is not None and len(value) > high:
        raise SchemaViolation(f"{path} 最多 {high} 项，收到 {len(value)} 项")
    items = subschema.get("items")
    if not isinstance(items, dict):
        return value
    return [_check(item, items, f"{path}[{i}]") for i, item in enumerate(value)]


def _check_object(value: dict[str, Any], subschema: dict[str, Any], path: str) -> dict[str, Any]:
    properties = subschema.get("properties")
    if not isinstance(properties, dict):
        return value
    missing = [n for n in (subschema.get("required") or []) if n not in value]
    if missing:
        raise SchemaViolation(f"{path} 缺少必填字段 {missing}")
    out = dict(value)
    for name, sub in properties.items():
        if name in value:
            out[name] = _check(value[name], sub, f"{path}.{name}")
        elif "default" in sub:
            out[name] = sub["default"]
    return out


def _name_of(value: Any) -> str:
    return "null" if value is None else f"{type(value).__name__}({value!r})"[:80]
