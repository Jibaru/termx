//! Changes one member of a JSON file in place, leaving the rest of the text
//! as it was: the order of the keys, the indentation, the line endings, the
//! byte order mark, every member termx does not touch.
//!
//! The settings of Claude Code are the user's file. termx adds two members
//! to it and removes them again; rewriting the whole file from a parsed
//! value would reorder it and reformat it. Every edit is checked: the
//! edited text is parsed again and must be exactly the original value with
//! that one change, or the edit is refused.

use serde_json::{Map, Value};

/// Why a file could not be edited.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditError {
    /// The file is not JSON, or its top level is not an object.
    #[error("{0}")]
    Invalid(String),
    /// The member to change into is there but is not an object.
    #[error("\"{0}\" is not an object")]
    NotAnObject(String),
    /// The edited text did not parse back to what was intended. A bug,
    /// caught before anything is written.
    #[error("the edit could not be made without changing something else")]
    Unverified,
}

/// A JSON text and the positions of its objects.
pub struct Document {
    text: String,
    /// `\u{feff}` when the file starts with one.
    bom: &'static str,
}

/// An object of the text: where its braces are and its members.
#[derive(Debug)]
struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

/// One member of an object.
#[derive(Debug)]
struct Member {
    key: String,
    /// Where the key starts (its opening quote).
    start: usize,
    /// Where the value starts and ends.
    value: std::ops::Range<usize>,
    /// The value, when it is an object.
    object: Option<Object>,
}

impl Document {
    /// Reads `text`.
    ///
    /// # Errors
    ///
    /// [`EditError::Invalid`] unless it is a JSON object.
    pub fn parse(text: &str) -> Result<Self, EditError> {
        let (bom, body) = match text.strip_prefix('\u{feff}') {
            Some(body) => ("\u{feff}", body),
            None => ("", text),
        };
        let value: Value =
            serde_json::from_str(body).map_err(|error| EditError::Invalid(error.to_string()))?;
        if !value.is_object() {
            return Err(EditError::Invalid(
                "the top level is not an object".to_owned(),
            ));
        }
        let document = Self {
            text: body.to_owned(),
            bom,
        };
        document.root()?;
        Ok(document)
    }

    /// The text, as it is to be written.
    #[must_use]
    pub fn text(&self) -> String {
        format!("{}{}", self.bom, self.text)
    }

    /// The parsed value.
    #[must_use]
    pub fn value(&self) -> Value {
        serde_json::from_str(&self.text).unwrap_or(Value::Null)
    }

    /// The value at `path`, if any.
    #[must_use]
    pub fn get(&self, path: &[&str]) -> Option<Value> {
        let mut value = self.value();
        for key in path {
            value = value.as_object_mut()?.remove(*key)?;
        }
        Some(value)
    }

    /// Sets `key` to `value` in the object at `path`, creating the objects
    /// of the path that are missing. A member that is there already has
    /// its value replaced, where it is.
    ///
    /// # Errors
    ///
    /// [`EditError::NotAnObject`] when something on the path is not an
    /// object, and [`EditError::Unverified`].
    pub fn set(&mut self, path: &[&str], key: &str, value: &Value) -> Result<(), EditError> {
        let mut expected = self.value();
        {
            let mut object = expected
                .as_object_mut()
                .ok_or_else(|| EditError::NotAnObject(String::new()))?;
            for step in path {
                object = object
                    .entry((*step).to_owned())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .ok_or_else(|| EditError::NotAnObject((*step).to_owned()))?;
            }
            object.insert(key.to_owned(), value.clone());
        }

        // The deepest object of the path that exists, and what is missing
        // below it.
        let root = self.root()?;
        let mut object = &root;
        let mut depth = 0;
        for step in path {
            let Some(member) = last_member(object, step) else {
                break;
            };
            object = member
                .object
                .as_ref()
                .ok_or_else(|| EditError::NotAnObject((*step).to_owned()))?;
            depth += 1;
        }
        let (target_key, target_value) = match path.get(depth..) {
            Some([]) | None => (key, value.clone()),
            Some([first, rest @ ..]) => {
                let mut nested = Map::new();
                nested.insert(key.to_owned(), value.clone());
                let mut built = Value::Object(nested);
                for step in rest.iter().rev() {
                    let mut wrapper = Map::new();
                    wrapper.insert((*step).to_owned(), built);
                    built = Value::Object(wrapper);
                }
                (*first, built)
            }
        };

        let edited = match last_member(object, target_key) {
            Some(member) => {
                let indent = self.line_indent(member.start);
                let rendered = render(&target_value, &indent, &self.indent_unit(), self.newline());
                splice(&self.text, member.value.clone(), &rendered)
            }
            None => self.insert(object, target_key, &target_value),
        };
        self.commit(edited, &expected)
    }

