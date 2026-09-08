use std::{collections::BTreeSet, ops::Range, path::PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use crate::projection::atomic_write;
use crate::{InboxEntry, InboxStatus, Project, Service, Snapshot, WriteOptions, new_id};

const END: &str = "<!-- taskcli:inbox:end -->";
const ID: &str = " <!-- taskcli:entry:";
const LEGACY_RECEIPT: &str = "  <!-- taskcli:entry-state -->";
const STATE: &str = " <!-- taskcli:entry-state ";

struct ParsedEntry {
    id: Option<String>,
    content: String,
    cancelled: bool,
    span: Range<usize>,
    header_end: usize,
}

fn start(project: &str) -> String {
    format!("<!-- taskcli:inbox:start project={project} -->")
}

fn strip_inline_receipt(header: &str) -> Result<&str> {
    if let Some((header, receipt)) = header.split_once(STATE) {
        ensure!(
            receipt.contains(" -->"),
            "invalid: Inbox entry state marker"
        );
        Ok(header)
    } else {
        Ok(header)
    }
}

fn parse(source: &str, project: &str) -> Result<Vec<ParsedEntry>> {
    let marker = start(project);
    ensure!(
        source.matches(&marker).count() == 1 && source.matches(END).count() == 1,
        "invalid: Inbox document markers missing or duplicated"
    );
    let begin = source.find(&marker).unwrap() + marker.len();
    let end = source.find(END).unwrap();
    ensure!(begin < end, "invalid: reversed Inbox markers");
    let mut lines = Vec::new();
    let mut offset = begin;
    for line in source[begin..end].split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let (offset, line) = lines[i];
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent <= 3 && (trimmed.starts_with("```") || trimmed.starts_with("~~~")) {
            let character = trimmed.chars().next().unwrap();
            let count = trimmed.chars().take_while(|c| *c == character).count();
            if let Some((open, size)) = fence {
                if open == character && count >= size && trimmed[count..].trim().is_empty() {
                    fence = None;
                }
            } else {
                fence = Some((character, count));
            }
            i += 1;
            continue;
        }
        if fence.is_some() || !line.starts_with("- [") {
            i += 1;
            continue;
        }
        ensure!(
            line.len() >= 6
                && matches!(
                    line.as_bytes()[3],
                    b' ' | b'/' | b'r' | b'p' | b'x' | b'X' | b'-'
                )
                && &line[4..6] == "] ",
            "invalid: Inbox checkbox"
        );
        let header = strip_inline_receipt(line[6..].trim_end_matches(['\r', '\n']))?;
        let (title, id) = if let Some((title, suffix)) = header.rsplit_once(ID) {
            let id = suffix
                .strip_suffix(" -->")
                .context("invalid: Inbox entry ID marker")?;
            ensure!(
                id.strip_prefix("inbox_")
                    .is_some_and(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit())),
                "invalid: Inbox entry ID"
            );
            ensure!(
                seen.insert(id.to_owned()),
                "conflict: duplicate Inbox entry ID"
            );
            (title.trim_end(), Some(id.to_owned()))
        } else {
            (header, None)
        };
        ensure!(!title.trim().is_empty(), "invalid: empty Inbox title");
        let mut content = title.to_owned();
        let mut end_offset = offset + line.len();
        let mut j = i + 1;
        while j < lines.len() {
            let (next_offset, next) = lines[j];
            if !next.trim().is_empty() && !next.starts_with("  ") && !next.starts_with('\t') {
                break;
            }
            if !next.starts_with(LEGACY_RECEIPT) {
                content.push('\n');
                content.push_str(
                    next.strip_prefix("  ")
                        .unwrap_or(next)
                        .trim_end_matches(['\r', '\n']),
                );
            }
            end_offset = next_offset + next.len();
            j += 1;
        }
        result.push(ParsedEntry {
            id,
            content: content.trim_end().into(),
            // A canonical [-] receipt can be left behind by an interrupted
            // reopen projection. Only a changed checkbox requests cancellation.
            cancelled: line.as_bytes()[3] == b'-'
                && !line.contains(" <!-- taskcli:entry-state CANCELLED "),
            span: offset..end_offset,
            header_end: offset + line.trim_end_matches(['\r', '\n']).len(),
        });
        i = j;
    }
    ensure!(fence.is_none(), "invalid: unclosed Inbox code fence");
    Ok(result)
}

