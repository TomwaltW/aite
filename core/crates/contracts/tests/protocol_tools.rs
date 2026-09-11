use std::collections::HashSet;

use aite_contracts::*;

#[test]
fn catalog_is_frozen() {
    let all = all_model_tools();
    assert_eq!(all.len(), 10);
    let names: Vec<&str> = all.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "checklist_add",
            "checklist_check",
            "checklist_fail",
            "checklist_note",
            "final",
            "read_group_history",
            "read_document",
            "download_attachment",
            "run_python",
            "list_files"
        ]
    );
    assert_eq!(
        names.iter().collect::<HashSet<_>>().len(),
        names.len(),
        "名字唯一"
    );
    for t in all {
        assert_eq!(
            t.parameters["type"], "object",
            "{} 的 parameters.type 必须是 object",
            t.name
        );
    }
    assert_eq!(checklist_tools().len(), 4);
    assert_eq!(final_tool().name, "final");
    assert_eq!(gateway_tools().len(), 5);
}

#[test]
fn local_tool_names_are_checklist_plus_final() {
    let expected: HashSet<String> = [
        "checklist_add",
        "checklist_check",
        "checklist_fail",
        "checklist_note",
        "final",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(local_tool_names(), &expected);
    assert!(is_local_tool("final") && !is_local_tool("run_python"));
}

#[test]
fn frozen_schema_details() {
    let run_python = gateway_tools()
        .iter()
        .find(|t| t.name == "run_python")
        .unwrap();
    assert_eq!(
        run_python.parameters["properties"]["timeout_sec"]["default"],
        120
    );
    assert_eq!(
        run_python.parameters["properties"]["timeout_sec"]["maximum"],
        300
    );
    let history = gateway_tools()
        .iter()
        .find(|t| t.name == "read_group_history")
        .unwrap();
    assert_eq!(history.parameters["properties"]["limit"]["default"], 50);
    let add = checklist_tools()
        .iter()
        .find(|t| t.name == "checklist_add")
        .unwrap();
    assert_eq!(add.parameters["properties"]["items"]["maxItems"], 8);
}