    /// Removes `key` from the object at `path`, and that object too when it
    /// is left empty and `drop_empty` says so. Nothing happens when it is
    /// not there.
    ///
    /// # Errors
    ///
    /// [`EditError::Unverified`].
    pub fn remove(
        &mut self,
        path: &[&str],
        key: &str,
        drop_empty: bool,
    ) -> Result<bool, EditError> {
        let root = self.root()?;
        let mut object = &root;
        for step in path {
            let Some(inner) = last_member(object, step).and_then(|member| member.object.as_ref())
            else {
                return Ok(false);
            };
            object = inner;
        }
        let Some(index) = object.members.iter().rposition(|member| member.key == key) else {
            return Ok(false);
        };

        let mut expected = self.value();
        {
            let mut target = expected.as_object_mut().ok_or(EditError::Unverified)?;
            for step in path {
                target = target
                    .get_mut(*step)
                    .and_then(Value::as_object_mut)
                    .ok_or(EditError::Unverified)?;
            }
            target.remove(key);
        }

        let edited = remove_member(&self.text, object, index);
        self.commit(edited, &expected)?;
        if drop_empty
            && let Some((parent, last)) = path.split_last().map(|(last, parent)| (parent, *last))
            && self
                .get(path)
                .is_some_and(|value| value.as_object().is_some_and(Map::is_empty))
        {
            self.remove(parent, last, false)?;
        }
        Ok(true)
    }

    /// Replaces the text with `edited` if it parses to `expected`.
    fn commit(&mut self, edited: String, expected: &Value) -> Result<(), EditError> {
        let parsed: Value = serde_json::from_str(&edited).map_err(|_| EditError::Unverified)?;
        if parsed != *expected {
            return Err(EditError::Unverified);
        }
        self.text = edited;
        Ok(())
    }

    /// The text with `key: value` added at the end of `object`.
    fn insert(&self, object: &Object, key: &str, value: &Value) -> String {
        let newline = self.newline();
        let unit = self.indent_unit();
        let key_text = serde_json::to_string(key).unwrap_or_default();
        match object.members.last() {
            Some(last) => {
                let multiline = self.text[object.open..last.start].contains('\n');
                if multiline {
                    let indent = self.line_indent(last.start);
                    let rendered = render(value, &indent, &unit, newline);
                    splice(
                        &self.text,
                        last.value.end..last.value.end,
                        &format!(",{newline}{indent}{key_text}: {rendered}"),
                    )
                } else {
                    let rendered = render(value, "", &unit, newline);
                    splice(
                        &self.text,
                        last.value.end..last.value.end,
                        &format!(", {key_text}: {}", flatten(&rendered)),
                    )
                }
            }
            None => {
                let outer = self.line_indent(object.open);
                let indent = format!("{outer}{unit}");
                let rendered = render(value, &indent, &unit, newline);
                splice(
                    &self.text,
                    object.open + 1..object.close,
                    &format!("{newline}{indent}{key_text}: {rendered}{newline}{outer}"),
                )
            }
        }
    }

    /// The root object and its members.
    fn root(&self) -> Result<Object, EditError> {
        let mut parser = Parser {
            bytes: self.text.as_bytes(),
            text: &self.text,
            at: 0,
        };
        parser.skip_space();
        match parser.value()? {
            Some(object) => Ok(object),
            None => Err(EditError::Invalid(
                "the top level is not an object".to_owned(),
            )),
        }
    }

