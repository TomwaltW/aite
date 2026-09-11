//! `validate_arguments` 的单元测试（对应旧 `tests/gateway/test_gateway_schema.py`，17 条）。
//!
//! §3.2 的第三步是「按 ToolSpec.parameters 校验 arguments」。这一份直接拿 §3.1 冻结的
//! 那 5 个 schema 当输入，别自己编 schema —— 编的话测的是校验器，不是契约。
//! 旧版的参数化用例在这里合成一条循环，覆盖面逐个对齐。
use aite_contracts::gateway_tools;
use aite_gateway::{SchemaViolation, validate_arguments};
use serde_json::{Map, Value, json};

fn schema_of(tool: &str) -> Value {
    gateway_tools()
        .iter()
        .find(|t| t.name == tool)
        .map(|t| t.parameters.clone())
        .unwrap_or_else(|| panic!("没有这个工具：{tool}"))
}

fn as_map(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("arguments 必须是对象：{other}"),
    }
}

fn check(tool: &str, args: Value) -> Result<Map<String, Value>, SchemaViolation> {
    validate_arguments(&schema_of(tool), &as_map(args))
}

fn expect_ok(tool: &str, args: Value) -> Map<String, Value> {
    check(tool, args.clone()).unwrap_or_else(|e| panic!("{tool} {args} 应该过：{e}"))
}

fn expect_err(tool: &str, args: Value) -> String {
    match check(tool, args.clone()) {
        Err(e) => e.to_string(),
        Ok(out) => panic!("{tool} {args} 应该被拒，实际过了：{out:?}"),
    }
}

// --- 填默认值 ---------------------------------------------------------------

#[test]
fn defaults_come_from_the_schema() {
    // §3.1：limit 默认 50、thread_only 默认 false。
    assert_eq!(
        expect_ok("read_group_history", json!({})),
        as_map(json!({"limit": 50, "thread_only": false}))
    );
}

#[test]
fn run_python_default_timeout_is_120() {
    assert_eq!(
        expect_ok("run_python", json!({"code": "print(1)"})),
        as_map(json!({"code": "print(1)", "timeout_sec": 120}))
    );
}

#[test]
fn list_files_takes_no_arguments() {
    assert_eq!(expect_ok("list_files", json!({})), Map::new());
}

#[test]
fn given_values_win_over_defaults() {
    assert_eq!(
        expect_ok("read_group_history", json!({"limit": 3})),
        as_map(json!({"limit": 3, "thread_only": false}))
    );
}

// --- 必填 -------------------------------------------------------------------

#[test]
fn missing_required_is_rejected() {
    let cases = [
        ("read_document", json!({})),
        ("download_attachment", json!({})),
        ("run_python", json!({})),
        ("run_python", json!({"timeout_sec": 10})),
    ];
    for (tool, args) in cases {
        let message = expect_err(tool, args);
        assert!(message.contains("必填"), "{message}");
    }
}

// --- 类型 -------------------------------------------------------------------

#[test]
fn wrong_types_are_rejected() {
    let cases = [
        ("read_group_history", json!({"limit": "50"})), // 不做隐式转换
        ("read_group_history", json!({"limit": 12.5})),
        ("read_group_history", json!({"limit": null})),
        ("read_group_history", json!({"thread_only": "true"})),
        ("read_document", json!({"url_or_token": 42})),
        ("run_python", json!({"code": ["print(1)"]})),
        ("run_python", json!({"code": "x", "timeout_sec": "30"})),
    ];
    for (tool, args) in cases {
        expect_err(tool, args);
    }
}

#[test]
fn bool_is_not_an_integer() {
    // JSON 里 true 不是数字，这条靠类型系统天然成立，但它是旧实现踩过的坑，照样钉住。
    expect_err("read_group_history", json!({"limit": true}));
}

#[test]
fn integer_is_not_a_boolean() {
    expect_err("read_group_history", json!({"thread_only": 1}));
}

#[test]
fn float_that_looks_like_an_integer_is_rejected() {
    // JSON Schema draft 2020-12 把 50.0 当合法 integer，旧实现的 isinstance(v, int) 不认。
    // schema.rs 补的那道严格检查就是为了这条（移植差异，见回执）。
    let message = expect_err("read_group_history", json!({"limit": 50.0}));
    assert!(message.contains("integer"), "{message}");
}

