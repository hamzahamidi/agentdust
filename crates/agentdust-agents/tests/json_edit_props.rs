use agentdust_agents::json_edit::{Json, Seg, append_item, insert_member, remove_item, remove_member};
use proptest::prelude::*;
use serde::Serialize;
use serde_json::ser::PrettyFormatter;
use serde_json::{Value, json};

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i32>().prop_map(|number| json!(number)),
        "[a-z \\\\\"\\[\\]{},:é]{0,8}".prop_map(Value::String),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Value::Array),
            prop::collection::btree_map("[a-z]{1,4}", inner, 0..4)
                .prop_map(|map| Value::Object(map.into_iter().collect())),
        ]
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

fn paths(value: &Value, here: &mut Vec<Seg>, out: &mut Vec<(Vec<Seg>, bool)>) {
    match value {
        Value::Object(map) => {
            out.push((here.clone(), true));
            for (name, child) in map {
                here.push(Seg::Key(name.clone()));
                paths(child, here, out);
                here.pop();
            }
        }
        Value::Array(items) => {
            out.push((here.clone(), false));
            for (position, child) in items.iter().enumerate() {
                here.push(Seg::Index(position));
                paths(child, here, out);
                here.pop();
            }
        }
        _ => {}
    }
}

fn at<'a>(value: &'a Value, path: &[Seg]) -> &'a Value {
    path.iter().fold(value, |current, seg| match seg {
        Seg::Key(name) => &current[name.as_str()],
        Seg::Index(position) => &current[*position],
    })
}

fn addition() -> Json {
    Json::Object(vec![
        ("matcher".to_owned(), Json::Str("Bash".to_owned())),
        ("hooks".to_owned(), Json::Array(vec![Json::Int(10), Json::Array(vec![])])),
    ])
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn adding_then_removing_restores_every_byte(
        document in value(),
        style in 0u8..8,
        pick in any::<prop::sample::Index>(),
    ) {
        let text = render(&document, style);
        let mut found = Vec::new();
        paths(&document, &mut Vec::new(), &mut found);
        prop_assume!(!found.is_empty());
        let (path, is_object) = found[pick.index(found.len())].clone();
        let (added, restored) = if is_object {
            let added = insert_member(&text, &path, "zz-new", &addition()).unwrap();
            let restored = remove_member(&added, &path, "zz-new").unwrap();
            (added, restored)
        } else {
            let length = at(&document, &path).as_array().unwrap().len();
            let added = append_item(&text, &path, &addition()).unwrap();
            let restored = remove_item(&added, &path, length).unwrap();
            (added, restored)
        };
        prop_assert_eq!(&restored, &text);
        let parsed: Value = serde_json::from_str(&added).unwrap();
        let expected_container = at(&parsed, &path);
        if is_object {
            prop_assert_eq!(&expected_container["zz-new"], &addition().to_value());
        } else {
            prop_assert_eq!(expected_container.as_array().unwrap().last().unwrap(), &addition().to_value());
        }
    }
}