    /// `\r\n` when the file uses it, else `\n`.
    fn newline(&self) -> &'static str {
        if self.text.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        }
    }

    /// The indentation of one level: that of the first member of the root
    /// on its own line, else two spaces.
    fn indent_unit(&self) -> String {
        let Ok(root) = self.root() else {
            return "  ".to_owned();
        };
        root.members
            .first()
            .map(|member| self.line_indent(member.start))
            .filter(|indent| !indent.is_empty())
            .unwrap_or_else(|| "  ".to_owned())
    }

    /// The white space between the start of the line of `at` and `at`, if
    /// that is all there is before it on the line; else what starts the
    /// line.
    fn line_indent(&self, at: usize) -> String {
        let start = self.text[..at].rfind('\n').map_or(0, |newline| newline + 1);
        self.text[start..at]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }
}

/// The member `key` of `object`: the last one, as a parser reads it.
fn last_member<'a>(object: &'a Object, key: &str) -> Option<&'a Member> {
    object.members.iter().rev().find(|member| member.key == key)
}

/// `text` with `range` replaced by `with`.
fn splice(text: &str, range: std::ops::Range<usize>, with: &str) -> String {
    let mut out = String::with_capacity(text.len() + with.len());
    out.push_str(&text[..range.start]);
    out.push_str(with);
    out.push_str(&text[range.end..]);
    out
}

/// `text` without the member at `index` of `object`, and the comma and
/// white space that went with it.
fn remove_member(text: &str, object: &Object, index: usize) -> String {
    let member = &object.members[index];
    if object.members.len() == 1 {
        return splice(text, object.open + 1..object.close, "");
    }
    match object.members.get(index + 1) {
        Some(next) => splice(text, member.start..next.start, ""),
        None => {
            let previous = &object.members[index - 1];
            splice(text, previous.value.end..member.value.end, "")
        }
    }
}