// --- 上下界 -----------------------------------------------------------------

#[test]
fn limit_outside_1_200_is_rejected() {
    for limit in [json!(0), json!(-1), json!(201), json!(10_000)] {
        expect_err("read_group_history", json!({"limit": limit}));
    }
}

#[test]
fn limit_on_the_boundary_is_accepted() {
    for limit in [1, 50, 200] {
        let out = expect_ok("read_group_history", json!({"limit": limit}));
        assert_eq!(out["limit"], json!(limit));
    }
}

#[test]
fn run_python_timeout_outside_1_300_is_rejected() {
    for timeout_sec in [json!(0), json!(-5), json!(301)] {
        expect_err(
            "run_python",
            json!({"code": "x", "timeout_sec": timeout_sec}),
        );
    }
}

#[test]
fn run_python_timeout_on_the_boundary_is_accepted() {
    for timeout_sec in [1, 120, 300] {
        let out = expect_ok(
            "run_python",
            json!({"code": "x", "timeout_sec": timeout_sec}),
        );
        assert_eq!(out["timeout_sec"], json!(timeout_sec));
    }
}

// --- 多余参数（本层策略，比 JSON Schema 默认更严）---------------------------

#[test]
fn unknown_arguments_are_rejected() {
    let cases = [
        ("read_group_history", json!({"limt": 5})), // 打错字
        ("list_files", json!({"path": "/work"})),   // 这个工具不收参数
        ("run_python", json!({"code": "x", "language": "python"})),
    ];
    for (tool, args) in cases {
        let message = expect_err(tool, args);
        assert!(message.contains("不认识的参数"), "{message}");
    }
}

#[test]
fn unknown_argument_message_lists_what_is_available() {
    // 模型收到的错误要能照着改，所以把可用参数名带上。
    let message = expect_err("read_group_history", json!({"limt": 5}));
    assert!(message.contains("limit"), "{message}");
    assert!(message.contains("thread_only"), "{message}");
}

// --- arguments / schema 本身的形状 ------------------------------------------

#[test]
fn a_non_object_schema_is_rejected() {
    // 旧版这条测的是「arguments 必须是对象」；Rust 的签名收 &Map，那条由类型系统保证，
    // 还留着的风险是 schema 自己写歪了 —— 顶层不是 object 就直接拒。
    let schema = json!({"type": "array", "items": {"type": "string"}});
    let err = validate_arguments(&schema, &Map::new()).expect_err("顶层不是 object 该被拒");
    assert!(err.to_string().contains("顶层必须是 object"), "{err}");
}

// --- 嵌套关键字（GATEWAY_TOOLS 没用到，但 checklist_tools 用了，校验器得撑得住）---

fn nested() -> Value {
    json!({
        "type": "object",
        "properties": {
            "items": {"type": "array", "items": {"type": "string"}, "minItems": 1, "maxItems": 3},
            "who": {
                "type": "object",
                "properties": {"id": {"type": "string"}, "vip": {"type": "boolean", "default": false}},
                "required": ["id"],
            },
            "mode": {"type": "string", "enum": ["fast", "slow"]},
        },
        "required": ["items"],
    })
}

#[test]
fn nested_array_and_object_are_validated() {
    let out = validate_arguments(
        &nested(),
        &as_map(json!({"items": ["a", "b"], "who": {"id": "ou_1"}, "mode": "fast"})),
    )
    .expect("这组该过");
    assert_eq!(
        out,
        as_map(json!({
            "items": ["a", "b"],
            "who": {"id": "ou_1", "vip": false},
            "mode": "fast",
        }))
    );
}

#[test]
fn nested_violations_are_rejected() {
    let cases = [
        json!({"items": []}),                      // minItems
        json!({"items": ["a", "b", "c", "d"]}),    // maxItems
        json!({"items": [1, 2]}),                  // items 的类型
        json!({"items": ["a"], "who": {}}),        // 嵌套必填
        json!({"items": ["a"], "mode": "medium"}), // enum
    ];
    for args in cases {
        let schema = nested();
        assert!(
            validate_arguments(&schema, &as_map(args.clone())).is_err(),
            "{args} 应该被拒"
        );
    }
}
