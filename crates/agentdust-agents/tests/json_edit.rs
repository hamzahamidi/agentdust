use agentdust_agents::json_edit::{
    EditError, Json, Seg, append_item, insert_member, remove_item, remove_member, validate,
};
use serde_json::{Value, json};

fn key(name: &str) -> Seg {
    Seg::Key(name.to_owned())
}

fn text(value: &str) -> Json {
    Json::Str(value.to_owned())
}

fn group() -> Json {
    Json::Object(vec![
        ("matcher".to_owned(), text("Bash")),
        (
            "hooks".to_owned(),
            Json::Array(vec![Json::Object(vec![
                ("type".to_owned(), text("command")),
                ("timeout".to_owned(), Json::Int(10)),
            ])]),
        ),
    ])
}

#[test]
fn a_member_is_added_after_the_last_one_in_a_multi_line_object() {
    let old = "{\n  \"a\": 1,\n  \"b\": [1, 2]\n}\n";
    let new = insert_member(old, &[], "k", &text("v")).unwrap();
    assert_eq!(new, "{\n  \"a\": 1,\n  \"b\": [1, 2],\n  \"k\": \"v\"\n}\n");
}

#[test]
fn the_indent_of_the_file_is_followed() {
    let four = insert_member("{\n    \"a\": 1\n}", &[], "k", &group()).unwrap();
    assert_eq!(
        four,
        "{\n    \"a\": 1,\n    \"k\": {\n        \"matcher\": \"Bash\",\n        \"hooks\": [\n            {\n                \"type\": \"command\",\n                \"timeout\": 10\n            }\n        ]\n    }\n}"
    );
    let tab = insert_member("{\n\t\"a\": 1\n}", &[], "k", &Json::Array(vec![text("x")])).unwrap();
    assert_eq!(tab, "{\n\t\"a\": 1,\n\t\"k\": [\n\t\t\"x\"\n\t]\n}");
}

#[test]
fn a_member_is_added_inside_a_nested_object() {
    let old = "{\n  \"hooks\": {\n    \"Stop\": []\n  },\n  \"model\": \"x\"\n}\n";
    let new = insert_member(old, &[key("hooks")], "PreToolUse", &Json::Array(vec![])).unwrap();
    assert_eq!(
        new,
        "{\n  \"hooks\": {\n    \"Stop\": [],\n    \"PreToolUse\": []\n  },\n  \"model\": \"x\"\n}\n"
    );
}

#[test]
fn the_first_member_of_an_empty_object_uses_the_indent_of_its_line() {
    let new = insert_member("{}\n", &[], "hooks", &Json::Object(vec![])).unwrap();
    assert_eq!(new, "{\n  \"hooks\": {}\n}\n");
    let old = "{\n  \"hooks\": {}\n}\n";
    let new = insert_member(old, &[key("hooks")], "Stop", &Json::Array(vec![text("x")])).unwrap();
    assert_eq!(
        new,
        "{\n  \"hooks\": {\n    \"Stop\": [\n      \"x\"\n    ]\n  }\n}\n"
    );
}

#[test]
fn a_one_line_document_stays_on_one_line() {
    let new = insert_member("{\"a\":1}", &[], "k", &group()).unwrap();
    assert_eq!(
        new,
        "{\"a\":1, \"k\": {\"matcher\": \"Bash\", \"hooks\": [{\"type\": \"command\", \"timeout\": 10}]}}"
    );
}

#[test]
fn an_empty_document_is_written_in_the_multi_line_layout() {
    assert_eq!(
        insert_member("{}", &[], "k", &Json::Int(1)).unwrap(),
        "{\n  \"k\": 1\n}"
    );
    assert_eq!(append_item("[]", &[], &Json::Int(1)).unwrap(), "[\n  1\n]");
    assert_eq!(
        insert_member("{\"a\":{}}", &[key("a")], "k", &Json::Int(1)).unwrap(),
        "{\"a\":{\"k\": 1}}"
    );
}

#[test]
fn a_one_line_container_inside_a_multi_line_file_stays_on_one_line() {
    let old = "{\n  \"permissions\": {\"allow\": [\"a\"]}\n}\n";
    let new = append_item(old, &[key("permissions"), key("allow")], &text("b")).unwrap();
    assert_eq!(new, "{\n  \"permissions\": {\"allow\": [\"a\", \"b\"]}\n}\n");
}

#[test]
fn an_item_is_appended_after_the_last_one() {
    let old = "{\n  \"list\": [\n    1,\n    2\n  ]\n}\n";
    let new = append_item(old, &[key("list")], &Json::Int(3)).unwrap();
    assert_eq!(new, "{\n  \"list\": [\n    1,\n    2,\n    3\n  ]\n}\n");
}