fn checked_write(path: &std::path::Path, expected: &str, next: &str) -> Result<()> {
    ensure!(
        std::fs::read_to_string(path)? == expected,
        "conflict: Inbox changed during synchronization; retry"
    );
    atomic_write(path, next)
}

impl Service {
    pub fn inbox_path(&self, project: &Project) -> Result<PathBuf> {
        self.safe_path(&format!("Projects/{}/Inbox.md", project.key))
    }

    pub(crate) async fn reconcile_inboxes_locked(
        &self,
        projects: Option<&BTreeSet<String>>,
        deferred_status: Option<&str>,
        publish: bool,
    ) -> Result<()> {
        let selected = if let Some(ids) = projects {
            let mut selected = Vec::new();
            for id in ids {
                selected.extend(
                    self.store()
                        .projection_snapshot(&format!("inbox:{id}"))
                        .await?
                        .projects,
                );
            }
            selected
        } else {
            self.store().projects().await?
        };
        for project in &selected {
            if projects.is_some_and(|ids| !ids.contains(&project.id)) {
                continue;
            }
            self.reconcile_inbox_locked(project, deferred_status, publish)
                .await?;
        }
        Ok(())
    }

    async fn reconcile_inbox_locked(
        &self,
        project: &Project,
        deferred_status: Option<&str>,
        publish: bool,
    ) -> Result<()> {
        let path = self.inbox_path(project)?;
        let key = format!("inbox_initialized:{}", project.id);
        let initialized = self.store().metadata(&key).await?.is_some();
        if !path.exists() {
            ensure!(
                !initialized,
                "Inbox document is missing: {}; restore it before synchronizing",
                path.display()
            );
            let template = format!(
                "---\nproject_id: {}\ntags:\n  - agent/inbox\n---\n\n# Inbox\n\nAdd `- [ ] Request` below; indent details. Cancel with `- [-]`.\n\n{}\n\n{END}\n",
                project.id,
                start(&project.id)
            );
            atomic_write(&path, &template)?;
        }
        let mut source = std::fs::read_to_string(&path)?;
        let parsed = parse(&source, &project.id)?;
        // Persist identity before import: an interrupted import can be retried
        // without assigning a second database identity to the same submission.
        let mut stamped = source.clone();
        for entry in parsed.iter().rev().filter(|e| e.id.is_none()) {
            stamped.insert_str(entry.header_end, &format!("{ID}{} -->", new_id("inbox")));
        }
        if stamped != source {
            checked_write(&path, &source, &stamped)?;
            source = stamped;
        }
        let parsed = parse(&source, &project.id)?;
        let entries: Vec<_> = parsed
            .iter()
            .map(|e| {
                json!({"id":e.id,"content":e.content,
                "cancelled":e.cancelled && e.id.as_deref() != deferred_status})
            })
            .collect();
        let options = WriteOptions {
            actor_ref: "user:inbox".into(),
            ..WriteOptions::default()
        };
        self.store()
            .execute(
                json!({"command":"inbox.import","project":project.id,"entries":entries}),
                options,
            )
            .await?;
        self.store().set_metadata(&key, &json!(true)).await?;
        if publish {
            self.render_inbox_locked(project, &source, &parsed).await?;
        }
        Ok(())
    }

