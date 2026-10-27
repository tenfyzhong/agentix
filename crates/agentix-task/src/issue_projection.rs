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
            "project",
            "status",
            "issue_type",
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
                "note.project":{"displayName":"Project"},
                "note.status":{"displayName":"Status"},
                "note.issue_type":{"displayName":"Type"},
                "note.milestone":{"displayName":"Milestone"},
                "note.updated_at":{"displayName":"Updated"}
            },
            "views":[
                {"type":"table","name":"Issues","filters":active,"order":columns,"sort":completion_sort()},
                {"type":"kanban","name":"By status","filters":active,"order":columns,"sort":completion_sort(),
                    "groupBy":{"property":"note.status","direction":"ASC"},"groupOrder":IssueStatus::ALL,
                    "hideEmptyColumns":false,"columnWidth":300},
                {"type":"table","name":"By project","filters":active,"order":columns,"sort":completion_sort(),
                    "groupBy":{"property":"note.project","direction":"ASC"}},
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
                "groupOrder":statuses,"order":if name == "Related Jobs" { json!(["formula.name","status"]) } else { json!(["formula.name","status","issue_type","milestone"]) },"sort":completion_sort(),
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

    fn migrate_issue_base(source: String, project: &Project) -> Result<String> {
        let mut base: Value = serde_yaml::from_str(&source)?;
        let Some(views) = base["views"].as_array_mut() else {
            return Ok(source);
        };
        let mut changed = false;
        for view in views {
            if view["type"] != "kanban"
                || !matches!(
                    view["name"].as_str(),
                    Some("Issue board" | "By Milestone" | "Milestone Issues")
                )
            {
                continue;
            }
            if let Some(order) = view["order"].as_array_mut()
                && !order
                    .iter()
                    .any(|column| column == "milestone" || column == "note.milestone")
            {
                order.push(json!("milestone"));
                changed = true;
            }
        }
        if !changed {
            return Ok(source);
        }
        Ok(format!(
            "# taskix-generated: issues:{}\n{}",
            project.id,
            serde_yaml::to_string(&base)?
        ))
    }

    fn milestone_status_board() -> Value {
        json!({"type":"kanban","name":"Milestone board",
            "groupBy":{"property":"note.status","direction":"ASC"},
            "groupOrder":["PLANNED","ACTIVE","COMPLETED","CANCELLED"],
            "order":["formula.name","status","due_date","completed_issues","total_issues","cancelled_issues"],
            "sort":[{"column":"order","direction":"ASC"},{"column":"file.name","direction":"ASC"}],
            "hideEmptyColumns":false,"columnWidth":300})
    }

    fn migrate_milestones_base(source: String, project: &Project) -> Result<String> {
        let mut base: Value = serde_yaml::from_str(&source)?;
        let scope = json!({"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(project.id))]});
        let open = json!({"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]});
        let mut old_filtered = scope.clone();
        old_filtered["and"]
            .as_array_mut()
            .unwrap()
            .push(open.clone());
        let move_filter = base["filters"] == old_filtered;
        let default_scope = base["filters"] == scope;
        let Some(views) = base["views"].as_array_mut() else {
            return Ok(source);
        };
        let mut changed = move_filter;
        let mut legacy_table = false;
        if let Some(order) = views
            .iter_mut()
            .find(|view| view["name"] == "Milestones" && view["type"] == "table")
            .and_then(|view| view["order"].as_array_mut())
            && !order
                .iter()
                .any(|column| column == "status" || column == "note.status")
        {
            order.insert(usize::from(!order.is_empty()), json!("status"));
            changed = true;
            legacy_table = true;
        }
        if move_filter || (legacy_table && default_scope) {
            for view in views.iter_mut().filter(|view| view["type"] == "table") {
                let filters = view
                    .get("filters")
                    .filter(|value| !value.is_null())
                    .cloned();
                view["filters"] = filters.map_or_else(
                    || json!({"and":[open]}),
                    |filters| json!({"and":[filters,open]}),
                );
            }
        }
        if !views.iter().any(|view| {
            view["type"] == "kanban"
                && matches!(
                    view["groupBy"]["property"].as_str(),
                    Some("status" | "note.status")
                )
        }) {
            views.push(Self::milestone_status_board());
            changed = true;
        }
        if !changed {
            return Ok(source);
        }
        if move_filter {
            base["filters"] = scope;
        }
        Ok(format!(
            "# taskix-generated: milestones:{}\n{}",
            project.id,
            serde_yaml::to_string(&base)?
        ))
    }

    fn milestones_base(project: &Project) -> Result<String> {
        let base = json!({"filters":{"and":["file.hasTag(\"agent/milestone\")",format!("project_id == {}",json!(project.id))]},
            "formulas":{"name":NAME_FORMULA},"views":[{"type":"table","name":"Milestones","filters":{"and":[{"or":["status == \"PLANNED\"", "status == \"ACTIVE\""]}]},"order":["formula.name","status","order","due_date","completed_issues","total_issues","cancelled_issues"],
                "sort":[{"column":"order","direction":"ASC"},{"column":"file.name","direction":"ASC"}]},Self::milestone_status_board()]});
        Ok(format!(
            "# taskix-generated: milestones:{}\n{}",
            project.id,
            serde_yaml::to_string(&base)?
        ))
    }

    fn issue_body_metadata(&self, body: &str, issue: &Issue, project: &Project) -> String {
        // Only the leading document heading is managed; headings in authored sections stay intact.
        let mut body = body.to_owned();
        let start = body.len() - body.trim_start().len();
        if body[start..].starts_with("# ") {
            let end = body[start..]
                .find('\n')
                .map_or(body.len(), |end| start + end);
            body.replace_range(start..end, &format!("# {}", super::escape(&issue.name)));
        }
        if let Some(section) = body.find("\n## Related Jobs\n")
            && let Some(start) = body[section..].find("![[")
        {
            let start = section + start;
            if let Some(end) = body[start..].find("#Related Jobs]]") {
                let end = start + end + "#Related Jobs]]".len();
                let link = self.base_link(&format!("{}/Issue.base", project.document_directory()));
                body.replace_range(start..end, &format!("![[{link}#Related Jobs]]"));
            }
        }
        body
    }

    pub(super) fn issue_document(
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
        managed["project"] = json!(self.config.project_link(project));
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
        let staged = self.issue_content_path(issue)?.exists();
        if !existing.is_empty() && !staged {
            // Status/metadata publication preserves arbitrary authored prose verbatim.
            let body = split_properties(existing)?.1;
            doc.push_str(&self.issue_body_metadata(body, issue, project));
            return Ok(doc);
        }
        let content = self.projected_issue_content(issue, existing)?;
        let mut body = if existing.is_empty() {
            Self::header(&issue.name)
        } else {
            split_properties(existing)?.1.to_owned()
        };
        for (label, key, value, hint) in [
            (
                "Requirement",
                "requirement",
                content.requirement.as_str(),
                REQUIREMENT_HINT,
            ),
            (
                "Acceptance Criteria",
                "acceptance",
                content.acceptance.as_str(),
                ACCEPTANCE_HINT,
            ),
            ("Notes", "notes", content.notes.as_str(), ""),
        ] {
            let value = if value.is_empty() { hint } else { value };
            let start = format!("<!-- taskix:{key}:start -->");
            let end = format!("<!-- taskix:{key}:end -->");
            if existing.is_empty() {
                body.push_str(&format!("\n## {label}\n\n{start}\n{value}\n{end}\n"));
            } else {
                let a = body.find(&start).context("missing Issue section start")? + start.len();
                let b = body[a..].find(&end).context("missing Issue section end")? + a;
                body.replace_range(a..b, &format!("\n{value}\n"));
            }
        }
        if existing.is_empty() {
            body.push_str(&format!(
                "\n## Related Jobs\n\n![[{}#Related Jobs]]\n",
                self.base_link(&format!("{}/Issue.base", project.document_directory()))
            ));
        }
        doc.push_str(&self.issue_body_metadata(&body, issue, project));
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
        for field in [
            "created_at",
            "updated_at",
            "started_at",
            "completed_at",
            "cancelled_at",
            "archived_at",
        ] {
            managed[field] = optional_local_timestamp(managed[field].as_i64())?;
        }
        managed["project"] = json!(self.config.project_link(project));
        managed["tags"] = json!(["agent/milestone"]);
        managed["archived"] = json!(milestone.archived_at.is_some());
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
                    "Milestone.base",
                    Self::milestones_base(project)?,
                ),
            ] {
                let key = format!("{kind}:{}", project.id);
                if !includes(&key) {
                    continue;
                }
                let path = format!("{}/{name}", project.document_directory());
                let body =
                    self.preserved_delivery_base(previous.get(&key), &path, kind, project, body)?;
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

    fn preserved_delivery_base(
        &self,
        previous: Option<&String>,
        path: &str,
        kind: &str,
        project: &Project,
        default: String,
    ) -> Result<String> {
        let source = if self.safe_path(path)?.exists() {
            path
        } else {
            previous.map_or(path, String::as_str)
        };
        if previous.is_none() || !self.safe_path(source)?.exists() {
            return Ok(default);
        }
        let existing = std::fs::read_to_string(self.safe_path(source)?)?;
        if kind == "issue-base" {
            Self::migrate_issue_base(existing, project)
        } else if kind == "milestone-base" {
            Self::migrate_milestones_base(existing, project)
        } else {
            Ok(existing)
        }
    }

    pub(super) fn delivery_source(
        &self,
        key: &str,
        id: &str,
        path: &str,
        previous: &BTreeMap<String, String>,
    ) -> Result<String> {
        let registered = self.safe_path(previous.get(key).map_or(path, String::as_str))?;
        let path = if registered.exists() {
            registered
        } else {
            self.safe_path(path)?
        };
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

    /// Find a moved delivery note only when its registered source has disappeared.
    /// The immutable ID identifies it; the numbered filename supplies the new name.
    fn renamed_delivery_document(
        &self,
        id: &str,
        project_id: &str,
        name: &str,
        revision: i64,
        prefix: &str,
        registered: &str,
    ) -> Result<Option<(String, String)>> {
        let parent = Path::new(registered)
            .parent()
            .context("missing delivery directory")?;
        let directory = self.safe_path(parent.to_str().context("invalid delivery directory")?)?;
        if !directory.is_dir() {
            return Ok(None);
        }
        let mut candidate = None;
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.path().extension().is_none_or(|ext| ext != "md") {
                continue;
            }
            let relative = parent
                .join(entry.file_name())
                .to_string_lossy()
                .replace('\\', "/");
            let path = self.safe_path(&relative)?;
            if !path.is_file() {
                continue;
            }
            let source = std::fs::read_to_string(path)?;
            let Ok((properties, _)) = split_properties(&source) else {
                continue;
            };
            if properties["id"] != id {
                continue;
            }
            ensure!(
                candidate.is_none(),
                "conflict: ambiguous renamed delivery documents for {id}"
            );
            ensure!(
                properties["taskix-generated"] == true && properties["project_id"] == project_id,
                "conflict: renamed delivery document identity changed"
            );
            ensure!(
                properties["revision"].as_i64() == Some(revision),
                "conflict: renamed delivery revision changed"
            );
            let filename = entry.file_name();
            let renamed_name = filename
                .to_str()
                .and_then(|s| s.strip_suffix(".md"))
                .and_then(|s| s.strip_prefix(prefix))
                .context("invalid: preserve the delivery filename date and sequence prefix")?;
            ensure!(
                !renamed_name.is_empty() && crate::naming::short_name(renamed_name) == renamed_name,
                "invalid: delivery filename must use a portable name of at most 48 characters"
            );
            ensure!(
                properties["title"] == name || properties["title"] == renamed_name,
                "conflict: delivery filename and title were edited differently"
            );
            candidate = Some((relative, renamed_name.to_owned()));
        }
        Ok(candidate)
    }

    async fn validate_delivery_rename(
        &self,
        revision: i64,
        request: &Value,
        relative: &str,
    ) -> Result<()> {
        let mut preview = self.store().request_snapshot(request).await?;
        let result = crate::mutations::apply(
            &mut preview,
            request,
            &WriteOptions {
                expected_revision: Some(revision),
                ..WriteOptions::default()
            },
            self.store().now(),
        )?;
        ensure!(
            result["document_path"] == relative,
            "conflict: renamed delivery filename collides with another delivery name"
        );
        Ok(())
    }

    #[allow(clippy::too_many_lines)] // Keep source selection, guarded import, and revision mapping together.
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
            let registered = self.safe_path(&path)?;
            let renamed = if !registered.exists() && !self.safe_path(&issue.document_path)?.exists()
            {
                self.renamed_delivery_document(
                    &issue.id,
                    &issue.project_id,
                    &issue.name,
                    issue.revision,
                    &crate::naming::numbered_name("", issue.created_at, issue.sequence)?,
                    &path,
                )?
            } else {
                None
            };
            let path = if let Some((relative, _)) = &renamed {
                self.safe_path(relative)?
            } else if registered.exists() {
                registered
            } else {
                self.safe_path(&issue.document_path)?
            };
            if !path.exists() {
                continue;
            }
            let source = std::fs::read_to_string(path)?;
            let (properties, _) = split_properties(&source)?;
            ensure!(
                properties["id"] == issue.id && properties["taskix-generated"] == true,
                "conflict: Issue document identity changed"
            );
            let content = self.projected_issue_content(&issue, &source)?;
            let source_hash = content.hash();
            if self.issue_content_path(&issue)?.exists() {
                continue;
            }
            // Pending publication still carries the previous projected metadata.
            let publication_pending = self.store().document_generation(&key).await?.is_some();
            // A full sync repairs terminal metadata instead of attempting an
            // illegal edit that would block every unrelated document. Targeted
            // sync still reports the rejected edit to the initiating user.
            let restore_terminal_type = selected.is_none() && issue.status.terminal();
            let issue_type: Option<crate::IssueType> =
                if publication_pending || restore_terminal_type {
                    issue.issue_type
                } else if properties["issue_type"].as_str() == Some("") {
                    None
                } else {
                    serde_json::from_value(properties["issue_type"].clone())
                        .context("invalid: unknown Issue type")?
                };
            let type_changed = issue_type != issue.issue_type;
            if type_changed {
                ensure!(
                    properties["revision"].as_i64() == Some(issue.revision),
                    "conflict: Issue type revision changed"
                );
            }
            let title = renamed
                .as_ref()
                .map(|(_, name)| name.as_str())
                .or_else(|| properties["title"].as_str())
                .context("invalid: Issue title must be text")?;
            let title_changed = renamed.is_some() || (title != issue.name && !publication_pending);
            if title_changed {
                ensure!(
                    !title.trim().is_empty() && !title.contains(['\n', '\r']),
                    "invalid: Issue title must be nonblank single-line text"
                );
                ensure!(
                    properties["revision"].as_i64() == Some(issue.revision),
                    "conflict: Issue title revision changed"
                );
            }
            if source_hash != issue.content_hash || title_changed || type_changed {
                let mut request = json!({"command":"issue.update","issue":issue.id,
                    "content_hash":source_hash,"review_content_hash":content.review_hash()});
                if type_changed {
                    request["issue_type"] = json!(issue_type);
                }
                if title_changed {
                    request["title"] = json!(title);
                    request["name"] = json!(title);
                }
                if let Some((relative, _)) = &renamed {
                    Box::pin(self.validate_delivery_rename(issue.revision, &request, relative))
                        .await?;
                }
                let outcome = self
                    .store()
                    .execute_as(
                        request.clone(),
                        WriteOptions {
                            actor_ref: "user:obsidian".into(),
                            expected_revision: Some(issue.revision),
                            ..WriteOptions::default()
                        },
                        request,
                    )
                    .await?;
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
        imported.extend(self.reconcile_milestone_documents_locked(selected).await?);
        Ok(imported)
    }

    #[allow(clippy::too_many_lines)] // Keep source selection, guarded import, and revision mapping together.
    async fn reconcile_milestone_documents_locked(
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
            let registered = self.safe_path(&path)?;
            let renamed = if !registered.exists()
                && !self.safe_path(&milestone.document_path)?.exists()
            {
                self.renamed_delivery_document(
                    &milestone.id,
                    &milestone.project_id,
                    &milestone.name,
                    milestone.revision,
                    &crate::naming::numbered_name("", milestone.created_at, milestone.sequence)?,
                    &path,
                )?
            } else {
                None
            };
            let path = if let Some((relative, _)) = &renamed {
                self.safe_path(relative)?
            } else if registered.exists() {
                registered
            } else {
                self.safe_path(&milestone.document_path)?
            };
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
            let publication_pending = self.store().document_generation(&key).await?.is_some();
            let title = renamed
                .as_ref()
                .map(|(_, name)| name.as_str())
                .or_else(|| properties["title"].as_str())
                .context("invalid: Milestone title must be text")?;
            let title_changed =
                renamed.is_some() || (title != milestone.name && !publication_pending);
            if title_changed {
                ensure!(
                    !title.trim().is_empty() && !title.contains(['\n', '\r']),
                    "invalid: Milestone title must be nonblank single-line text"
                );
                ensure!(
                    properties["revision"].as_i64() == Some(milestone.revision),
                    "conflict: Milestone title revision changed"
                );
            }
            let hash = crate::store::hash_bytes(goal.as_bytes());
            let goal_changed = goal != milestone.goal
                && self
                    .store()
                    .metadata(&format!("delivery:{}", milestone.id))
                    .await?
                    .as_ref()
                    .and_then(Value::as_str)
                    != Some(hash.as_str());
            ensure!(
                !publication_pending || !goal_changed,
                "conflict: Milestone goal was edited while document publication is pending"
            );
            if goal_changed || title_changed {
                let mut request = json!({"command":"milestone.update","milestone":milestone.id});
                if goal_changed {
                    request["goal"] = json!(goal);
                }
                if title_changed {
                    request["title"] = json!(title);
                    request["name"] = json!(title);
                }
                if let Some((relative, _)) = &renamed {
                    Box::pin(self.validate_delivery_rename(milestone.revision, &request, relative))
                        .await?;
                }
                let outcome = self
                    .store()
                    .execute(
                        request,
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
