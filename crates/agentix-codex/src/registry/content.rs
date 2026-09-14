//! Read-only notification routing hints. Ownership mutations and interactive
//! requests still require complete JSON validation in the registry.
use std::borrow::Cow;

pub(super) fn session_hint(text: &str) -> Option<String> {
    Cursor(text).route(false).map(Cow::into_owned)
}

struct Cursor<'a>(&'a str);

impl<'a> Cursor<'a> {
    fn whitespace(&mut self) {
        self.0 = self.0.trim_start_matches(|c: char| c.is_ascii_whitespace());
    }

    fn take(&mut self, token: char) -> Option<()> {
        self.whitespace();
        self.0 = self.0.strip_prefix(token)?;
        Some(())
    }

    // Skip long string bodies with a vectorized byte search, including when the
    // sender places delta/text before threadId. Decode only routing fields.
    fn string_span(&mut self) -> Option<&'a str> {
        self.whitespace();
        let original = self.0;
        let mut offset = 1;
        original.strip_prefix('"')?;
        loop {
            let tail = original.get(offset..)?;
            // The standard library is preoptimized in debug builds. memchr's
            // inlined SIMD path needs optimization to outperform it.
            let quote = if cfg!(debug_assertions) {
                tail.find('"')
            } else {
                memchr::memchr(b'"', tail.as_bytes())
            }?;
            let index = offset + quote;
            let escapes = original.as_bytes()[..index]
                .iter()
                .rev()
                .take_while(|byte| **byte == b'\\')
                .count();
            if escapes % 2 == 0 {
                self.0 = &original[index + 1..];
                return Some(&original[..=index]);
            }
            offset = index + 1;
        }
    }

    fn string(&mut self) -> Option<Cow<'a, str>> {
        let span = self.string_span()?;
        if span.as_bytes().contains(&b'\\') {
            serde_json::from_str::<String>(span).ok().map(Cow::Owned)
        } else {
            Some(Cow::Borrowed(&span[1..span.len() - 1]))
        }
    }

    fn skip(&mut self, depth: usize) -> Option<()> {
        if depth > 64 {
            return None;
        }
        self.whitespace();
        match self.0.as_bytes().first()? {
            b'"' => {
                self.string_span()?;
            }
            b'{' => {
                self.take('{')?;
                self.whitespace();
                if self.0.starts_with('}') {
                    return self.take('}');
                }
                loop {
                    self.string_span()?;
                    self.take(':')?;
                    self.skip(depth + 1)?;
                    self.whitespace();
                    if self.0.starts_with('}') {
                        return self.take('}');
                    }
                    self.take(',')?;
                }
            }
            b'[' => {
                self.take('[')?;
                self.whitespace();
                if self.0.starts_with(']') {
                    return self.take(']');
                }
                loop {
                    self.skip(depth + 1)?;
                    self.whitespace();
                    if self.0.starts_with(']') {
                        return self.take(']');
                    }
                    self.take(',')?;
                }
            }
            _ => {
                let end = self.0.find([',', '}', ']'])?;
                serde_json::from_str::<serde::de::IgnoredAny>(&self.0[..end]).ok()?;
                self.0 = &self.0[end..];
            }
        }
        Some(())
    }

    fn route(&mut self, params: bool) -> Option<Cow<'a, str>> {
        self.take('{')?;
        loop {
            let key = self.string()?;
            self.take(':')?;
            match (params, key.as_ref()) {
                (false, "id") => return None,
                (false, "params") => return self.route(true),
                (true, "threadId") => return self.string(),
                _ => self.skip(0)?,
            }
            self.take(',')?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::session_hint;

    #[test]
    fn routing_skips_large_escaped_unicode_payloads_at_scan_boundaries() {
        for padding in 0..65 {
            for suffix in ["\\", "\"", "\\\"", "🙂"] {
                let payload = format!("{}{}{}", "x".repeat(padding), "雪\"\\".repeat(4096), suffix);
                let frame = serde_json::json!({
                    "method": "item/agentMessage/delta",
                    "params": {"delta": payload, "threadId": "target"},
                })
                .to_string();
                assert_eq!(session_hint(&frame).as_deref(), Some("target"));
            }
        }
    }

    #[test]
    fn routing_ignores_nested_ids_and_decodes_escaped_keys() {
        for frame in [
            r#"{"method":"item/delta","params":{"threadId":"target","delta":"x"}}"#,
            r#"{"params":{"nested":{"threadId":"wrong"},"array":[true,null,3,{},[]],"threadId":"target"},"method":"item/delta"}"#,
            r#"{"method":"item/delta","params":{"delta":"text \\\"threadId\\\": \\\"wrong\\\"","threadId":"target"}}"#,
            r#"{"method":"item/delta","params":{"thread\u0049d":"targ\u0065t"}}"#,
        ] {
            assert_eq!(session_hint(frame).as_deref(), Some("target"), "{frame}");
        }
        for frame in [
            "malformed",
            r#"{"id":1,"method":"item/tool/requestUserInput","params":{"threadId":"target"}}"#,
            r#"{"params":{"nested":{"threadId":"wrong"}}}"#,
            r#"{"params":{"threadId":null}}"#,
            r#"{"params":{"delta":"unfinished"#,
        ] {
            assert_eq!(session_hint(frame), None, "{frame}");
        }
    }
}
