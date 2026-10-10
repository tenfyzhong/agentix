//! Project/status/Milestone predicates run before pagination and body loading.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::{
    Issue, IssueStatus, Milestone, Store,
    scoped::{entities, resolve},
};

#[derive(Debug, Default)]
pub struct IssueFilter {
    pub status: Option<IssueStatus>,
    pub milestone_id: Option<String>,
    pub archived: Option<bool>,
}

fn predicates<'a>(
    query: &mut QueryBuilder<'a, Sqlite>,
    project: Option<&'a str>,
    filter: &'a IssueFilter,
) {
    if let Some(project) = project {
        query.push(" AND project_id=").push_bind(project);
    }
    if let Some(status) = filter.status {
        query
            .push(" AND json_extract(data,'$.status')=")
            .push_bind(status.to_string());
    }
    if let Some(milestone) = &filter.milestone_id {
        query
            .push(" AND json_extract(data,'$.milestone_id')=")
            .push_bind(milestone);
    }
    if let Some(archived) = filter.archived {
        query.push(if archived {
            " AND json_extract(data,'$.archived_at') IS NOT NULL"
        } else {
            " AND json_extract(data,'$.archived_at') IS NULL"
        });
    }
}

impl Store {
    /// Related Job metadata is filtered before six-item IM pagination.
    pub async fn issue_job_page(
        &self,
        id: &str,
        status: Option<crate::JobStatus>,
        archived: Option<bool>,
        offset: i64,
        limit: i64,
    ) -> Result<Value> {
        ensure!(
            offset >= 0 && (1..=100).contains(&limit),
            "invalid: Issue Job page bounds"
        );
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "issues", id).await?;
        let condition = " FROM jobs WHERE json_extract(data,'$.issue_id')=?1 AND (?2 IS NULL OR json_extract(data,'$.status')=?2) AND (?3 IS NULL OR (json_extract(data,'$.archived_at') IS NOT NULL)=?3)";
        let total: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*){condition}"))
            .bind(&id)
            .bind(status.map(|s| s.to_string()))
            .bind(archived)
            .fetch_one(&mut *tx)
            .await?;
        let rows:Vec<String>=sqlx::query_scalar(&format!("SELECT json_remove(data,'$.goal','$.prompt','$.conversation','$.followup_task_ids'){condition} ORDER BY id DESC LIMIT ?4 OFFSET ?5"))
            .bind(&id).bind(status.map(|s|s.to_string())).bind(archived).bind(limit).bind(offset).fetch_all(&mut *tx).await?;
        let items = rows
            .iter()
            .map(|s| crate::stored_paths::from_str::<Value>(s))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(json!({"total":total,"items":items}))
    }
    pub async fn issue_record(&self, id: &str) -> Result<Issue> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "issues", id).await?;
        Ok(
            entities(&mut tx, "issues", &std::collections::BTreeSet::from([id]))
                .await?
                .remove(0),
        )
    }

    pub async fn issue_detail(&self, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "issues", id).await?;
        let issue: Issue = entities(&mut tx, "issues", &std::collections::BTreeSet::from([id]))
            .await?
            .remove(0);
        let jobs:Vec<String>=sqlx::query_scalar("SELECT json_remove(data,'$.goal','$.prompt','$.conversation','$.followup_task_ids') FROM jobs WHERE json_extract(data,'$.issue_id')=? ORDER BY rowid")
            .bind(&issue.id).fetch_all(&mut *tx).await?;
        let milestone: Option<Milestone> = entities(
            &mut tx,
            "milestones",
            &issue.milestone_id.iter().cloned().collect(),
        )
        .await?
        .pop();
        let jobs = jobs
            .iter()
            .map(|s| crate::stored_paths::from_str::<Value>(s))
            .collect::<Result<Vec<_>>>()?;
        tx.commit().await?;
        Ok(json!({"issue":issue,"jobs":jobs,"milestone":milestone}))
    }

    pub async fn issue_page(
        &self,
        project: Option<&str>,
        filter: &IssueFilter,
        after: Option<&str>,
        limit: i64,
    ) -> Result<Value> {
        self.read_issue_page(project, filter, after, 0, limit).await
    }

    pub async fn issue_offset_page(
        &self,
        project: Option<&str>,
        filter: &IssueFilter,
        offset: i64,
        limit: i64,
    ) -> Result<Value> {
        ensure!(offset >= 0, "invalid: Issue offset");
        self.read_issue_page(project, filter, None, offset, limit)
            .await
    }

    async fn read_issue_page(
        &self,
        project: Option<&str>,
        filter: &IssueFilter,
        after: Option<&str>,
        offset: i64,
        limit: i64,
    ) -> Result<Value> {
        ensure!(
            (1..=100).contains(&limit),
            "invalid: Issue page limit must be 1..100"
        );
        ensure!(
            after.is_none_or(|id| id.starts_with("issue_") && id.len() == 38),
            "invalid: Issue cursor"
        );
        let mut tx = self.pool.begin().await?;
        let project = match project {
            Some(id) => Some(resolve(&mut tx, "projects", id).await?),
            None => None,
        };
        let filter = IssueFilter {
            status: filter.status,
            archived: filter.archived,
            milestone_id: match &filter.milestone_id {
                Some(id) => Some(resolve(&mut tx, "milestones", id).await?),
                None => None,
            },
        };
        let mut query = QueryBuilder::<Sqlite>::new("SELECT COUNT(*) FROM issues WHERE 1");
        predicates(&mut query, project.as_deref(), &filter);
        let total: i64 = query.build_query_scalar().fetch_one(&mut *tx).await?;
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT json_remove(data,'$.requirement','$.acceptance','$.notes') FROM issues WHERE 1",
        );
        predicates(&mut query, project.as_deref(), &filter);
        if let Some(after) = after {
            query.push(" AND id>").push_bind(after);
        }
        query
            .push(" ORDER BY id LIMIT ")
            .push_bind(limit + 1)
            .push(" OFFSET ")
            .push_bind(offset);
        let rows: Vec<String> = query.build_query_scalar().fetch_all(&mut *tx).await?;
        let complete = rows.len() <= usize::try_from(limit)?;
        let items = rows
            .iter()
            .take(usize::try_from(limit)?)
            .map(|s| crate::stored_paths::from_str::<Value>(s))
            .collect::<Result<Vec<_>>>()?;
        let next_cursor = if complete {
            None
        } else {
            items
                .last()
                .and_then(|v| v["id"].as_str())
                .map(str::to_owned)
        };
        tx.commit().await?;
        Ok(json!({"items":items,"total":total,"complete":complete,"next_cursor":next_cursor}))
    }

    pub async fn milestone_record(&self, id: &str) -> Result<Milestone> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "milestones", id).await?;
        Ok(entities(
            &mut tx,
            "milestones",
            &std::collections::BTreeSet::from([id]),
        )
        .await?
        .remove(0))
    }

    pub async fn milestone_detail(&self, id: &str) -> Result<Value> {
        let mut tx = self.pool.begin().await?;
        let id = resolve(&mut tx, "milestones", id).await?;
        let milestone: Milestone = entities(
            &mut tx,
            "milestones",
            &std::collections::BTreeSet::from([id]),
        )
        .await?
        .remove(0);
        let row=sqlx::query("SELECT COUNT(*) AS total,COALESCE(SUM(json_extract(data,'$.status')='COMPLETED'),0) AS completed,COALESCE(SUM(json_extract(data,'$.status')='CANCELLED'),0) AS cancelled FROM issues WHERE json_extract(data,'$.milestone_id')=?")
            .bind(&milestone.id).fetch_one(&mut *tx).await?;
        let total: i64 = row.get("total");
        let completed: i64 = row.get("completed");
        let cancelled: i64 = row.get("cancelled");
        tx.commit().await?;
        Ok(
            json!({"milestone":milestone,"progress":{"total":total,"completed":completed,"cancelled":cancelled,"remaining":total-completed-cancelled}}),
        )
    }

    pub async fn milestones(&self, project: Option<&str>) -> Result<Vec<Milestone>> {
        let mut tx = self.pool.begin().await?;
        let mut query = QueryBuilder::<Sqlite>::new("SELECT data FROM milestones WHERE 1");
        if let Some(project) = project {
            query
                .push(" AND project_id=")
                .push_bind(resolve(&mut tx, "projects", project).await?);
        }
        query.push(" ORDER BY json_extract(data,'$.order'),id");
        let rows: Vec<String> = query.build_query_scalar().fetch_all(&mut *tx).await?;
        tx.commit().await?;
        rows.iter()
            .map(|s| crate::stored_paths::from_str(s))
            .collect()
    }
}
