use std::collections::HashSet;

use serde::de::IgnoredAny;
use serde_json::Value;
use thiserror::Error;

const DEFAULT_UNIT: &str = "  ";
const MAX_DEPTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Seg {
    Key(String),
    Index(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Str(String),
    Int(u64),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("the text is not valid JSON: {0}")]
    Syntax(String),
    #[error("{0} does not exist")]
    NotFound(String),
    #[error("{at} is not {expected}")]
    WrongType { at: String, expected: &'static str },
    #[error("the key \"{0}\" appears more than once")]
    Duplicate(String),
    #[error("\"{0}\" already exists")]
    Exists(String),
}

impl Json {
    pub fn to_value(&self) -> Value {
        match self {
            Self::Str(text) => Value::String(text.clone()),
            Self::Int(number) => Value::from(*number),
            Self::Array(items) => Value::Array(items.iter().map(Self::to_value).collect()),
            Self::Object(members) => Value::Object(
                members
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_value()))
                    .collect(),
            ),
        }
    }

    fn render(&self, indent: &str, unit: &str, multiline: bool) -> String {
        match self {
            Self::Str(text) => Value::String(text.clone()).to_string(),
            Self::Int(number) => number.to_string(),
            Self::Array(items) if items.is_empty() => "[]".to_owned(),
            Self::Object(members) if members.is_empty() => "{}".to_owned(),
            Self::Array(items) => {
                let parts: Vec<String> = items
                    .iter()
                    .map(|item| item.render(&inner(indent, unit, multiline), unit, multiline))
                    .collect();
                wrap('[', ']', &parts, indent, unit, multiline)
            }
            Self::Object(members) => {
                let parts: Vec<String> = members
                    .iter()
                    .map(|(name, value)| {
                        let deeper = inner(indent, unit, multiline);
                        format!(
                            "{}: {}",
                            Value::String(name.clone()),
                            value.render(&deeper, unit, multiline)
                        )
                    })
                    .collect();
                wrap('{', '}', &parts, indent, unit, multiline)
            }
        }
    }
}

fn inner(indent: &str, unit: &str, multiline: bool) -> String {
    if multiline {
        format!("{indent}{unit}")
    } else {
        String::new()
    }
}

fn wrap(open: char, close: char, parts: &[String], indent: &str, unit: &str, multiline: bool) -> String {
    if multiline {
        let deeper = format!("{indent}{unit}");
        let body: Vec<String> = parts.iter().map(|part| format!("{deeper}{part}")).collect();
        format!("{open}\n{}\n{indent}{close}", body.join(",\n"))
    } else {
        format!("{open}{}{close}", parts.join(", "))
    }
}

#[derive(Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

struct Member {
    key: String,
    key_start: usize,
    value: Node,
}

enum Node {
    Object { span: Span, members: Vec<Member> },
    Array { span: Span, items: Vec<Node> },
    Scalar(Span),
}

impl Node {
    fn span(&self) -> Span {
        match self {
            Self::Object { span, .. } | Self::Array { span, .. } => *span,
            Self::Scalar(span) => *span,
        }
    }
}

struct Scanner<'a> {
    text: &'a str,
    pos: usize,
    depth: usize,
}