#[test]
fn the_first_item_of_an_empty_array_is_indented_under_its_line() {
    let old = "{\n  \"hooks\": {\n    \"Stop\": []\n  }\n}\n";
    let new = append_item(old, &[key("hooks"), key("Stop")], &Json::Int(1)).unwrap();
    assert_eq!(
        new,
        "{\n  \"hooks\": {\n    \"Stop\": [\n      1\n    ]\n  }\n}\n"
    );
}

#[test]
fn an_item_is_appended_inside_an_array_reached_through_an_index() {
    let old = "{\n  \"a\": [\n    {\n      \"b\": []\n    }\n  ]\n}\n";
    let new = append_item(old, &[key("a"), Seg::Index(0), key("b")], &Json::Int(7)).unwrap();
    assert_eq!(
        new,
        "{\n  \"a\": [\n    {\n      \"b\": [\n        7\n      ]\n    }\n  ]\n}\n"
    );
}

#[test]
fn removing_the_member_that_was_added_restores_the_exact_bytes() {
    for old in [
        "{\n  \"a\": 1,\n  \"b\": [1, 2]\n}\n",
        "{\n    \"a\": 1\n}",
        "{\n\t\"a\": 1\n}\n",
        "{\"a\":1}",
        "{}",
        "{}\n",
        "{\n  \"a\": {}\n}\n",
    ] {
        let added = insert_member(old, &[], "zz", &group()).unwrap();
        assert_ne!(added, old);
        assert_eq!(remove_member(&added, &[], "zz").unwrap(), old, "{old:?}");
    }
}

#[test]
fn removing_the_item_that_was_appended_restores_the_exact_bytes() {
    for old in [
        "[1, 2]",
        "[\n  1,\n  2\n]\n",
        "{\n  \"a\": [\n    1\n  ]\n}\n",
        "{\n  \"a\": []\n}\n",
        "{\"a\": []}",
    ] {
        let at: Vec<Seg> = if old.starts_with('[') {
            vec![]
        } else {
            vec![key("a")]
        };
        let added = append_item(old, &at, &group()).unwrap();
        let count = serde_json::from_str::<Value>(&added).unwrap();
        let length = if at.is_empty() { &count } else { &count["a"] }
            .as_array()
            .unwrap()
            .len();
        assert_eq!(remove_item(&added, &at, length - 1).unwrap(), old, "{old:?}");
    }
}

#[test]
fn removing_a_middle_or_first_item_takes_one_separator_with_it() {
    let old = "[\n  1,\n  2,\n  3\n]";
    assert_eq!(remove_item(old, &[], 1).unwrap(), "[\n  1,\n  3\n]");
    assert_eq!(remove_item(old, &[], 0).unwrap(), "[\n  2,\n  3\n]");
    assert_eq!(remove_item(old, &[], 2).unwrap(), "[\n  1,\n  2\n]");
    assert_eq!(remove_item("[1, 2, 3]", &[], 1).unwrap(), "[1, 3]");
}

#[test]
fn removing_the_only_item_or_member_leaves_an_empty_container() {
    assert_eq!(remove_item("[\n  1\n]", &[], 0).unwrap(), "[]");
    assert_eq!(remove_member("{\n  \"a\": 1\n}\n", &[], "a").unwrap(), "{}\n");
}

#[test]
fn removing_a_middle_member_takes_one_separator_with_it() {
    let old = "{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}";
    assert_eq!(
        remove_member(old, &[], "b").unwrap(),
        "{\n  \"a\": 1,\n  \"c\": 3\n}"
    );
    assert_eq!(
        remove_member(old, &[], "a").unwrap(),
        "{\n  \"b\": 2,\n  \"c\": 3\n}"
    );
}

#[test]
fn bytes_outside_the_edit_are_never_touched() {
    let old = "{\"a\" : [1,2 ,3],\n \"weird\":   {\"x\":\"\\u00e9\\\"}]{\", \"y\" : 1e3 , \"z\":1.0},\n  \"big\": 12345678901234567890 }";
    let new = insert_member(old, &[], "k", &text("v")).unwrap();
    assert_eq!(new, old.replace("890 }", "890, \"k\": \"v\" }"));
    assert_eq!(remove_member(&new, &[], "k").unwrap(), old);
}

#[test]
fn strings_with_brackets_commas_and_escapes_do_not_confuse_the_scanner() {
    let old = "{\n  \"a\": \"],}\\\\\",\n  \"b\": [\"{\", \"\\\"[\"]\n}";
    let new = append_item(old, &[key("b")], &text("]")).unwrap();
    let parsed: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(parsed["a"], json!("],}\\"));
    assert_eq!(parsed["b"], json!(["{", "\"[", "]"]));
}

