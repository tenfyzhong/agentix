use std::collections::BTreeSet;

use anyhow::{Context, Result};

pub(crate) fn task_path(state: &crate::Snapshot, task: &crate::Task) -> Result<String> {
    let project = &state.projects[state.project_index(&task.project_id)?];
    let filename = numbered_name(&task.name, task.created_at, task.sequence)?;
    Ok(format!("Projects/{}/Tasks/{filename}.md", project.key))
}

/// Allocate independently for each entity type/project (filtered by the caller).
pub(crate) fn next_sequence(
    created_at: i64,
    existing: impl Iterator<Item = (i64, u64)>,
) -> Result<u64> {
    existing
        .filter(|(created, _)| created.div_euclid(86_400) == created_at.div_euclid(86_400))
        .map(|(_, sequence)| sequence)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .context("document sequence exhausted")
}

pub(crate) fn numbered_name(name: &str, created_at: i64, sequence: u64) -> Result<String> {
    let date = time::OffsetDateTime::from_unix_timestamp(created_at)?;
    Ok(format!(
        "{:02}{:02}{:02}-{sequence:04}-{name}",
        date.year().rem_euclid(100),
        u8::from(date.month()),
        date.day(),
    ))
}

/// A portable, readable filename stem, including non-ASCII project/task names.
pub(crate) fn short_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| {
            if c.is_control() || r#"/\:*?\"<>|[]#^"#.contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let name: String = name.chars().take(48).collect();
    let name = name.trim_matches([' ', '.']);
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if name.is_empty() {
        "Untitled".into()
    } else if [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ]
    .contains(&stem.as_str())
    {
        format!("{name}-1")
    } else {
        name.into()
    }
}

pub(crate) fn unique_name<'a>(title: &str, existing: impl Iterator<Item = &'a str>) -> String {
    let base = short_name(title);
    let used: BTreeSet<_> = existing.map(str::to_lowercase).collect();
    let mut name = base.clone();
    let mut suffix = 2;
    while used.contains(&name.to_lowercase()) {
        name = format!("{base}-{suffix}");
        suffix += 1;
    }
    name
}
