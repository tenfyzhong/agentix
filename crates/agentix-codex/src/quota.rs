use serde_json::Value;

pub(crate) fn render(value: &Value) -> String {
    let buckets: Vec<_> = value["rateLimitsByLimitId"]
        .as_object()
        .filter(|buckets| !buckets.is_empty())
        .map_or_else(
            || vec![("Codex", &value["rateLimits"])],
            |buckets| {
                buckets
                    .iter()
                    .map(|(name, bucket)| (name.as_str(), bucket))
                    .collect()
            },
        );
    let mut lines = Vec::new();
    for (id, bucket) in buckets {
        let name = bucket["limitName"].as_str().unwrap_or(id);
        for kind in ["primary", "secondary"] {
            let window = &bucket[kind];
            if !window.is_object() {
                continue;
            }
            let duration = window["windowDurationMins"].as_u64().map_or_else(
                || kind.to_owned(),
                |minutes| {
                    if minutes > 0 && minutes.is_multiple_of(1440) {
                        format!("{}d", minutes / 1440)
                    } else if minutes > 0 && minutes.is_multiple_of(60) {
                        format!("{}h", minutes / 60)
                    } else {
                        format!("{minutes}m")
                    }
                },
            );
            let remaining = window["usedPercent"].as_f64().map_or_else(
                || "not reported".into(),
                |used| format!("{}% remaining", (100.0 - used).clamp(0.0, 100.0)),
            );
            let reset = window["resetsAt"]
                .as_i64()
                .and_then(local_time)
                .map_or_else(String::new, |reset| format!(" · resets {reset}"));
            lines.push(format!("- {name} · {duration}: {remaining}{reset}"));
        }
        let credits = &bucket["credits"];
        if credits["unlimited"] == true {
            lines.push(format!("- {name} credits: unlimited"));
        } else if let Some(balance) = credits["balance"].as_str() {
            lines.push(format!("- {name} credits remaining: {balance}"));
        }
    }
    if lines.is_empty() {
        "Quota not reported for this account.".into()
    } else {
        lines.join("\n")
    }
}

fn local_time(seconds: i64) -> Option<String> {
    let instant = time::OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    let offset = time::UtcOffset::local_offset_at(instant).ok()?;
    instant
        .to_offset(offset)
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
}