#[test]
fn a_new_member_is_valid_json_with_the_value_in_it() {
    let new = insert_member("{\n  \"a\": 1\n}", &[], "g", &group()).unwrap();
    let parsed: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(parsed["a"], json!(1));
    assert_eq!(parsed["g"], group().to_value());
    assert_eq!(
        group().to_value(),
        json!({"matcher": "Bash", "hooks": [{"type": "command", "timeout": 10}]})
    );
}

#[test]
fn strings_are_escaped_when_rendered() {
    let new = insert_member("{}", &[], "k", &text("a \"quoted\" \\ path\nnext")).unwrap();
    let parsed: Value = serde_json::from_str(&new).unwrap();
    assert_eq!(parsed["k"], json!("a \"quoted\" \\ path\nnext"));
}

#[test]
fn invalid_json_is_refused_and_not_repaired() {
    for old in [
        "",
        "{",
        "{\"a\":}",
        "{\"a\":1,}",
        "[1,]",
        "not json",
        "{} {}",
        "\u{feff}{}",
    ] {
        assert!(
            matches!(
                insert_member(old, &[], "k", &Json::Int(1)),
                Err(EditError::Syntax(_))
            ),
            "{old:?}"
        );
    }
}

#[test]
fn a_very_deep_document_is_an_error_and_not_a_stack_overflow() {
    let deep = format!("{}{}", "[".repeat(400), "]".repeat(400));
    assert!(matches!(
        append_item(&deep, &[], &Json::Int(1)),
        Err(EditError::Syntax(_))
    ));
    let fine = format!("{}{}", "[".repeat(100), "]".repeat(100));
    assert!(append_item(&fine, &[], &Json::Int(1)).is_ok());
}

#[test]
fn a_path_that_does_not_exist_is_an_error() {
    let old = "{\"a\": {\"b\": [1]}}";
    assert!(matches!(
        insert_member(old, &[key("nope")], "k", &Json::Int(1)),
        Err(EditError::NotFound(_))
    ));
    assert!(matches!(
        append_item(old, &[key("a"), key("b"), Seg::Index(4)], &Json::Int(1)),
        Err(EditError::NotFound(_))
    ));
    assert!(matches!(
        remove_member(old, &[key("a")], "zz"),
        Err(EditError::NotFound(_))
    ));
    assert!(matches!(
        remove_item(old, &[key("a"), key("b")], 1),
        Err(EditError::NotFound(_))
    ));
}

#[test]
fn the_wrong_kind_of_container_is_an_error() {
    let old = "{\"a\": [1], \"s\": \"x\"}";
    assert!(matches!(
        insert_member(old, &[key("a")], "k", &Json::Int(1)),
        Err(EditError::WrongType { .. })
    ));
    assert!(matches!(
        append_item(old, &[], &Json::Int(1)),
        Err(EditError::WrongType { .. })
    ));
    assert!(matches!(
        insert_member(old, &[key("s")], "k", &Json::Int(1)),
        Err(EditError::WrongType { .. })
    ));
    assert!(matches!(
        remove_member(old, &[key("a")], "k"),
        Err(EditError::WrongType { .. })
    ));
}

#[test]
fn an_existing_key_is_never_overwritten_or_duplicated() {
    let old = "{\"a\": 1}";
    assert!(matches!(
        insert_member(old, &[], "a", &Json::Int(2)),
        Err(EditError::Exists(_))
    ));
}

#[test]
fn a_duplicate_key_on_the_way_is_refused() {
    let old = "{\"hooks\": {}, \"hooks\": {}}";
    assert!(matches!(
        insert_member(old, &[key("hooks")], "k", &Json::Int(1)),
        Err(EditError::Duplicate(_))
    ));
    assert!(matches!(
        remove_member(old, &[], "hooks"),
        Err(EditError::Duplicate(_))
    ));
}

#[test]
fn a_duplicate_key_anywhere_in_the_document_is_refused() {
    let old = "{\"a\": {\"x\": 1, \"x\": 2}}";
    assert!(matches!(
        insert_member(old, &[], "k", &Json::Int(1)),
        Err(EditError::Duplicate(_))
    ));
}

#[test]
fn validate_accepts_unambiguous_json_and_names_the_problem_otherwise() {
    assert_eq!(validate("{\"a\": [1, {\"b\": 2}]}\n"), Ok(()));
    assert_eq!(validate("[]"), Ok(()));
    assert!(matches!(
        validate("{\"a\":1,\"a\":2}"),
        Err(EditError::Duplicate(_))
    ));
    assert!(matches!(validate("{"), Err(EditError::Syntax(_))));
    assert!(matches!(validate(""), Err(EditError::Syntax(_))));
}
