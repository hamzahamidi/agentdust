use agentdust_agents::hook_config::{HOOK_SPECS, HookState, inspect, install, remove};
use proptest::prelude::*;
use serde::Serialize;
use serde_json::ser::PrettyFormatter;
use serde_json::{Map, Value, json};

const CMD: &str = "/opt/homebrew/bin/agentdust hook claude";
const TARGET: &str = "/home/u/.claude/settings.json";

fn other() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|number| json!(number)),
        "[a-z \\\\\"é]{0,6}".prop_map(Value::String),
        prop::collection::vec("[a-z]{0,4}".prop_map(Value::String), 0..3).prop_map(Value::Array),
    ]
}

fn user_group() -> impl Strategy<Value = Value> {
    (prop::option::of("[A-Za-z|]{1,6}"), "[a-z/ ]{1,10}").prop_map(|(matcher, command)| {
        let mut group = json!({"hooks": [{"type": "command", "command": command}]});
        if let Some(matcher) = matcher {
            group["matcher"] = json!(matcher);
        }
        group
    })
}

fn hooks_object() -> impl Strategy<Value = Value> {
    prop::collection::btree_map(
        prop::sample::select(vec!["Stop", "Notification", "PreToolUse", "SessionStart", "UserPromptSubmit"]),
        prop::collection::vec(user_group(), 0..3),
        0..4,
    )
    .prop_map(|map| {
        Value::Object(
            map.into_iter()
                .map(|(event, groups)| (event.to_owned(), Value::Array(groups)))
                .collect::<Map<_, _>>(),
        )
    })
}

fn settings() -> impl Strategy<Value = Value> {
    (
        prop::collection::btree_map("[a-z]{1,6}", other(), 0..4),
        prop::option::of(hooks_object()),
    )
        .prop_map(|(others, hooks)| {
            let mut map: Map<String, Value> = others.into_iter().collect();
            map.remove("hooks");
            if let Some(hooks) = hooks {
                map.insert("hooks".to_owned(), hooks);
            }
            Value::Object(map)
        })
}

fn render(value: &Value, style: u8) -> String {
    let indent: &[u8] = match style % 4 {
        0 => return serde_json::to_string(value).unwrap(),
        1 => b"  ",
        2 => b"    ",
        _ => b"\t",
    };
    let mut out = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut out, PrettyFormatter::with_indent(indent));
    value.serialize(&mut serializer).unwrap();
    let mut text = String::from_utf8(out).unwrap();
    if style >= 4 {
        text.push('\n');
    }
    text
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn install_adds_only_our_groups_and_remove_restores_every_byte(document in settings(), style in 0u8..8) {
        let text = render(&document, style);
        let first = install(Some(&text), TARGET, &[], CMD).unwrap();
        let after: Value = serde_json::from_str(&first.text).unwrap();
        for (key, value) in document.as_object().unwrap() {
            if key != "hooks" {
                prop_assert_eq!(&after[key.as_str()], value);
            }
        }
        for (event, groups) in document.get("hooks").and_then(Value::as_object).into_iter().flatten() {
            let kept = after["hooks"][event.as_str()].as_array().unwrap();
            prop_assert_eq!(&kept[..groups.as_array().unwrap().len()], &groups.as_array().unwrap()[..]);
        }
        let again = install(Some(&first.text), TARGET, &first.entries, CMD).unwrap();
        prop_assert_eq!(&again.text, &first.text);
        let report = inspect(Some(&first.text), TARGET, &first.entries, CMD).unwrap();
        prop_assert!(report.iter().all(|item| item.state == HookState::Installed));
        prop_assert_eq!(report.len(), HOOK_SPECS.len());
        let removed = remove(Some(&first.text), TARGET, &first.entries).unwrap();
        prop_assert_eq!(removed.text, text);
    }
}
