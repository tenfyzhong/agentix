//! A shared native Base supplies the Issue list and the embedding note's Job filter.
use std::{collections::BTreeMap, path::Path};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

use super::{
    NAME_FORMULA, completion_sort, frontmatter, optional_local_timestamp, section, split_properties,
};
use crate::{Issue, IssueStatus, JobStatus, Milestone, Project, Service, Snapshot, WriteOptions};

const REQUIREMENT_HINT: &str = "<!-- Describe the requirement. -->";
const ACCEPTANCE_HINT: &str = "<!-- Define acceptance criteria. -->";

impl Service {
    pub(super) fn base_link(&self, path: &str) -> String {
        self.config()
            .vault_relative_path(Path::new(path))
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches("./")
            .to_owned()
    }

    pub(super) fn global_issue_base(&self) -> Result<String> {
        let folder_filter = |path: &Path| {
            let folder = path.to_string_lossy().replace('\\', "/");
            format!("file.inFolder({})", json!(folder.trim_start_matches("./")))
        };
        let active = json!({"and":["archived != true"]});
        let columns = json!([
            "formula.name",
            "project_id",
            "status",
            "milestone",
            "updated_at"
        ]);
        let base = json!({
            "filters":{"and":[
                {"or":[folder_filter(&self.config.documents.directory.join("Projects")), folder_filter(&self.config.documents.archive_directory)]},
                "file.ext == \"md\"", "note[\"taskix-generated\"] == true", "file.hasTag(\"agent/issue\")"
            ]},
            "formulas":{"name":NAME_FORMULA},
            "properties":{
                "formula.name":{"displayName":"Issue"},
                "note.project_id":{"displayName":"Project"},
                "note.status":{"displayName":"Status"},
                "note.milestone":{"displayName":"Milestone"},
                "note.updated_at":{"displayName":"Updated"}
            },
            "views":[
                {"type":"table","name":"Issues","filters":active,"order":columns,"sort":completion_sort()},
                {"type":"kanban","name":"By status","filters":active,"order":columns,"sort":completion_sort(),
                    "groupBy":{"property":"note.status","direction":"ASC"},"groupOrder":IssueStatus::ALL,
                    "hideEmptyColumns":false,"columnWidth":300},
                {"type":"table","name":"By project","filters":active,"order":columns,"sort":completion_sort(),
                    "groupBy":{"property":"note.project_id","direction":"ASC"}},
                {"type":"table","name":"Archived","filters":{"and":["archived == true"]},"order":columns,"sort":completion_sort()}
            ]
        });
        Ok(format!(
            "# taskix-generated: global-issues\n{}",
            serde_yaml::to_string(&base)?
        ))
    }

    fn issue_base(project: &Project) -> Result<String> {
        let issue_filters = json!({"and":["file.hasTag(\"agent/issue\")","archived != true"]});
        let board = |name: &str, filters: Value, statuses: Value| {
            json!({
                "type":"kanban","name":name,"filters":filters,
                "groupBy":{"property":"note.status","direction":"ASC"},
                "groupOrder":statuses,"order":["formula.name","status"],"sort":completion_sort(),
                "hideEmptyColumns":false,"columnWidth":300
            })
        };
        let mut milestones = board("By Milestone", issue_filters.clone(), Value::Null);
        milestones.as_object_mut().unwrap().remove("groupOrder");
        milestones["groupBy"] = json!({"property":"note.milestone","direction":"ASC"});
        let base = json!({
            "filters":{"and":[format!("project_id == {}",json!(project.id))]},
            "formulas":{"name":NAME_FORMULA},
            "views":[
                board("Issue board",issue_filters.clone(),json!(IssueStatus::ALL)),
                milestones,
                {"type":"table","name":"Pending review","filters":{"and":["file.hasTag(\"agent/issue\")","status == \"PENDING_REVIEW\"","archived != true"]},"order":["formula.name","status","milestone"]},
                board("Related Jobs",json!({"and":[
                    "file.hasTag(\"agent/job\") || file.hasTag(\"agent/archived/job\")",
                    "this.file.hasTag(\"agent/issue\")","issue_id == this.id"
                ]}),json!(JobStatus::ALL)),
                board("Milestone Issues",json!({"and":["file.hasTag(\"agent/issue\")","this.file.hasTag(\"agent/milestone\")","milestone_id == this.id"]}),json!(IssueStatus::ALL))
            ]
        });
        Ok(format!(
            "# taskix-generated: issues:{}\n{}",
            project.id,
            serde_yaml::to_string(&base)?
        ))
    }

