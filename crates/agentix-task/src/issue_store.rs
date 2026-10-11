//! Indexed persistence and audit events for project deliverables.
use anyhow::Result;
use serde_json::json;
use sqlx::SqliteConnection;

use crate::{
    Snapshot, TaskEvent, WriteOptions,
    state_index::StateIndex,
    store::{append_event, event, upsert},
};

pub(crate) async fn persist(
    conn: &mut SqliteConnection,
    before: &Snapshot,
    after: &Snapshot,
    indexes: (&StateIndex<'_>, &StateIndex<'_>),
    command: &str,
    options: &WriteOptions,
    now: i64,
) -> Result<()> {
    let (old, new) = indexes;
    for stage in &after.milestones {
        if old.milestones.get(stage.id.as_str()).copied() == Some(stage) {
            continue;
        }
        upsert(conn, "milestones", &stage.id, stage).await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(stage.project_id.clone()),
                revision: stage.revision,
                payload: crate::event_payload::compact_payload(
                    &json!({"id":stage.id,"title":stage.title}),
                ),
                ..event(command, options, now)
            },
        )
        .await?;
    }
    for issue in &after.issues {
        let previous = old.issues.get(issue.id.as_str()).copied();
        if previous == Some(issue) {
            continue;
        }
        require_migrated_content(conn, &issue.id).await?;
        upsert(conn, "issues", &issue.id, issue).await?;
        let kind = if previous.is_some_and(|previous| previous.status != issue.status) {
            format!("issue.{}", issue.status.to_string().to_lowercase())
        } else {
            command.into()
        };
        append_event(conn,TaskEvent {
            project_id:Some(issue.project_id.clone()),revision:issue.revision,
            payload:crate::event_payload::compact_payload(&json!({"id":issue.id,"title":issue.title,"status":issue.status,"milestone_id":issue.milestone_id})),
            ..event(&kind,options,now)
        }).await?;
    }
    for (table, id, project, revision, path) in before
        .issues
        .iter()
        .filter(|issue| !new.issues.contains_key(issue.id.as_str()))
        .map(|issue| {
            (
                "issues",
                &issue.id,
                &issue.project_id,
                issue.revision,
                &issue.document_path,
            )
        })
        .chain(
            before
                .milestones
                .iter()
                .filter(|stage| !new.milestones.contains_key(stage.id.as_str()))
                .map(|stage| {
                    (
                        "milestones",
                        &stage.id,
                        &stage.project_id,
                        stage.revision,
                        &stage.document_path,
                    )
                }),
        )
    {
        sqlx::query(&format!("DELETE FROM {table} WHERE id=?"))
            .bind(id)
            .execute(&mut *conn)
            .await?;
        append_event(
            conn,
            TaskEvent {
                project_id: Some(project.clone()),
                revision: revision + 1,
                payload: crate::event_payload::compact_payload(&json!({"id":id,"deleted":true})),
                ..event(command, options, now)
            },
        )
        .await?;
        // Durable cleanup is scoped to this generated note; the publisher verifies identity.
        let cleanup = crate::deletion::Cleanup {
            id: crate::new_id("cleanup"),
            files: std::collections::BTreeSet::new(),
            directories: std::collections::BTreeSet::new(),
            candidates: std::collections::BTreeMap::from([(
                path.clone(),
                std::collections::BTreeSet::from([id.clone()]),
            )]),
        };
        sqlx::query("INSERT INTO document_deletions(id,data) VALUES (?,?)")
            .bind(&cleanup.id)
            .bind(serde_json::to_string(&cleanup)?)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn require_migrated_content(conn: &mut SqliteConnection, id: &str) -> Result<()> {
    let legacy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM issues WHERE id=? AND (json_type(data,'$.requirement') IS NOT NULL OR json_type(data,'$.acceptance') IS NOT NULL OR json_type(data,'$.notes') IS NOT NULL))")
        .bind(id).fetch_one(conn).await?;
    anyhow::ensure!(
        !legacy,
        "conflict: synchronize Issue documents to migrate legacy content before writing"
    );
    Ok(())
}
