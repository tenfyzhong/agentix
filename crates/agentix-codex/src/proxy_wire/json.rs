//! Bounded incremental JSON validation and lifecycle metadata projection.
//! No history, tool output, array elements, or unrecognized fields are retained.
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 128;
const MAX_TOKEN: usize = 4096;

#[derive(Clone, Copy, PartialEq)]
enum Node {
    Root,
    Result,
    Thread,
    Source,
    Params,
    Client,
    Leaf,
    Skip,
}
impl Node {
    #[allow(clippy::match_same_arms)] // Keep the protocol field map grouped by parent.
    fn child(self, key: &str) -> Self {
        match (self, key) {
            (Self::Root, "id" | "method" | "error") => Self::Leaf,
            (Self::Root, "result") => Self::Result,
            (Self::Root, "params") => Self::Params,
            (Self::Result, "thread") => Self::Thread,
            (Self::Result, "status") => Self::Leaf,
            (Self::Thread, "id" | "ephemeral") => Self::Leaf,
            (Self::Thread, "source") => Self::Source,
            (Self::Source, "subAgent") => Self::Leaf,
            (Self::Params, "threadId" | "ephemeral") => Self::Leaf,
            (Self::Params, "clientInfo") => Self::Client,
            (Self::Client, "name") => Self::Leaf,
            _ => Self::Skip,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    KeyOrEnd,
    Key,
    Colon,
    ValueOrEnd,
    Value,
    CommaOrEnd,
}
struct Container {
    object: bool,
    phase: Phase,
    node: Node,
    key: String,
    fields: Map<String, Value>,
}
#[derive(Default)]
#[allow(clippy::struct_excessive_bools)] // Independent token selection and escape-state flags.
struct StringToken {
    key: bool,
    capture: bool,
    raw: Vec<u8>,
    escape: bool,
    digits: u8,
    code: u16,
    high: bool,
    low_escape: u8,
}
impl StringToken {
    fn byte(&mut self, b: u8) -> Result<bool, ()> {
        if self.capture {
            if self.raw.len() == MAX_TOKEN {
                return Err(());
            }
            self.raw.push(b);
        }
        if self.digits > 0 {
            let digit = u16::try_from(char::from(b).to_digit(16).ok_or(())?).map_err(|_| ())?;
            self.code = (self.code << 4) | digit;
            self.digits -= 1;
            if self.digits == 0 {
                if self.high {
                    if !(0xdc00..=0xdfff).contains(&self.code) {
                        return Err(());
                    }
                    self.high = false;
                } else if (0xd800..=0xdbff).contains(&self.code) {
                    self.high = true;
                    self.low_escape = 2;
                } else if (0xdc00..=0xdfff).contains(&self.code) {
                    return Err(());
                }
            }
        } else if self.low_escape > 0 {
            if b != if self.low_escape == 2 { b'\\' } else { b'u' } {
                return Err(());
            }
            self.low_escape -= 1;
            if self.low_escape == 0 {
                self.digits = 4;
                self.code = 0;
            }
        } else if self.escape {
            self.escape = false;
            match b {
                b'u' => {
                    self.digits = 4;
                    self.code = 0;
                }
                b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => {}
                _ => return Err(()),
            }
        } else {
            match b {
                b'"' => return Ok(true),
                b'\\' => self.escape = true,
                0..=31 => return Err(()),
                _ => {}
            }
        }
        Ok(false)
    }
}

enum Token {
    String(StringToken),
    Atom { raw: Vec<u8>, node: Node },
}

#[derive(Default)]
pub(super) struct Projection {
    stack: Vec<Container>,
    token: Option<Token>,
    result: Option<Value>,
    invalid: bool,
    utf8_tail: Vec<u8>,
}
impl Projection {
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.invalid {
            return;
        }
        if self.validate_utf8(bytes).is_err() || self.parse(bytes).is_err() {
            self.invalid = true;
            self.stack.clear();
            self.token = None;
            self.result = None;
        }
    }
    fn validate_utf8(&mut self, mut bytes: &[u8]) -> Result<(), ()> {
        if !self.utf8_tail.is_empty() {
            while !bytes.is_empty() && self.utf8_tail.len() < 4 {
                self.utf8_tail.push(bytes[0]);
                bytes = &bytes[1..];
                match std::str::from_utf8(&self.utf8_tail) {
                    Ok(_) => {
                        self.utf8_tail.clear();
                        break;
                    }
                    Err(e) if e.error_len().is_some() => return Err(()),
                    Err(_) => {}
                }
            }
            if !self.utf8_tail.is_empty() {
                return Ok(());
            }
        }
        if let Err(e) = std::str::from_utf8(bytes) {
            if e.error_len().is_some() {
                return Err(());
            }
            self.utf8_tail.extend_from_slice(&bytes[e.valid_up_to()..]);
        }
        Ok(())
    }
    fn node(&self) -> Node {
        self.stack.last().map_or(Node::Root, |c| {
            if c.object {
                c.node.child(&c.key)
            } else {
                Node::Skip
            }
        })
    }
    fn value(&mut self, value: Value) -> Result<(), ()> {
        if let Some(c) = self.stack.last_mut() {
            if !matches!(c.phase, Phase::Value | Phase::ValueOrEnd) {
                return Err(());
            }
            if c.object && c.node.child(&c.key) != Node::Skip {
                c.fields.insert(std::mem::take(&mut c.key), value);
            }
            c.phase = Phase::CommaOrEnd;
        } else if self.result.replace(value).is_some() {
            return Err(());
        }
        Ok(())
    }
    #[allow(clippy::too_many_lines)] // One incremental grammar transition loop.
    fn parse(&mut self, mut bytes: &[u8]) -> Result<(), ()> {
        while !bytes.is_empty() {
            if let Some(Token::String(s)) = &mut self.token {
                if !s.capture && !s.escape && s.digits == 0 && s.low_escape == 0 {
                    let n = memchr::memchr2(b'"', b'\\', bytes).unwrap_or(bytes.len());
                    // A reduction vectorizes; a short-circuit `any` adds a
                    // scalar branch per byte on large ordinary text fields.
                    if bytes[..n]
                        .iter()
                        .fold(0u8, |invalid, b| invalid | u8::from(*b < 32))
                        != 0
                    {
                        return Err(());
                    }
                    bytes = &bytes[n..];
                    if bytes.is_empty() {
                        break;
                    }
                }
                let done = s.byte(bytes[0])?;
                bytes = &bytes[1..];
                if done {
                    let Some(Token::String(s)) = self.token.take() else {
                        unreachable!()
                    };
                    if s.key {
                        let c = self.stack.last_mut().ok_or(())?;
                        c.key = if s.capture {
                            serde_json::from_slice(&s.raw).map_err(|_| ())?
                        } else {
                            String::new()
                        };
                        c.phase = Phase::Colon;
                    } else {
                        let value = if s.capture {
                            serde_json::from_slice(&s.raw).map_err(|_| ())?
                        } else {
                            Value::Null
                        };
                        self.value(value)?;
                    }
                }
                continue;
            }
            if let Some(Token::Atom { raw, .. }) = &mut self.token {
                if !matches!(bytes[0], b' ' | b'\r' | b'\n' | b'\t' | b',' | b'}' | b']') {
                    if raw.len() >= MAX_TOKEN {
                        return Err(());
                    }
                    raw.push(bytes[0]);
                    bytes = &bytes[1..];
                    continue;
                }
                self.finish_atom()?;
                continue;
            }
            let b = bytes[0];
            bytes = &bytes[1..];
            if matches!(b, b' ' | b'\r' | b'\n' | b'\t') {
                continue;
            }
            if let Some(c) = self.stack.last_mut() {
                match c.phase {
                    Phase::KeyOrEnd | Phase::Key => {
                        if b == b'"' {
                            self.token = Some(Token::String(StringToken {
                                key: true,
                                capture: c.node != Node::Skip && c.node != Node::Leaf,
                                raw: vec![b'"'],
                                ..Default::default()
                            }));
                            continue;
                        }
                        if b != b'}' || c.phase != Phase::KeyOrEnd {
                            return Err(());
                        }
                    }
                    Phase::Colon => {
                        if b != b':' {
                            return Err(());
                        }
                        c.phase = Phase::Value;
                        continue;
                    }
                    Phase::CommaOrEnd => {
                        if b == b',' {
                            c.phase = if c.object { Phase::Key } else { Phase::Value };
                            continue;
                        }
                        if b != if c.object { b'}' } else { b']' } {
                            return Err(());
                        }
                    }
                    Phase::Value | Phase::ValueOrEnd => {}
                }
                if b == b'}' || b == b']' {
                    if b != if c.object { b'}' } else { b']' }
                        || !matches!(
                            c.phase,
                            Phase::KeyOrEnd | Phase::ValueOrEnd | Phase::CommaOrEnd
                        )
                    {
                        return Err(());
                    }
                    let c = self.stack.pop().unwrap();
                    self.value(if c.object {
                        Value::Object(c.fields)
                    } else {
                        Value::Array(Vec::new())
                    })?;
                    continue;
                }
            } else if self.result.is_some() {
                return Err(());
            }
            let node = self.node();
            match b {
                b'{' | b'[' => {
                    if self.stack.len() >= MAX_DEPTH {
                        return Err(());
                    }
                    self.stack.push(Container {
                        object: b == b'{',
                        phase: if b == b'{' {
                            Phase::KeyOrEnd
                        } else {
                            Phase::ValueOrEnd
                        },
                        node,
                        key: String::new(),
                        fields: Map::new(),
                    });
                }
                b'"' => {
                    self.token = Some(Token::String(StringToken {
                        capture: node != Node::Skip,
                        raw: vec![b'"'],
                        ..Default::default()
                    }));
                }
                b'-' | b'0'..=b'9' | b't' | b'f' | b'n' => {
                    self.token = Some(Token::Atom { raw: vec![b], node });
                }
                _ => return Err(()),
            }
        }
        Ok(())
    }
    fn finish_atom(&mut self) -> Result<(), ()> {
        let Some(Token::Atom { raw, node }) = self.token.take() else {
            return Ok(());
        };
        let value = if node == Node::Skip {
            serde_json::from_slice::<serde::de::IgnoredAny>(&raw).map_err(|_| ())?;
            Value::Null
        } else {
            serde_json::from_slice(&raw).map_err(|_| ())?
        };
        self.value(value)
    }
    pub fn finish(mut self) -> Option<String> {
        if self.invalid
            || !self.utf8_tail.is_empty()
            || self.finish_atom().is_err()
            || self.token.is_some()
            || !self.stack.is_empty()
        {
            return None;
        }
        let value = self.result?;
        serde_json::to_string(&value).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn project(text: &[u8], chunk: usize) -> Option<Value> {
        let mut p = Projection::default();
        for bytes in text.chunks(chunk) {
            p.feed(bytes);
        }
        p.finish().map(|s| serde_json::from_str(&s).unwrap())
    }
    #[test]
    fn validates_every_split_and_preserves_only_lifecycle_metadata() {
        let text = br#"{"ignored":[1,-2.5e+3,true,false,null,{"x":"\uD83D\uDE00"}],"result":{"thread":{"turns":[],"i\u0064":"new","ephemeral":false,"source":{"subAgent":{"id":"helper"}}}},"id":"r"}"#;
        let expected = serde_json::json!({"result":{"thread":{"id":"new","ephemeral":false,"source":{"subAgent":{}}}},"id":"r"});
        for chunk in 1..text.len() {
            assert_eq!(
                project(text, chunk),
                Some(expected.clone()),
                "chunk {chunk}"
            );
        }
    }
    #[test]
    fn rejects_malformed_json_even_inside_discarded_fields() {
        for text in [
            r#"{"id":1,"x":"\uD800"}"#,
            r#"{"id":1,"x":"\uDC00"}"#,
            r#"{"id":1,"x":"\q"}"#,
            r#"{"id":1,"x":01}"#,
            r#"{"id":1,"x":[1,]}"#,
            r#"{"id":1,}"#,
            r#"{"id":1,"x":{"a":}}"#,
            r#"{"id":1}{}"#,
            r#"{"id":1,"x":"unfinished"#,
            r#"{"id":1,"x":[true false]}"#,
        ] {
            for chunk in 1..8 {
                assert!(project(text.as_bytes(), chunk).is_none(), "{text}");
            }
        }
    }
    #[test]
    fn utf8_is_validated_across_discarded_string_chunks() {
        let text = "{\"id\":1,\"ignored\":\"雪🙂\"}";
        for chunk in 1..8 {
            assert_eq!(
                project(text.as_bytes(), chunk),
                Some(serde_json::json!({"id":1}))
            );
        }
        for bytes in [
            b"{\"x\":\"\xff\"}".as_slice(),
            b"{\"x\":\"\xe9\"}",
            b"{\"x\":\"\xc0\x80\"}",
        ] {
            assert!(project(bytes, 1).is_none());
        }
    }

    #[test]
    fn discarded_strings_reject_raw_control_bytes_at_scan_boundaries() {
        for offset in [0, 15, 16, 31, 32, 63, 64, 127] {
            for control in 0..32 {
                let mut text = br#"{"id":1,"ignored":""#.to_vec();
                text.extend(std::iter::repeat_n(b'x', offset));
                text.push(control);
                text.extend_from_slice(br#"","result":{"thread":{"id":"bad"}}}"#);
                for chunk in [1, 17, 128] {
                    assert!(project(&text, chunk).is_none());
                }
            }
        }
    }
    #[test]
    fn rejects_metadata_and_nesting_beyond_fixed_budgets() {
        let text = serde_json::json!({"id":"x".repeat(MAX_TOKEN+1)}).to_string();
        assert!(project(text.as_bytes(), 97).is_none());
        assert!(
            project(
                format!("{}0{}", "[".repeat(129), "]".repeat(129)).as_bytes(),
                13
            )
            .is_none()
        );
    }
}