impl Scanner<'_> {
    fn peek(&self) -> Result<u8, EditError> {
        self.text
            .as_bytes()
            .get(self.pos)
            .copied()
            .ok_or_else(|| EditError::Syntax("unexpected end of text".to_owned()))
    }

    fn skip_whitespace(&mut self) {
        while matches!(
            self.text.as_bytes().get(self.pos),
            Some(b' ' | b'\t' | b'\n' | b'\r')
        ) {
            self.pos += 1;
        }
    }

    fn skip_string(&mut self) -> Result<(), EditError> {
        self.pos += 1;
        loop {
            match self.peek()? {
                b'\\' => self.pos += 2,
                b'"' => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => self.pos += 1,
            }
        }
    }

    fn nested(
        &mut self,
        start: usize,
        body: fn(&mut Self, usize) -> Result<Node, EditError>,
    ) -> Result<Node, EditError> {
        if self.depth >= MAX_DEPTH {
            return Err(EditError::Syntax("recursion limit exceeded".to_owned()));
        }
        self.depth += 1;
        let node = body(self, start);
        self.depth -= 1;
        node
    }

    fn value(&mut self) -> Result<Node, EditError> {
        self.skip_whitespace();
        let start = self.pos;
        match self.peek()? {
            b'{' => self.nested(start, Self::object),
            b'[' => self.nested(start, Self::array),
            b'"' => {
                self.skip_string()?;
                Ok(Node::Scalar(Span { start, end: self.pos }))
            }
            _ => {
                while !matches!(self.peek()?, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r') {
                    self.pos += 1;
                }
                Ok(Node::Scalar(Span { start, end: self.pos }))
            }
        }
    }

    fn object(&mut self, start: usize) -> Result<Node, EditError> {
        self.pos += 1;
        let mut members = Vec::new();
        let mut seen = HashSet::new();
        self.skip_whitespace();
        if self.peek()? == b'}' {
            self.pos += 1;
            return Ok(Node::Object {
                span: Span { start, end: self.pos },
                members,
            });
        }
        loop {
            self.skip_whitespace();
            let key_start = self.pos;
            self.skip_string()?;
            let key: String = serde_json::from_str(&self.text[key_start..self.pos])
                .map_err(|err| EditError::Syntax(err.to_string()))?;
            self.skip_whitespace();
            self.pos += 1;
            let value = self.value()?;
            if !seen.insert(key.clone()) {
                return Err(EditError::Duplicate(key));
            }
            members.push(Member {
                key,
                key_start,
                value,
            });
            self.skip_whitespace();
            let separator = self.peek()?;
            self.pos += 1;
            if separator == b'}' {
                return Ok(Node::Object {
                    span: Span { start, end: self.pos },
                    members,
                });
            }
        }
    }

    fn array(&mut self, start: usize) -> Result<Node, EditError> {
        self.pos += 1;
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek()? == b']' {
            self.pos += 1;
            return Ok(Node::Array {
                span: Span { start, end: self.pos },
                items,
            });
        }
        loop {
            items.push(self.value()?);
            self.skip_whitespace();
            let separator = self.peek()?;
            self.pos += 1;
            if separator == b']' {
                return Ok(Node::Array {
                    span: Span { start, end: self.pos },
                    items,
                });
            }
        }
    }
}

fn parse(text: &str) -> Result<Node, EditError> {
    serde_json::from_str::<IgnoredAny>(text).map_err(|err| EditError::Syntax(err.to_string()))?;
    Scanner {
        text,
        pos: 0,
        depth: 0,
    }
    .value()
}

pub fn validate(text: &str) -> Result<(), EditError> {
    parse(text).map(drop)
}

fn describe(at: &[Seg]) -> String {
    if at.is_empty() {
        return "the document".to_owned();
    }
    let mut out = String::new();
    for seg in at {
        match seg {
            Seg::Key(name) if out.is_empty() => out.push_str(name),
            Seg::Key(name) => {
                out.push('.');
                out.push_str(name);
            }
            Seg::Index(index) => out.push_str(&format!("[{index}]")),
        }
    }
    out
}

fn locate<'a>(root: &'a Node, at: &[Seg]) -> Result<&'a Node, EditError> {
    let mut node = root;
    for (depth, seg) in at.iter().enumerate() {
        let here = &at[..=depth];
        node = match (node, seg) {
            (Node::Object { members, .. }, Seg::Key(name)) => members
                .iter()
                .find(|member| &member.key == name)
                .map(|member| &member.value)
                .ok_or_else(|| EditError::NotFound(describe(here)))?,
            (Node::Array { items, .. }, Seg::Index(index)) => items
                .get(*index)
                .ok_or_else(|| EditError::NotFound(describe(here)))?,
            (Node::Object { .. }, Seg::Index(_)) => {
                return Err(EditError::WrongType {
                    at: describe(&at[..depth]),
                    expected: "an array",
                });
            }
            _ => {
                return Err(EditError::WrongType {
                    at: describe(&at[..depth]),
                    expected: "an object",
                });
            }
        };
    }
    Ok(node)
}

fn line_indent(text: &str, position: usize) -> &str {
    let line_start = text[..position].rfind('\n').map_or(0, |newline| newline + 1);
    let line = &text[line_start..];
    let width = line.find(|ch| ch != ' ' && ch != '\t').unwrap_or(line.len());
    &text[line_start..line_start + width]
}

fn indent_after_last_newline(gap: &str) -> Option<&str> {
    gap.rfind('\n').map(|newline| &gap[newline + 1..])
}

fn unit_of(text: &str, root: &Node) -> String {
    let (span, first_start) = match root {
        Node::Object { span, members } => (span, members.first().map(|member| member.key_start)),
        Node::Array { span, items } => (span, items.first().map(|item| item.span().start)),
        Node::Scalar(_) => return DEFAULT_UNIT.to_owned(),
    };
    let Some(first_start) = first_start else {
        return DEFAULT_UNIT.to_owned();
    };
    let base = line_indent(text, span.start);
    match indent_after_last_newline(&text[span.start + 1..first_start]) {
        Some(indent) if indent.len() > base.len() && indent.starts_with(base) => {
            indent[base.len()..].to_owned()
        }
        _ => DEFAULT_UNIT.to_owned(),
    }
}