/// `value` as JSON, its inner lines indented from `indent` by `unit`.
fn render(value: &Value, indent: &str, unit: &str, newline: &str) -> String {
    match value {
        Value::Object(map) if !map.is_empty() => {
            let inner = format!("{indent}{unit}");
            let members: Vec<String> = map
                .iter()
                .map(|(key, value)| {
                    format!(
                        "{inner}{}: {}",
                        serde_json::to_string(key).unwrap_or_default(),
                        render(value, &inner, unit, newline)
                    )
                })
                .collect();
            format!(
                "{{{newline}{}{newline}{indent}}}",
                members.join(&format!(",{newline}"))
            )
        }
        Value::Array(items) if !items.is_empty() => {
            let inner = format!("{indent}{unit}");
            let items: Vec<String> = items
                .iter()
                .map(|item| format!("{inner}{}", render(item, &inner, unit, newline)))
                .collect();
            format!(
                "[{newline}{}{newline}{indent}]",
                items.join(&format!(",{newline}"))
            )
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// `rendered` on one line, for an object written on one line.
fn flatten(rendered: &str) -> String {
    serde_json::from_str::<Value>(rendered)
        .ok()
        .and_then(|value| serde_json::to_string(&value).ok())
        .unwrap_or_else(|| rendered.to_owned())
}

/// Finds the objects of a text that is known to be valid JSON.
struct Parser<'a> {
    bytes: &'a [u8],
    text: &'a str,
    at: usize,
}

impl Parser<'_> {
    fn skip_space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn expect(&mut self, byte: u8) -> Result<(), EditError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            Err(EditError::Invalid(format!(
                "expected '{}'",
                char::from(byte)
            )))
        }
    }

    /// Reads a value; returns it when it is an object.
    fn value(&mut self) -> Result<Option<Object>, EditError> {
        match self.peek() {
            Some(b'{') => self.object().map(Some),
            Some(b'[') => {
                self.at += 1;
                self.skip_space();
                if self.peek() == Some(b']') {
                    self.at += 1;
                    return Ok(None);
                }
                loop {
                    self.skip_space();
                    self.value()?;
                    self.skip_space();
                    match self.peek() {
                        Some(b',') => self.at += 1,
                        Some(b']') => {
                            self.at += 1;
                            return Ok(None);
                        }
                        _ => return Err(EditError::Invalid("bad array".to_owned())),
                    }
                }
            }
            Some(b'"') => {
                self.string()?;
                Ok(None)
            }
            Some(_) => {
                while self.peek().is_some_and(|b| {
                    !matches!(b, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                }) {
                    self.at += 1;
                }
                Ok(None)
            }
            None => Err(EditError::Invalid("unexpected end".to_owned())),
        }
    }

    fn object(&mut self) -> Result<Object, EditError> {
        let open = self.at;
        self.expect(b'{')?;
        let mut members = Vec::new();
        self.skip_space();
        if self.peek() == Some(b'}') {
            let close = self.at;
            self.at += 1;
            return Ok(Object {
                open,
                close,
                members,
            });
        }
        loop {
            self.skip_space();
            let start = self.at;
            let key = self.string()?;
            self.skip_space();
            self.expect(b':')?;
            self.skip_space();
            let value_start = self.at;
            let object = self.value()?;
            let value = value_start..self.at;
            members.push(Member {
                key,
                start,
                value,
                object,
            });
            self.skip_space();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    let close = self.at;
                    self.at += 1;
                    return Ok(Object {
                        open,
                        close,
                        members,
                    });
                }
                _ => return Err(EditError::Invalid("bad object".to_owned())),
            }
        }
    }

    /// Reads a string and returns what it says.
    fn string(&mut self) -> Result<String, EditError> {
        let start = self.at;
        self.expect(b'"')?;
        while let Some(byte) = self.peek() {
            self.at += 1;
            match byte {
                b'\\' => self.at += 1,
                b'"' => {
                    return serde_json::from_str(&self.text[start..self.at])
                        .map_err(|error| EditError::Invalid(error.to_string()));
                }
                _ => {}
            }
        }
        Err(EditError::Invalid("unterminated string".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn edit(text: &str, change: impl FnOnce(&mut Document)) -> String {
        let mut document = Document::parse(text).unwrap();
        change(&mut document);
        document.text()
    }

    const SETTINGS: &str = r#"{
    "model": "opus",
    "hooks": {
        "Stop": [{"hooks": [{"type": "command", "command": "mascot stop"}]}]
    },
    "enabledPlugins": {
        "other@market": true
    }
}
"#;

    #[test]
    fn a_member_is_added_where_the_others_are_and_like_them() {
        let text = edit(SETTINGS, |document| {
            document
                .set(&["enabledPlugins"], "termx@termx", &json!(true))
                .unwrap();
        });
        assert_eq!(
            text,
            r#"{
    "model": "opus",
    "hooks": {
        "Stop": [{"hooks": [{"type": "command", "command": "mascot stop"}]}]
    },
    "enabledPlugins": {
        "other@market": true,
        "termx@termx": true
    }
}
"#
        );
    }

    #[test]
    fn a_missing_object_is_created_at_the_end() {
        let text = edit(SETTINGS, |document| {
            document
                .set(
                    &["extraKnownMarketplaces"],
                    "termx",
                    &json!({"source": {"source": "directory", "path": "/x"}}),
                )
                .unwrap();
        });
        assert!(
            text.starts_with(&SETTINGS[..SETTINGS.rfind("\n}").unwrap()]),
            "{text}"
        );
        assert!(
            text.ends_with(
                r#"    },
    "extraKnownMarketplaces": {
        "termx": {
            "source": {
                "path": "/x",
                "source": "directory"
            }
        }
    }
}
"#
            ),
            "{text}"
        );
    }

    #[test]
    fn removing_gives_back_the_original_text() {
        let added = edit(SETTINGS, |document| {
            document
                .set(&["enabledPlugins"], "termx@termx", &json!(true))
                .unwrap();
            document
                .set(
                    &["extraKnownMarketplaces"],
                    "termx",
                    &json!({"source": {"source": "directory", "path": "/x"}}),
                )
                .unwrap();
        });
        let removed = edit(&added, |document| {
            assert!(
                document
                    .remove(&["enabledPlugins"], "termx@termx", true)
                    .unwrap()
            );
            assert!(
                document
                    .remove(&["extraKnownMarketplaces"], "termx", true)
                    .unwrap()
            );
        });
        assert_eq!(removed, SETTINGS);
    }

    #[test]
    fn an_empty_or_new_file_gets_readable_members() {
        for empty in ["{}", "{ }", "{\n}\n"] {
            let text = edit(empty, |document| {
                document
                    .set(&["enabledPlugins"], "termx@termx", &json!(true))
                    .unwrap();
            });
            let value: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(value, json!({"enabledPlugins": {"termx@termx": true}}));
            assert!(
                text.contains("\n  \"enabledPlugins\": {\n    \"termx@termx\": true\n  }\n"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn tabs_crlf_one_liners_and_a_bom_are_kept() {
        let tabs = "{\r\n\t\"a\": 1\r\n}\r\n";
        let text = edit(tabs, |document| {
            document
                .set(&["enabledPlugins"], "x", &json!(true))
                .unwrap();
        });
        assert_eq!(
            text,
            "{\r\n\t\"a\": 1,\r\n\t\"enabledPlugins\": {\r\n\t\t\"x\": true\r\n\t}\r\n}\r\n"
        );

        let one_line = r#"{"a": 1, "enabledPlugins": {"b": false}}"#;
        let text = edit(one_line, |document| {
            document
                .set(&["enabledPlugins"], "x", &json!(true))
                .unwrap();
        });
        assert_eq!(
            text,
            r#"{"a": 1, "enabledPlugins": {"b": false, "x": true}}"#
        );

        let bom = "\u{feff}{\"a\": 1}";
        let text = edit(bom, |document| {
            document.set(&[], "b", &json!(2)).unwrap();
        });
        assert_eq!(text, "\u{feff}{\"a\": 1, \"b\": 2}");
    }

    #[test]
    fn a_value_that_is_there_is_replaced_in_place() {
        let text = edit(SETTINGS, |document| {
            document
                .set(&["enabledPlugins"], "other@market", &json!(false))
                .unwrap();
        });
        assert_eq!(
            text,
            SETTINGS.replace("\"other@market\": true", "\"other@market\": false")
        );
    }

    #[test]
    fn strings_with_braces_and_escapes_do_not_confuse_it() {
        let tricky = r#"{"a\"}": "{\"enabledPlugins\": 1}", "list": [1, {"x": "}"}, "]"], "enabledPlugins": {}}"#;
        let text = edit(tricky, |document| {
            document
                .set(&["enabledPlugins"], "termx@termx", &json!(true))
                .unwrap();
        });
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["enabledPlugins"], json!({"termx@termx": true}));
        assert_eq!(value["a\"}"], json!("{\"enabledPlugins\": 1}"));
        assert_eq!(value["list"], json!([1, {"x": "}"}, "]"]));
    }

    #[test]
    fn what_is_not_an_object_is_refused() {
        for bad in ["", "[]", "not json", "{\"a\": 1,}", "// comment\n{}"] {
            assert!(
                matches!(Document::parse(bad), Err(EditError::Invalid(_))),
                "{bad:?}"
            );
        }
        let mut document = Document::parse(r#"{"enabledPlugins": []}"#).unwrap();
        assert_eq!(
            document.set(&["enabledPlugins"], "x", &json!(true)),
            Err(EditError::NotAnObject("enabledPlugins".to_owned()))
        );
        assert_eq!(document.text(), r#"{"enabledPlugins": []}"#);
    }

    #[test]
    fn removing_what_is_not_there_changes_nothing() {
        let text = edit(SETTINGS, |document| {
            assert!(
                !document
                    .remove(&["enabledPlugins"], "termx@termx", true)
                    .unwrap()
            );
            assert!(!document.remove(&["nothing"], "termx", true).unwrap());
        });
        assert_eq!(text, SETTINGS);
    }

    #[test]
    fn removing_the_first_of_several_members_keeps_the_layout() {
        let text = edit("{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}", |document| {
            document.remove(&[], "a", false).unwrap();
        });
        assert_eq!(text, "{\n  \"b\": 2,\n  \"c\": 3\n}");
        let text = edit("{\n  \"a\": 1,\n  \"b\": 2,\n  \"c\": 3\n}", |document| {
            document.remove(&[], "b", false).unwrap();
        });
        assert_eq!(text, "{\n  \"a\": 1,\n  \"c\": 3\n}");
    }
}
