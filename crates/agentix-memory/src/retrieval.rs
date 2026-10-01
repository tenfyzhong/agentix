use std::{collections::BTreeSet, sync::LazyLock};

use anyhow::{Result, ensure};
use jieba_rs::Jieba;
use sha2::{Digest, Sha256};

static TOKENIZER: LazyLock<Jieba> = LazyLock::new(Jieba::new);

pub(crate) fn digest(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(crate) fn project_token(project: &str) -> String {
    format!("p{}", digest(project))
}

pub(crate) fn tokens(text: &str) -> Vec<String> {
    let mut terms = BTreeSet::new();
    for word in TOKENIZER.cut_for_search(text, false) {
        let word = word.word.to_lowercase();
        if word.chars().any(char::is_alphanumeric) {
            terms.insert(word);
        }
    }
    // Preserve whole identifiers and add components for snake_case and camelCase.
    for identifier in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        if identifier.is_empty() {
            continue;
        }
        terms.insert(identifier.to_lowercase());
        let mut component = String::new();
        for c in identifier.chars() {
            if c == '_' || (c.is_ascii_uppercase() && !component.is_empty()) {
                if !component.is_empty() {
                    terms.insert(std::mem::take(&mut component));
                }
                if c == '_' {
                    continue;
                }
            }
            component.push(c.to_ascii_lowercase());
        }
        if !component.is_empty() {
            terms.insert(component);
        }
    }
    terms.into_iter().collect()
}

pub(crate) fn index_text(text: &str) -> String {
    tokens(text).join(" ")
}

pub(crate) fn query(project: &str, text: &str) -> Result<Option<String>> {
    ensure!(
        text.len() <= 4096,
        "invalid: memory query exceeds 4096 bytes"
    );
    let terms = tokens(text);
    if terms.is_empty() {
        return Ok(None);
    }
    // Host prompts have a character budget, not a term budget. Keep recall
    // bounded without disabling lexical and semantic retrieval for long prompts.
    let quoted: Vec<_> = terms
        .iter()
        .take(128)
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect();
    Ok(Some(format!(
        "project_token : \"{}\" AND {{title body tags scope}} : ({})",
        project_token(project),
        quoted.join(" OR ")
    )))
}