    fn milestones_base(project: &Project) -> Result<String> {
        let base = json!({"filters":{"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(project.id))]},
            "formulas":{"name":NAME_FORMULA},"views":[{"type":"table","name":"Milestones","order":["formula.name","order","due_date","completed_issues","total_issues","cancelled_issues"],
                "sort":[{"column":"order","direction":"ASC"},{"column":"file.name","direction":"ASC"}]}]});
        Ok(format!(
            "# taskix-generated: milestones:{}\n{}",
            project.id,
            serde_yaml::to_string(&base)?
        ))
    }

    fn issue_document(
        &self,
        issue: &Issue,
        project: &Project,
        milestone: Option<&Milestone>,
        existing: &str,
    ) -> Result<String> {
        let mut properties = split_properties(existing)?.0;
        let mut managed = serde_json::to_value(issue)?;
        for field in [
            "requirement",
            "acceptance",
            "notes",
            "name",
            "document_path",
        ] {
            managed.as_object_mut().unwrap().remove(field);
        }
        for field in [
            "created_at",
            "updated_at",
            "started_at",
            "pending_review_at",
            "completed_at",
            "cancelled_at",
            "archived_at",
        ] {
            managed[field] = optional_local_timestamp(managed[field].as_i64())?;
        }
        managed["tags"] = json!(["agent/issue"]);
        managed["title"] = json!(issue.name);
        managed["archived"] = json!(issue.archived_at.is_some() || project.archived_at.is_some());
        managed["milestone"] = milestone.map_or(Value::Null, |milestone| {
            json!(self.link(&milestone.document_path, &milestone.name))
        });
        properties
            .as_object_mut()
            .unwrap()
            .extend(managed.as_object().unwrap().clone());
        let mut doc = frontmatter(properties);
        doc.push_str(&Self::header(&issue.name));
        for (label, key, body, hint) in [
            (
                "Requirement",
                "requirement",
                issue.requirement.as_str(),
                REQUIREMENT_HINT,
            ),
            (
                "Acceptance Criteria",
                "acceptance",
                issue.acceptance.as_str(),
                ACCEPTANCE_HINT,
            ),
            ("Notes", "notes", issue.notes.as_str(), ""),
        ] {
            if !existing.is_empty() {
                section(existing, key)?.context("missing Issue section")?;
            }
            let body = if body.is_empty() { hint } else { body };
            doc.push_str(&format!(
                "\n## {label}\n\n<!-- taskix:{key}:start -->\n{body}\n<!-- taskix:{key}:end -->\n"
            ));
        }
        doc.push_str(&format!(
            "\n## Related Jobs\n\n![[{}#Related Jobs]]\n",
            self.base_link(&format!("{}/Issue.base", project.document_directory()))
        ));
        Ok(doc)
    }

    fn milestone_document(
        &self,
        milestone: &Milestone,
        progress: crate::issue_read::MilestoneProgress,
        project: &Project,
        existing: &str,
    ) -> Result<String> {
        let mut properties = split_properties(existing)?.0;
        let mut managed = serde_json::to_value(milestone)?;
        for field in ["goal", "name", "document_path"] {
            managed.as_object_mut().unwrap().remove(field);
        }
        for field in ["created_at", "updated_at"] {
            managed[field] = optional_local_timestamp(managed[field].as_i64())?;
        }
        managed["tags"] = json!(["agent/milestone"]);
        managed["title"] = json!(milestone.name);
        managed["total_issues"] = json!(progress.total);
        managed["completed_issues"] = json!(progress.completed);
        managed["cancelled_issues"] = json!(progress.cancelled);
        properties
            .as_object_mut()
            .unwrap()
            .extend(managed.as_object().unwrap().clone());
        if !existing.is_empty() {
            section(existing, "goal")?.context("missing Milestone goal")?;
        }
        let mut doc = frontmatter(properties);
        doc.push_str(&Self::header(&milestone.name));
        doc.push_str(&format!("\n## Goal\n\n<!-- taskix:goal:start -->\n{}\n<!-- taskix:goal:end -->\n\n## Issues\n\n![[{}#Milestone Issues]]\n",milestone.goal,self.base_link(&format!("{}/Issue.base",project.document_directory()))));
        Ok(doc)
    }

    pub(super) fn render_delivery_documents(
        &self,
        state: &Snapshot,
        selected: Option<&str>,
        previous: &BTreeMap<String, String>,
        files: &mut BTreeMap<String, String>,
        paths: &mut BTreeMap<String, String>,
    ) -> Result<()> {
        let includes = |key: &str| selected.is_none_or(|selected| selected == key);
        let projects: BTreeMap<_, _> = state
            .projects
            .iter()
            .map(|project| (project.id.as_str(), project))
            .collect();
        let milestones: BTreeMap<_, _> = state
            .milestones
            .iter()
            .map(|milestone| (milestone.id.as_str(), milestone))
            .collect();
        let mut progress = if selected.is_none() {
            BTreeMap::new()
        } else {
            state.query_context.milestone_progress.clone()
        };
        if selected.is_none() {
            // Full snapshots already contain every Issue; aggregate once for all Milestones.
            for issue in &state.issues {
                if let Some(id) = &issue.milestone_id {
                    let counts = progress.entry(id.clone()).or_default();
                    counts.total += 1;
                    counts.completed += i64::from(issue.status == IssueStatus::Completed);
                    counts.cancelled += i64::from(issue.status == IssueStatus::Cancelled);
                }
            }
        }
        for project in &state.projects {
            for folder in ["Issues", "Milestones"] {
                std::fs::create_dir_all(
                    self.safe_path(&format!("{}/{folder}", project.document_directory()))?,
                )?;
            }
            for (kind, name, body) in [
                ("issue-base", "Issue.base", Self::issue_base(project)?),
                (
                    "milestone-base",
                    "Milestones.base",
                    Self::milestones_base(project)?,
                ),
            ] {
                let key = format!("{kind}:{}", project.id);
                if !includes(&key) {
                    continue;
                }
                let path = format!("{}/{name}", project.document_directory());
                let body = if previous.contains_key(&key) && self.safe_path(&path)?.exists() {
                    std::fs::read_to_string(self.safe_path(&path)?)?
                } else {
                    body
                };
                files.insert(path.clone(), body);
                paths.insert(key, path);
            }
        }
        for issue in &state.issues {
            let key = format!("issue:{}", issue.id);
            if !includes(&key) {
                continue;
            }
            let project = projects
                .get(issue.project_id.as_str())
                .context("missing Issue Project")?;
            let existing = self.delivery_source(&key, &issue.id, &issue.document_path, previous)?;
            let milestone = issue
                .milestone_id
                .as_deref()
                .and_then(|id| milestones.get(id).copied());
            files.insert(
                issue.document_path.clone(),
                self.issue_document(issue, project, milestone, &existing)?,
            );
            paths.insert(key, issue.document_path.clone());
        }
        for milestone in &state.milestones {
            let key = format!("milestone:{}", milestone.id);
            if !includes(&key) {
                continue;
            }
            let project = projects
                .get(milestone.project_id.as_str())
                .context("missing Milestone Project")?;
            let existing =
                self.delivery_source(&key, &milestone.id, &milestone.document_path, previous)?;
            files.insert(
                milestone.document_path.clone(),
                self.milestone_document(
                    milestone,
                    progress.get(&milestone.id).copied().unwrap_or_default(),
                    project,
                    &existing,
                )?,
            );
            paths.insert(key, milestone.document_path.clone());
        }
        Ok(())
    }

    fn delivery_source(
        &self,
        key: &str,
        id: &str,
        path: &str,
        previous: &BTreeMap<String, String>,
    ) -> Result<String> {
        let path = self.safe_path(previous.get(key).map_or(path, String::as_str))?;
        if !path.exists() {
            return Ok(String::new());
        }
        let source = std::fs::read_to_string(path)?;
        let (properties, _) = split_properties(&source)?;
        ensure!(
            properties["taskix-generated"] == true && properties["id"] == id,
            "conflict: Issue or Milestone document identity changed"
        );
        Ok(source)
    }

    pub(super) async fn reconcile_issue_documents_locked(
        &self,
        selected: Option<&Snapshot>,
    ) -> Result<BTreeMap<String, (i64, i64)>> {
        let mut imported = BTreeMap::new();
        let issues = if let Some(selected) = selected {
            selected.issues.clone()
        } else {
            let records:Vec<String>=sqlx::query_scalar("SELECT data FROM issues WHERE id IN (SELECT substr(key,7) FROM document_registry WHERE key LIKE 'issue:%')")
                .fetch_all(&self.store().pool).await?;
            records
                .iter()
                .map(|record| crate::stored_paths::from_str(record))
                .collect::<Result<Vec<Issue>>>()?
        };
        for issue in issues {
            let key = format!("issue:{}", issue.id);
            let registered: Option<String> =
                sqlx::query_scalar("SELECT path FROM document_registry WHERE key=?")
                    .bind(&key)
                    .fetch_optional(&self.store().pool)
                    .await?;
            let Some(path) = registered else {
                continue;
            };
            let path = self.safe_path(&path)?;
            if !path.exists() {
                continue;
            }
            let source = std::fs::read_to_string(path)?;
            let (properties, _) = split_properties(&source)?;
            ensure!(
                properties["id"] == issue.id && properties["taskix-generated"] == true,
                "conflict: Issue document identity changed"
            );
            let requirement =
                section(&source, "requirement")?.context("missing Issue requirement")?;
            let acceptance = section(&source, "acceptance")?.context("missing Issue acceptance")?;
            let notes = section(&source, "notes")?.context("missing Issue notes")?;
            let requirement = if requirement == REQUIREMENT_HINT {
                String::new()
            } else {
                requirement
            };
            let acceptance = if acceptance == ACCEPTANCE_HINT {
                String::new()
            } else {
                acceptance
            };
            let source_hash = crate::store::hash_bytes(
                json!([requirement, acceptance, notes])
                    .to_string()
                    .as_bytes(),
            );
            let receipt = self
                .store()
                .metadata(&format!("delivery:{}", issue.id))
                .await?;
            if receipt.as_ref().and_then(Value::as_str) == Some(source_hash.as_str()) {
                // The last published body cannot override a committed replacement after a crash.
                continue;
            }
            ensure!(
                self.store().document_generation(&key).await?.is_none()
                    || source_hash == issue.content_hash,
                "conflict: Issue content was edited while document publication is pending; resolve the content conflict before sync"
            );
            if requirement != issue.requirement
                || acceptance != issue.acceptance
                || notes != issue.notes
            {
                let outcome = self.store().execute(json!({"command":"issue.update","issue":issue.id,"requirement":requirement,"acceptance":acceptance,"notes":notes}),
                    WriteOptions{actor_ref:"user:obsidian".into(),expected_revision:Some(issue.revision),..WriteOptions::default()}).await?;
                imported.insert(
                    issue.id,
                    (
                        issue.revision,
                        outcome.result["revision"]
                            .as_i64()
                            .context("missing imported revision")?,
                    ),
                );
            }
        }
        imported.extend(self.reconcile_milestone_goals_locked(selected).await?);
        Ok(imported)
    }

    async fn reconcile_milestone_goals_locked(
        &self,
        selected: Option<&Snapshot>,
    ) -> Result<BTreeMap<String, (i64, i64)>> {
        let mut imported = BTreeMap::new();
        let milestones = if let Some(selected) = selected {
            selected.milestones.clone()
        } else {
            let records: Vec<String> = sqlx::query_scalar("SELECT data FROM milestones WHERE id IN (SELECT substr(key,11) FROM document_registry WHERE key LIKE 'milestone:%')")
                .fetch_all(&self.store().pool).await?;
            records
                .iter()
                .map(|record| crate::stored_paths::from_str(record))
                .collect::<Result<Vec<Milestone>>>()?
        };
        for milestone in milestones {
            let key = format!("milestone:{}", milestone.id);
            let registered: Option<String> =
                sqlx::query_scalar("SELECT path FROM document_registry WHERE key=?")
                    .bind(&key)
                    .fetch_optional(&self.store().pool)
                    .await?;
            let Some(path) = registered else { continue };
            let path = self.safe_path(&path)?;
            if !path.exists() {
                continue;
            }
            let source = std::fs::read_to_string(path)?;
            let (properties, _) = split_properties(&source)?;
            ensure!(
                properties["id"] == milestone.id && properties["taskix-generated"] == true,
                "conflict: Milestone document identity changed"
            );
            let goal = section(&source, "goal")?.context("missing Milestone goal")?;
            let hash = crate::store::hash_bytes(goal.as_bytes());
            if self
                .store()
                .metadata(&format!("delivery:{}", milestone.id))
                .await?
                .as_ref()
                .and_then(Value::as_str)
                == Some(hash.as_str())
            {
                continue;
            }
            ensure!(
                self.store().document_generation(&key).await?.is_none() || goal == milestone.goal,
                "conflict: Milestone goal was edited while document publication is pending"
            );
            if goal != milestone.goal {
                let outcome = self
                    .store()
                    .execute(
                        json!({"command":"milestone.update","milestone":milestone.id,"goal":goal}),
                        WriteOptions {
                            actor_ref: "user:obsidian".into(),
                            expected_revision: Some(milestone.revision),
                            ..WriteOptions::default()
                        },
                    )
                    .await?;
                imported.insert(
                    milestone.id,
                    (
                        milestone.revision,
                        outcome.result["revision"]
                            .as_i64()
                            .context("missing imported revision")?,
                    ),
                );
            }
        }
        Ok(imported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn milestone_rendering_uses_frozen_full_and_scoped_snapshots_without_database_reads() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vault");
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        let config: crate::Config = toml::from_str(&format!(
            "schema_version=1\n[storage]\npath={:?}\n[documents]\nroot={:?}\ndirectory='Taskix'\n",
            dir.path().join("state.db"),
            root
        ))
        .unwrap();
        let store = crate::Store::open(&config.storage.path).await.unwrap();
        let service = Service::new(config, store).unwrap();
        let project = service
            .store()
            .execute(
                json!({"command":"project.register","name":"Snapshot","root":dir.path()}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        let milestone = service
            .store()
            .execute(
                json!({"command":"milestone.create","project":project["id"],"title":"Release"}),
                WriteOptions::default(),
            )
            .await
            .unwrap()
            .result;
        let issue = service.store().execute(json!({"command":"issue.create","project":project["id"],"milestone":milestone["id"],"title":"First"}), WriteOptions::default()).await.unwrap().result;
        let key = format!("milestone:{}", milestone["id"].as_str().unwrap());
        let full = service.store().snapshot().await.unwrap();
        let scoped = service.store().projection_snapshot(&key).await.unwrap();
        assert!(
            scoped.issues.is_empty(),
            "progress must not load Issue bodies"
        );
        service
            .store()
            .execute(
                json!({"command":"issue.cancel","issue":issue["id"]}),
                WriteOptions::default(),
            )
            .await
            .unwrap();
        service.store().execute(json!({"command":"issue.create","project":project["id"],"milestone":milestone["id"],"title":"Later"}), WriteOptions::default()).await.unwrap();
        // Rendering must not acquire another connection or observe either later write.
        service.store().pool.close().await;
        for (state, selected) in [(&full, None), (&scoped, Some(key.as_str()))] {
            let mut files = BTreeMap::new();
            let mut paths = BTreeMap::new();
            service
                .render_delivery_documents(
                    state,
                    selected,
                    &BTreeMap::new(),
                    &mut files,
                    &mut paths,
                )
                .unwrap();
            let source = &files[milestone["document_path"].as_str().unwrap()];
            let properties = split_properties(source).unwrap().0;
            assert_eq!(properties["total_issues"], 1);
            assert_eq!(properties["cancelled_issues"], 0);
            assert_eq!(properties["completed_issues"], 0);
        }
    }
}
