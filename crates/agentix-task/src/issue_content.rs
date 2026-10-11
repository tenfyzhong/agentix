//! Issue prose belongs to the vault. `SQLite` retains only identities and hashes.
use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{atomic_write, section, split_properties};
use crate::{Issue, IssueContent, Service, WriteOptions, store::hash_bytes};

impl IssueContent {
    pub(crate) fn hash(&self) -> String {
        hash_bytes(
            json!([self.requirement, self.acceptance, self.notes])
                .to_string()
                .as_bytes(),
        )
    }

    pub(crate) fn review_hash(&self) -> String {
        hash_bytes(
            json!([self.requirement, self.acceptance])
                .to_string()
                .as_bytes(),
        )
    }

    pub(super) fn parse(source: &str) -> Result<Self> {
        let text = |key: &str, hint: &str| -> Result<String> {
            let value = section(source, key)?.unwrap_or_default();
            Ok(if value == hint { String::new() } else { value })
        };
        Ok(Self {
            requirement: text("requirement", "<!-- Describe the requirement. -->")?,
            acceptance: text("acceptance", "<!-- Define acceptance criteria. -->")?,
            notes: text("notes", "")?,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct PendingContent {
    base_hash: String,
    content: IssueContent,
}

impl Service {
    pub(super) fn issue_content_path(&self, issue: &Issue) -> Result<PathBuf> {
        ensure!(
            issue.id.starts_with("issue_")
                && issue
                    .id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_'),
            "invalid: Issue ID"
        );
        ensure!(
            issue.content_hash.chars().all(|c| c.is_ascii_hexdigit()),
            "invalid: Issue content hash"
        );
        self.safe_path(&format!(
            ".issue-content/{}-{}.json",
            issue.id, issue.content_hash
        ))
    }

    /// Load authored content on demand; metadata queries never read the vault.
    pub async fn issue_content(&self, id: &str) -> Result<IssueContent> {
        let issue = self.store().issue_record(id).await?;
        let source = self.issue_source(&issue).await?;
        self.projected_issue_content(&issue, &source)
    }

    pub async fn issue_detail(&self, id: &str) -> Result<Value> {
        let mut detail = self.store().issue_detail(id).await?;
        let content = serde_json::to_value(self.issue_content(id).await?)?;
        detail["issue"]
            .as_object_mut()
            .unwrap()
            .extend(content.as_object().unwrap().clone());
        Ok(detail)
    }

    async fn issue_source(&self, issue: &Issue) -> Result<String> {
        let key = format!("issue:{}", issue.id);
        let registered: Option<String> =
            sqlx::query_scalar("SELECT path FROM document_registry WHERE key=?")
                .bind(&key)
                .fetch_optional(&self.store().pool)
                .await?;
        let previous = registered
            .map(|path| BTreeMap::from([(key.clone(), path)]))
            .unwrap_or_default();
        self.delivery_source(&key, &issue.id, &issue.document_path, &previous)
    }

    pub(super) fn projected_issue_content(
        &self,
        issue: &Issue,
        source: &str,
    ) -> Result<IssueContent> {
        let current = IssueContent::parse(source)?;
        let path = self.issue_content_path(issue)?;
        if !path.exists() {
            return Ok(current);
        }
        let pending: PendingContent = serde_json::from_str(&std::fs::read_to_string(path)?)?;
        ensure!(
            pending.content.hash() == issue.content_hash,
            "conflict: Issue content journal hash changed"
        );
        ensure!(
            source.is_empty()
                || current.hash() == pending.base_hash
                || current.hash() == issue.content_hash,
            "conflict: Issue content was edited while document publication is pending"
        );
        Ok(pending.content)
    }

    pub(super) async fn prepare_issue_content(
        &self,
        request: &mut Value,
        options: &WriteOptions,
    ) -> Result<()> {
        let command = request["command"].as_str().unwrap_or_default();
        if !matches!(command, "issue.create" | "issue.update") {
            return Ok(());
        }
        let create = command == "issue.create";
        let authored = ["requirement", "acceptance", "notes"]
            .iter()
            .any(|key| request.get(key).is_some());
        if !create && !authored {
            return Ok(());
        }
        let mut content = if create {
            request["id"] = json!(crate::new_id("issue"));
            IssueContent::default()
        } else {
            self.issue_content(request["issue"].as_str().context("missing Issue")?)
                .await?
        };
        let base_hash = content.hash();
        for (key, field) in [
            ("requirement", &mut content.requirement),
            ("acceptance", &mut content.acceptance),
            ("notes", &mut content.notes),
        ] {
            if let Some(value) = request.as_object_mut().unwrap().remove(key) {
                *field = value
                    .as_str()
                    .with_context(|| format!("invalid: {key} must be text"))?
                    .into();
            }
        }
        request["content_hash"] = json!(content.hash());
        request["review_content_hash"] = json!(content.review_hash());
        // Validate before writing any journal, then repeat all guards in the transaction.
        let mut preview = self.store().request_snapshot(request).await?;
        let result = crate::mutations::apply(&mut preview, request, options, self.store().now())?;
        let issue: Issue = serde_json::from_value(result)?;
        let path = self.issue_content_path(&issue)?;
        atomic_write(
            &path,
            &serde_json::to_string(&PendingContent { base_hash, content })?,
        )?;
        Ok(())
    }

    pub(super) async fn cleanup_issue_content_journals_locked(&self) -> Result<()> {
        let directory = self.safe_path(".issue-content")?;
        if !directory.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let filename = entry.file_name();
            let Some((id, hash)) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.rsplit_once('-'))
            else {
                continue;
            };
            if !id.starts_with("issue_") {
                continue;
            }
            let current: Option<String> = sqlx::query_scalar(
                "SELECT json_extract(data,'$.content_hash') FROM issues WHERE id=?",
            )
            .bind(id)
            .fetch_optional(&self.store().pool)
            .await?;
            let published = self.store().metadata(&format!("delivery:{id}")).await?;
            if current.as_deref() != Some(hash)
                || published.as_ref().and_then(Value::as_str) == Some(hash)
            {
                std::fs::remove_file(
                    self.safe_path(&format!(".issue-content/{}", filename.to_string_lossy()))?,
                )?;
            }
        }
        Ok(())
    }
    /// Upgrade one row at a time under both the vault lock and `SQLite` writer lock.
    /// A failed export leaves the legacy row intact for a later retry.
    pub(super) async fn migrate_issue_content_locked(&self) -> Result<()> {
        loop {
            let mut tx = self.store().pool.begin_with("BEGIN IMMEDIATE").await?;
            let row: Option<String> = sqlx::query_scalar("SELECT data FROM issues WHERE json_type(data,'$.requirement') IS NOT NULL OR json_type(data,'$.acceptance') IS NOT NULL OR json_type(data,'$.notes') IS NOT NULL LIMIT 1")
                .fetch_optional(&mut *tx).await?;
            let Some(row) = row else {
                tx.commit().await?;
                break;
            };
            let legacy: Value = serde_json::from_str(&row)?;
            let mut issue: Issue = serde_json::from_value(legacy.clone())?;
            let content: IssueContent = serde_json::from_value(legacy.clone())?;
            let key = format!("issue:{}", issue.id);
            let registered: Option<String> =
                sqlx::query_scalar("SELECT path FROM document_registry WHERE key=?")
                    .bind(&key)
                    .fetch_optional(&mut *tx)
                    .await?;
            let relative = registered.as_deref().unwrap_or(&issue.document_path);
            let path = self.safe_path(relative)?;
            let source = if path.exists() {
                std::fs::read_to_string(&path)?
            } else {
                String::new()
            };
            let current = if source.is_empty() {
                None
            } else {
                let (properties, _) = split_properties(&source)?;
                ensure!(
                    properties["id"] == issue.id && properties["taskix-generated"] == true,
                    "conflict: Issue migration document identity changed"
                );
                Some(IssueContent::parse(&source)?)
            };
            if current.as_ref().is_some_and(|current| *current != content) {
                // Preserve both versions when the vault contains an unsynchronized edit.
                let backup = self.safe_path(&format!("Issue Migration/{}.md", issue.id))?;
                let saved = format!(
                    "# Legacy Issue content: {}\n\n## Requirement\n{}\n\n## Acceptance Criteria\n{}\n\n## Notes\n{}\n",
                    issue.id, content.requirement, content.acceptance, content.notes
                );
                if backup.exists() {
                    ensure!(
                        std::fs::read_to_string(&backup)? == saved,
                        "conflict: Issue migration backup already exists"
                    );
                }
                atomic_write(&backup, &saved)?;
            }
            if source.is_empty() {
                let project_data: String =
                    sqlx::query_scalar("SELECT data FROM projects WHERE id=?")
                        .bind(&issue.project_id)
                        .fetch_one(&mut *tx)
                        .await?;
                let project = crate::stored_paths::from_str(&project_data)?;
                issue.content_hash = content.hash();
                issue.review_content_hash = content.review_hash();
                let journal = self.issue_content_path(&issue)?;
                atomic_write(
                    &journal,
                    &serde_json::to_string(&PendingContent {
                        base_hash: IssueContent::default().hash(),
                        content: content.clone(),
                    })?,
                )?;
                atomic_write(&path, &self.issue_document(&issue, &project, None, "")?)?;
                std::fs::remove_file(journal)?;
            }
            let preserved = current.unwrap_or(content);
            issue.content_hash = preserved.hash();
            issue.review_content_hash = preserved.review_hash();
            sqlx::query("UPDATE issues SET data=? WHERE id=?")
                .bind(crate::stored_paths::to_string(&issue)?)
                .bind(&issue.id)
                .execute(&mut *tx)
                .await?;
            // Historical replay results must not keep a second copy of authored prose.
            sqlx::query("UPDATE idempotency_keys SET result=json_remove(result,'$.result.requirement','$.result.acceptance','$.result.notes') WHERE json_extract(result,'$.result.id')=?")
                .bind(&issue.id).execute(&mut *tx).await?;
            tx.commit().await?;
        }
        Ok(())
    }
}
