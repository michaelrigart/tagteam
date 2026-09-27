use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpliceError {
    #[error("the document is not a JSON object")]
    NotObject,
    #[error("the document is torn or not valid JSON: {0}")]
    Torn(String),
}

const MAX_DEPTH: usize = 512;

struct Member {
    key: String,
    start: usize,
    value_start: usize,
    value_end: usize,
}

struct Object {
    open: usize,
    close: usize,
    members: Vec<Member>,
}

struct Scanner<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> Scanner<'a> {
    fn torn(&self, what: &str) -> SpliceError {
        SpliceError::Torn(format!("{what} at byte {}", self.i))
    }

    fn ws(&mut self) {
        while self.i < self.b.len() && matches!(self.b[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn expect(&mut self, c: u8) -> Result<(), SpliceError> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.torn(&format!("expected '{}'", c as char)))
        }
    }

    fn string(&mut self) -> Result<(), SpliceError> {
        self.expect(b'"')?;
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'"' => return Ok(()),
                b'\\' => match self.peek() {
                    Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => self.i += 1,
                    Some(b'u') => {
                        let hex = self.b.get(self.i + 1..self.i + 5);
                        if !hex.is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit)) {
                            return Err(self.torn("bad \\u escape"));
                        }
                        self.i += 5;
                    }
                    _ => return Err(self.torn("bad escape")),
                },
                0x00..=0x1f => return Err(self.torn("control character in string")),
                _ => {}
            }
        }
        Err(self.torn("unterminated string"))
    }

    fn number(&mut self) -> Result<(), SpliceError> {
        let start = self.i;
        if self.peek() == Some(b'-') {
            self.i += 1;
        }
        let digits = |s: &mut Self| {
            let d = s.i;
            while s.peek().is_some_and(|c| c.is_ascii_digit()) {
                s.i += 1;
            }
            s.i > d
        };
        let int_start = self.i;
        if !digits(self) {
            return Err(self.torn("bad number"));
        }
        if self.b[int_start] == b'0' && self.i - int_start > 1 {
            return Err(self.torn("leading zero in number"));
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            if !digits(self) {
                return Err(self.torn("bad fraction"));
            }
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if !digits(self) {
                return Err(self.torn("bad exponent"));
            }
        }
        debug_assert!(self.i > start);
        Ok(())
    }

    fn literal(&mut self, word: &[u8]) -> Result<(), SpliceError> {
        if self.b[self.i..].starts_with(word) {
            self.i += word.len();
            Ok(())
        } else {
            Err(self.torn("bad literal"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<(), SpliceError> {
        if depth > MAX_DEPTH {
            return Err(self.torn("nesting too deep"));
        }
        match self.peek() {
            Some(b'{') => self.container(b'{', b'}', depth, true),
            Some(b'[') => self.container(b'[', b']', depth, false),
            Some(b'"') => self.string(),
            Some(b't') => self.literal(b"true"),
            Some(b'f') => self.literal(b"false"),
            Some(b'n') => self.literal(b"null"),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(self.torn("expected a value")),
        }
    }

    fn container(
        &mut self,
        open: u8,
        close: u8,
        depth: usize,
        keyed: bool,
    ) -> Result<(), SpliceError> {
        self.expect(open)?;
        self.ws();
        if self.peek() == Some(close) {
            self.i += 1;
            return Ok(());
        }
        loop {
            self.ws();
            if keyed {
                self.string()?;
                self.ws();
                self.expect(b':')?;
                self.ws();
            }
            self.value(depth + 1)?;
            self.ws();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(c) if c == close => {
                    self.i += 1;
                    return Ok(());
                }
                _ => return Err(self.torn("expected ',' or a closing bracket")),
            }
        }
    }
}

/// Consumes the `}` at the scanner's current position, then checks that
/// nothing but whitespace follows. Shared by the empty-object and
/// member-loop exits, which both reach this with `s.peek() == Some(b'}')`.
fn close_object(s: &mut Scanner, open: usize, members: Vec<Member>) -> Result<Object, SpliceError> {
    let close = s.i;
    s.i += 1;
    s.ws();
    if s.i != s.b.len() {
        return Err(s.torn("trailing data"));
    }
    Ok(Object {
        open,
        close,
        members,
    })
}

fn scan(doc: &[u8]) -> Result<Object, SpliceError> {
    std::str::from_utf8(doc).map_err(|e| SpliceError::Torn(format!("invalid UTF-8: {e}")))?;
    let mut s = Scanner { b: doc, i: 0 };
    s.ws();
    match s.peek() {
        Some(b'{') => {}
        Some(b'[' | b'"' | b't' | b'f' | b'n' | b'-' | b'0'..=b'9') => {
            // A complete non-object value is "not an object"; anything else is torn.
            s.value(0)?;
            return Err(SpliceError::NotObject);
        }
        _ => return Err(s.torn("expected '{'")),
    }
    let open = s.i;
    s.i += 1;
    let mut members = Vec::new();
    s.ws();
    if s.peek() != Some(b'}') {
        loop {
            s.ws();
            let start = s.i;
            s.string()?;
            let key: String = serde_json::from_slice(&doc[start..s.i])
                .map_err(|e| SpliceError::Torn(format!("bad key: {e}")))?;
            s.ws();
            s.expect(b':')?;
            s.ws();
            let value_start = s.i;
            s.value(1)?;
            members.push(Member {
                key,
                start,
                value_start,
                value_end: s.i,
            });
            s.ws();
            match s.peek() {
                Some(b',') => s.i += 1,
                Some(b'}') => break,
                _ => return Err(s.torn("expected ',' or '}'")),
            }
        }
    }
    close_object(&mut s, open, members)
}

/// `JSON.stringify(v, null, 2)` layout, nested at `depth` (continuation lines indented).
pub fn render_nested(value: &Value, depth: usize) -> String {
    let pretty = serde_json::to_string_pretty(value).expect("a Value always serializes");
    pretty.replace('\n', &format!("\n{}", "  ".repeat(depth)))
}

pub fn replace_top_level(doc: &[u8], key: &str, value: &Value) -> Result<Vec<u8>, SpliceError> {
    let obj = scan(doc)?;
    let rendered = render_nested(value, 1);
    let mut out = Vec::with_capacity(doc.len() + rendered.len() + key.len() + 8);
    if let Some(m) = obj.members.iter().rev().find(|m| m.key == key) {
        out.extend_from_slice(&doc[..m.value_start]);
        out.extend_from_slice(rendered.as_bytes());
        out.extend_from_slice(&doc[m.value_end..]);
    } else {
        let key_json = serde_json::to_string(key).expect("a string always serializes");
        match obj.members.last() {
            Some(last) => {
                out.extend_from_slice(&doc[..last.value_end]);
                out.extend_from_slice(format!(",\n  {key_json}: {rendered}").as_bytes());
                out.extend_from_slice(&doc[last.value_end..]);
            }
            None => {
                out.extend_from_slice(&doc[..=obj.open]);
                out.extend_from_slice(format!("\n  {key_json}: {rendered}\n").as_bytes());
                out.extend_from_slice(&doc[obj.close..]);
            }
        }
    }
    Ok(out)
}

/// Removes every occurrence of the key, so an earlier duplicate can never resurface.
pub fn remove_top_level(doc: &[u8], key: &str) -> Result<Vec<u8>, SpliceError> {
    let mut out = doc.to_vec();
    loop {
        let obj = scan(&out)?;
        let Some(i) = obj.members.iter().rposition(|m| m.key == key) else {
            return Ok(out);
        };
        let m = &obj.members[i];
        let (cut_start, cut_end) = if i > 0 {
            (obj.members[i - 1].value_end, m.value_end)
        } else if obj.members.len() > 1 {
            (m.start, obj.members[1].start)
        } else {
            (obj.open + 1, obj.close)
        };
        out.drain(cut_start..cut_end);
    }
}

pub fn get_top_level(doc: &[u8], key: &str) -> Result<Option<Value>, SpliceError> {
    let obj = scan(doc)?;
    obj.members
        .iter()
        .rev()
        .find(|m| m.key == key)
        .map(|m| {
            serde_json::from_slice(&doc[m.value_start..m.value_end])
                .map_err(|e| SpliceError::Torn(format!("bad value: {e}")))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const DOC: &str = "{\n  \"numStartups\": 1e400,\n  \"projects\": {\n    \"/x\": {\n      \"oauthAccount\": \"nested, not top-level\"\n    }\n  },\n  \"oauthAccount\": {\n    \"emailAddress\": \"old@a.co\"\n  },\n  \"tipsHistory\": { \"x\": 0.1000 },\n  \"userID\": \"héllo ✓\"\n}\n";

    #[test]
    fn replace_changes_only_the_value_span() {
        let out = replace_top_level(
            DOC.as_bytes(),
            "oauthAccount",
            &json!({"emailAddress": "new@b.co"}),
        )
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        let expected = DOC.replace(
            "{\n    \"emailAddress\": \"old@a.co\"\n  }",
            "{\n    \"emailAddress\": \"new@b.co\"\n  }",
        );
        assert_eq!(out, expected);
        assert!(
            out.contains("1e400")
                && out.contains("0.1000")
                && out.contains("nested, not top-level")
        );
    }

    #[test]
    fn a_missing_key_is_inserted_before_the_closing_brace() {
        let out = replace_top_level(b"{\n  \"a\": 1\n}\n", "k", &json!([1, {"b": null}])).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "{\n  \"a\": 1,\n  \"k\": [\n    1,\n    {\n      \"b\": null\n    }\n  ]\n}\n"
        );
    }

    #[test]
    fn an_empty_object_gets_its_first_member() {
        let out = replace_top_level(b"{}", "k", &json!("v")).unwrap();
        assert_eq!(out, b"{\n  \"k\": \"v\"\n}");
    }

    #[test]
    fn crlf_documents_keep_their_other_bytes() {
        let doc = b"{\r\n  \"a\": 1,\r\n  \"k\": 2\r\n}\r\n";
        let out = replace_top_level(doc, "k", &json!(3)).unwrap();
        assert_eq!(out, b"{\r\n  \"a\": 1,\r\n  \"k\": 3\r\n}\r\n");
    }

    #[test]
    fn remove_takes_one_adjoining_comma() {
        let doc = b"{\n  \"a\": 1,\n  \"k\": 2,\n  \"z\": 3\n}";
        assert_eq!(
            remove_top_level(doc, "k").unwrap(),
            b"{\n  \"a\": 1,\n  \"z\": 3\n}"
        );
        assert_eq!(
            remove_top_level(doc, "a").unwrap(),
            b"{\n  \"k\": 2,\n  \"z\": 3\n}"
        );
        assert_eq!(
            remove_top_level(doc, "z").unwrap(),
            b"{\n  \"a\": 1,\n  \"k\": 2\n}"
        );
        assert_eq!(remove_top_level(b"{\"k\": 1}", "k").unwrap(), b"{}");
        assert_eq!(remove_top_level(doc, "missing").unwrap(), doc.to_vec());
        // Every duplicate goes, so an earlier value cannot come back.
        assert_eq!(
            remove_top_level(b"{\"k\": 1, \"a\": 0, \"k\": 2}", "k").unwrap(),
            b"{\"a\": 0}"
        );
    }

    #[test]
    fn duplicate_keys_replace_the_last_like_json_parse() {
        let out = replace_top_level(b"{\"k\": 1, \"k\": 2}", "k", &json!(9)).unwrap();
        assert_eq!(out, b"{\"k\": 1, \"k\": 9}");
    }

    #[test]
    fn get_reads_a_top_level_value() {
        assert_eq!(
            get_top_level(DOC.as_bytes(), "userID").unwrap(),
            Some(json!("héllo ✓"))
        );
        assert_eq!(get_top_level(DOC.as_bytes(), "nope").unwrap(), None);
    }

    #[test]
    fn torn_and_non_object_documents_are_refused() {
        for torn in [
            &b""[..],
            b"{",
            b"{\"a\": 1",
            b"{\"a\": 1,}",
            b"{\"a\" 1}",
            b"{\"a\": tru}",
            b"{} x",
            b"{\"a\": \"\x01\"}",
            // A malformed unrelated value refuses the whole write, too.
            b"{\"x\": 01, \"oauthAccount\": {}}",
            b"{\"x\": \"\\q\", \"oauthAccount\": {}}",
            b"{\"x\": \"\\u12\"}",
            b"{\"x\": \"\xff\"}",
        ] {
            assert!(
                matches!(
                    replace_top_level(torn, "k", &json!(1)),
                    Err(SpliceError::Torn(_))
                ),
                "{:?}",
                String::from_utf8_lossy(torn)
            );
        }
        for not_obj in [&b"[]"[..], b"\"s\"", b"12", b"null"] {
            assert!(matches!(
                replace_top_level(not_obj, "k", &json!(1)),
                Err(SpliceError::NotObject)
            ));
        }
    }

    #[test]
    fn trailing_data_after_a_non_empty_object_is_torn() {
        for torn in [&b"{\"a\":1} x"[..], b"{\"a\": 1}\0\0\0"] {
            assert!(matches!(
                replace_top_level(torn, "k", &json!(1)),
                Err(SpliceError::Torn(_))
            ));
            assert!(matches!(
                remove_top_level(torn, "a"),
                Err(SpliceError::Torn(_))
            ));
            assert!(matches!(
                get_top_level(torn, "a"),
                Err(SpliceError::Torn(_))
            ));
        }
    }

    #[test]
    fn unterminated_input_is_torn() {
        for torn in [&b"{\"a\": \"x"[..], b"{\"a\": [1}", b"{\"a\": {\"b\": 1}"] {
            assert!(matches!(
                replace_top_level(torn, "k", &json!(1)),
                Err(SpliceError::Torn(_))
            ));
        }
    }

    #[test]
    fn remove_top_level_handles_adjacent_duplicates() {
        for (doc, expected) in [
            (&b"{\"k\":1,\"k\":2}"[..], &b"{}"[..]),
            (b"{\"k\":1,\"k\":2,\"a\":3}", b"{\"a\":3}"),
            (
                b"{\"k\":1,\"a\":2,\"k\":3,\"b\":4,\"k\":5}",
                b"{\"a\":2,\"b\":4}",
            ),
        ] {
            let out = remove_top_level(doc, "k").unwrap();
            assert_eq!(out, expected);
            serde_json::from_slice::<Value>(&out).expect("result must still be valid JSON");
        }
    }

    #[test]
    fn torn_errors_never_echo_the_input() {
        let doc = b"{\"primaryApiKey\": \"sk-ant-SECRET\"";
        let err = replace_top_level(doc, "k", &json!(1)).unwrap_err();
        assert!(!format!("{err}").contains("sk-ant"));
        assert!(!format!("{err:?}").contains("sk-ant"));
    }

    #[test]
    fn deep_nesting_is_torn_not_a_stack_overflow() {
        let doc = format!("{{\"a\": {}{}}}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(matches!(
            replace_top_level(doc.as_bytes(), "k", &json!(1)),
            Err(SpliceError::Torn(_))
        ));
    }

    #[test]
    fn large_documents_splice_quickly() {
        let mut doc = String::from("{\n  \"projects\": {");
        for i in 0..20_000 {
            doc.push_str(&format!(
                "\n    \"/p/{i}\": {{ \"allowedTools\": [], \"history\": [\"x\"] }},"
            ));
        }
        doc.pop();
        doc.push_str("\n  }\n}\n");
        let start = std::time::Instant::now();
        let out = replace_top_level(doc.as_bytes(), "oauthAccount", &json!({"e": 1})).unwrap();
        assert!(start.elapsed() < std::time::Duration::from_millis(500));
        // Everything up to the end of `projects` is untouched; the new member follows it.
        let untouched = &doc.as_bytes()[..doc.len() - "\n}\n".len()];
        assert!(out.starts_with(untouched));
    }
}