fn splice(text: &str, start: usize, end: usize, replacement: &str) -> Result<String, EditError> {
    let edited = format!("{}{}{}", &text[..start], replacement, &text[end..]);
    parse(&edited).map_err(|err| EditError::Syntax(format!("the edit would produce invalid JSON: {err}")))?;
    Ok(edited)
}

fn container_is_multiline(text: &str, at: &[Seg]) -> bool {
    at.is_empty() || text.contains('\n')
}

pub fn insert_member(text: &str, at: &[Seg], key: &str, value: &Json) -> Result<String, EditError> {
    let root = parse(text)?;
    let Node::Object { span, members } = locate(&root, at)? else {
        return Err(EditError::WrongType {
            at: describe(at),
            expected: "an object",
        });
    };
    if members.iter().any(|member| member.key == key) {
        return Err(EditError::Exists(key.to_owned()));
    }
    let unit = unit_of(text, &root);
    let member = |indent: &str, multiline: bool| {
        format!(
            "{}: {}",
            Value::String(key.to_owned()),
            value.render(indent, &unit, multiline)
        )
    };
    let (Some(first), Some(last)) = (members.first(), members.last()) else {
        let replacement = if container_is_multiline(text, at) {
            let base = line_indent(text, span.start);
            let indent = format!("{base}{unit}");
            format!("{{\n{indent}{}\n{base}}}", member(&indent, true))
        } else {
            format!("{{{}}}", member("", false))
        };
        return splice(text, span.start, span.end, &replacement);
    };
    let end = last.value.span().end;
    let addition = match indent_after_last_newline(&text[span.start + 1..first.key_start]) {
        Some(indent) => format!(",\n{indent}{}", member(indent, true)),
        None => format!(", {}", member("", false)),
    };
    splice(text, end, end, &addition)
}

pub fn append_item(text: &str, at: &[Seg], value: &Json) -> Result<String, EditError> {
    let root = parse(text)?;
    let Node::Array { span, items } = locate(&root, at)? else {
        return Err(EditError::WrongType {
            at: describe(at),
            expected: "an array",
        });
    };
    let unit = unit_of(text, &root);
    let (Some(first), Some(last)) = (items.first(), items.last()) else {
        let replacement = if container_is_multiline(text, at) {
            let base = line_indent(text, span.start);
            let indent = format!("{base}{unit}");
            format!("[\n{indent}{}\n{base}]", value.render(&indent, &unit, true))
        } else {
            format!("[{}]", value.render("", &unit, false))
        };
        return splice(text, span.start, span.end, &replacement);
    };
    let end = last.span().end;
    let addition = match indent_after_last_newline(&text[span.start + 1..first.span().start]) {
        Some(indent) => format!(",\n{indent}{}", value.render(indent, &unit, true)),
        None => format!(", {}", value.render("", &unit, false)),
    };
    splice(text, end, end, &addition)
}

fn remove_one(
    text: &str,
    container: Span,
    parts: &[Span],
    index: usize,
    empty: &str,
) -> Result<String, EditError> {
    if parts.len() == 1 {
        return splice(text, container.start, container.end, empty);
    }
    if index + 1 == parts.len() {
        splice(text, parts[index - 1].end, parts[index].end, "")
    } else {
        splice(text, parts[index].start, parts[index + 1].start, "")
    }
}

pub fn remove_member(text: &str, at: &[Seg], key: &str) -> Result<String, EditError> {
    let root = parse(text)?;
    let Node::Object { span, members } = locate(&root, at)? else {
        return Err(EditError::WrongType {
            at: describe(at),
            expected: "an object",
        });
    };
    let index = members
        .iter()
        .position(|member| member.key == key)
        .ok_or_else(|| EditError::NotFound(format!("{} in {}", key, describe(at))))?;
    let parts: Vec<Span> = members
        .iter()
        .map(|member| Span {
            start: member.key_start,
            end: member.value.span().end,
        })
        .collect();
    remove_one(text, *span, &parts, index, "{}")
}

pub fn remove_item(text: &str, at: &[Seg], index: usize) -> Result<String, EditError> {
    let root = parse(text)?;
    let Node::Array { span, items } = locate(&root, at)? else {
        return Err(EditError::WrongType {
            at: describe(at),
            expected: "an array",
        });
    };
    if index >= items.len() {
        return Err(EditError::NotFound(format!("item {index} of {}", describe(at))));
    }
    let parts: Vec<Span> = items.iter().map(Node::span).collect();
    remove_one(text, *span, &parts, index, "[]")
}