    async fn render_inbox_locked(
        &self,
        project: &Project,
        source: &str,
        parsed: &[ParsedEntry],
    ) -> Result<()> {
        let key = format!("inbox:{}", project.id);
        let generation = self.store().document_generation(&key).await?;
        let sequence = self.store().latest_sequence().await?;
        let state = self.store().inbox_snapshot(&project.id).await?;
        let mut rendered = source.to_owned();
        let mut published = Vec::new();
        for item in parsed.iter().rev() {
            let entry = state
                .inboxes
                .iter()
                .find(|e| Some(&e.id) == item.id.as_ref())
                .context("missing imported Inbox entry")?;
            let replacement = if entry.deleted {
                String::new()
            } else {
                published.push(entry.id.clone());
                // The source remains authored by the human even after dispatch.
                self.inbox_entry_markdown(
                    entry,
                    if entry.content_pending {
                        &entry.content
                    } else {
                        &item.content
                    },
                    &state,
                )
            };
            rendered.replace_range(item.span.clone(), &replacement);
        }
        let mut pending: Vec<_> = state
            .inboxes
            .iter()
            .filter(|e| e.project_id == project.id && !e.published && !e.deleted)
            .collect();
        pending.sort_by_key(|e| e.position);
        for entry in pending {
            let position = rendered.find(END).context("missing Inbox end marker")?;
            rendered.insert_str(
                position,
                &self.inbox_entry_markdown(entry, &entry.content, &state),
            );
            published.push(entry.id.clone());
        }
        let rendered = crate::projection::normalize_document_timestamps(&rendered)?;
        checked_write(&self.inbox_path(project)?, source, &rendered)?;
        self.store()
            .execute(
                json!({"command":"inbox.publish","project":project.id,"ids":published}),
                WriteOptions::default(),
            )
            .await?;
        if let Some(generation) = generation {
            self.store()
                .acknowledge_documents(
                    &std::collections::BTreeMap::from([(key, generation)]),
                    &std::collections::BTreeMap::new(),
                    &BTreeSet::new(),
                    sequence,
                    &crate::publication::PublicationMetadata::default(),
                )
                .await?;
        }
        Ok(())
    }

    fn inbox_entry_markdown(&self, entry: &InboxEntry, content: &str, state: &Snapshot) -> String {
        let mut lines = content.lines();
        let check = match entry.status {
            InboxStatus::Active => '/',
            InboxStatus::PendingReview => 'r',
            InboxStatus::Completed => 'x',
            InboxStatus::Cancelled => '-',
            InboxStatus::Todo => ' ',
        };
        let mut output = format!(
            "- [{check}] {}{ID}{} -->{STATE}{} revision={} -->",
            lines.next().unwrap_or_default(),
            entry.id,
            entry.status,
            entry.revision
        );
        if let Some(job) = entry
            .job_id
            .as_ref()
            .and_then(|id| state.jobs.iter().find(|j| &j.id == id))
        {
            output.push_str(" · ");
            output.push_str(&self.link(&job.document_path, &job.name));
        }
        if let Some(lease) = &entry.lease {
            output.push_str(&format!(" · {}", lease.executor_ref));
        }
        output.push('\n');
        for line in lines {
            if !line.is_empty() {
                output.push_str("  ");
                output.push_str(line);
            }
            output.push('\n');
        }
        output
    }
}

pub(crate) fn request_projects(state: &Snapshot, request: &Value) -> BTreeSet<String> {
    let mut projects = BTreeSet::new();
    if let Some(id) = request["project"]
        .as_str()
        .and_then(|id| state.project_index(id).ok())
    {
        projects.insert(state.projects[id].id.clone());
    }
    if let Some(i) = request["job"]
        .as_str()
        .and_then(|id| state.job_index(id).ok())
    {
        projects.insert(state.jobs[i].project_id.clone());
    }
    if let Some(i) = request["task"]
        .as_str()
        .and_then(|id| state.task_index(id).ok())
    {
        projects.insert(state.tasks[i].project_id.clone());
    }
    if let Some(i) = request["inbox"]
        .as_str()
        .and_then(|id| crate::inbox::index(state, id).ok())
    {
        projects.insert(state.inboxes[i].project_id.clone());
    }
    if let Some(session) = request["session"].as_str() {
        projects.extend(
            state
                .tasks
                .iter()
                .filter(|t| t.last_session.as_deref() == Some(session))
                .map(|t| t.project_id.clone()),
        );
        projects.extend(
            state
                .inboxes
                .iter()
                .filter(|e| e.last_session.as_deref() == Some(session))
                .map(|e| e.project_id.clone()),
        );
    }
    projects
}
