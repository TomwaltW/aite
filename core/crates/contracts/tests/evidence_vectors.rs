//! §3.1 的 hash 测试向量逐字节校验 + 与 Python json.dumps(sort_keys, ensure_ascii=False, separators) 的对拍。
use aite_contracts::{GENESIS, canonical_json, chain_hash, payload_hash_of};
use serde_json::{Map, Value, json};

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().expect("object").clone()
}

#[test]
fn spec_vectors_byte_exact() {
    let p1 = obj(json!({"a": 1}));
    assert_eq!(canonical_json(&p1), "{\"a\":1}");
    assert_eq!(
        payload_hash_of(&p1),
        "015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
    );
    let h1 = chain_hash(GENESIS, &payload_hash_of(&p1));
    assert_eq!(
        h1,
        "cdae94bd29b7de0c2a7af392f4d984ed5a41b15bdd872f7b9726132508047adc"
    );

    let p2 = obj(json!({"b": "文"}));
    assert_eq!(canonical_json(&p2), "{\"b\":\"文\"}");
    assert_eq!(
        payload_hash_of(&p2),
        "1e8763171f38ca61b0bb0f996142a149ce16ba66c664d341d03c91f54ae4ea10"
    );
    assert_eq!(
        chain_hash(&h1, &payload_hash_of(&p2)),
        "11dbc980a72dcce295871168fe45f8926b2b9e8ac90a2fb783dfb586e1d38f5e"
    );
}

/// 四组用 python3 现算的向量（见 R0 回执）：嵌套、Unicode、控制字符、大整数、键里带斜杠/制表符。
#[test]
fn matches_python_json_dumps_on_tricky_payloads() {
    let cases: Vec<(Value, &str, &str)> = vec![
        (
            json!({"b":"文","a":[1,2,{"z":null,"y":true}],"c":"quote\"back\\slash\n\t"}),
            "{\"a\":[1,2,{\"y\":true,\"z\":null}],\"b\":\"文\",\"c\":\"quote\\\"back\\\\slash\\n\\t\"}",
            "071a45c2ce92b6cb03d9f4886d16ec0bd8977f66bacb25ff318b6a8d76b25e80",
        ),
        (
            json!({"é":1,"z":"\u{2028}line sep","A":false,"a":[[],{}],"emoji":"\u{1F642}","ctrl":"\u{8}\u{c}\r\u{1}"}),
            "{\"A\":false,\"a\":[[],{}],\"ctrl\":\"\\b\\f\\r\\u0001\",\"emoji\":\"\u{1F642}\",\"z\":\"\u{2028}line sep\",\"é\":1}",
            "7e11827c7bb3377229b1d245d78701df9474bb4fc7fc351ee1afd7a7f7d4093c",
        ),
        (
            json!({"n":-5,"big":12345678901234567890u64,"neg0":0,"nested":{"k2":{"k1":[true,null]},"k1":"v"}}),
            "{\"big\":12345678901234567890,\"n\":-5,\"neg0\":0,\"nested\":{\"k1\":\"v\",\"k2\":{\"k1\":[true,null]}}}",
            "c225e9e42a7a27863264c21a99d18018e27dd5b8ff231bcd1a4431f0383cabdd",
        ),
        (
            json!({"tab\tkey":"x","key/with/slash":"/","del":"\u{7f}","cn":"中文键值：全角，标点。"}),
            "{\"cn\":\"中文键值：全角，标点。\",\"del\":\"\u{7f}\",\"key/with/slash\":\"/\",\"tab\\tkey\":\"x\"}",
            "1d01f0e75c7ef853e9de2521f01093841902556caea4c6a33cf264299843eff2",
        ),
    ];
    for (payload, canonical, hash) in cases {
        let m = obj(payload);
        assert_eq!(canonical_json(&m), canonical);
        assert_eq!(payload_hash_of(&m), hash);
    }
}

#[test]
fn key_order_is_codepoint_order_regardless_of_insertion() {
    let mut m = Map::new();
    m.insert("z".into(), json!(1));
    m.insert("a".into(), json!(2));
    m.insert("é".into(), json!(3));
    m.insert("B".into(), json!(4));
    assert_eq!(canonical_json(&m), "{\"B\":4,\"a\":2,\"z\":1,\"é\":3}");
}

#[test]
fn genesis_is_64_zeros() {
    assert_eq!(GENESIS.len(), 64);
    assert!(GENESIS.chars().all(|c| c == '0'));
}
