use anyhow::{Context, Result};

pub(crate) fn new_id(created_at: i64) -> Result<String> {
    timestamp_id(created_at, &uuid::Uuid::now_v7().simple().to_string())
}

pub(crate) fn timestamp_id(created_at: i64, suffix: &str) -> Result<String> {
    let instant = time::OffsetDateTime::from_unix_timestamp(created_at)?;
    let offset = time::UtcOffset::local_offset_at(instant)
        .context("cannot resolve the computer's local time zone for memory ID")?;
    let local = instant.to_offset(offset);
    Ok(format!(
        "mem_{:02}{:02}{:02}{:02}{:02}{:02}_{suffix}",
        local.year().rem_euclid(100),
        u8::from(local.month()),
        local.day(),
        local.hour(),
        local.minute(),
        local.second()
    ))
}

pub(crate) fn legacy_suffix(id: &str) -> Option<&str> {
    id.strip_prefix("mem_").filter(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    })
}
