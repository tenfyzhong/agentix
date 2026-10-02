//! Relocate project projections under the output lock; workspace roots never move.
use anyhow::{Context, Result, ensure};
use serde_json::json;
use sqlx::Row;

use crate::{Project, Service, WriteOptions};

impl Service {
    pub(crate) async fn relocate_archived_projects_locked(&self) -> Result<()> {
        let rows = sqlx::query("SELECT p.data,d.path FROM projects p LEFT JOIN document_registry d ON d.key='board:'||p.id")
            .fetch_all(&self.store().pool).await?;
        for row in rows {
            let project: Project = serde_json::from_str(&row.get::<String, _>("data"))?;
            let old = project.document_directory();
            let new = if project.archived_at.is_some() {
                self.config()
                    .archive_dir()
                    .join(&project.key)
                    .to_string_lossy()
                    .replace('\\', "/")
            } else {
                format!("Projects/{}", project.key)
            };
            if old == new {
                continue;
            }
            let source = self.safe_path(&old)?;
            let destination = self.safe_path(&new)?;
            if source.exists() {
                // Legacy vault-root output can already occupy the absolute target.
                if !destination.exists() || source.canonicalize()? != destination.canonicalize()? {
                    ensure!(
                        !destination.exists(),
                        "conflict: archive destination already exists: {}",
                        destination.display()
                    );
                    std::fs::create_dir_all(
                        destination.parent().context("missing archive parent")?,
                    )?;
                    std::fs::rename(&source, &destination)
                        .with_context(|| format!("move project documents from {old} to {new}"))?;
                }
            } else if destination.exists() {
                // Recover a crash between the filesystem rename and the DB commit.
                let source = std::fs::read_to_string(destination.join("Board.md"))?;
                let (properties, _) = crate::projection::split_properties(&source)?;
                ensure!(
                    properties["taskix-generated"] == true
                        && properties["id"] == project.id
                        && properties["root"] == project.root,
                    "conflict: archive destination is not the registered Project"
                );
                ensure!(
                    row.get::<Option<String>, _>("path").as_deref()
                        == Some(format!("{old}/Board.md").as_str()),
                    "conflict: no registered source for project move recovery"
                );
            } else {
                ensure!(
                    row.get::<Option<String>, _>("path").is_none(),
                    "conflict: published project folder is missing: {old}"
                );
            }
            self.store().execute(json!({"command":"project.relocate","project":project.id,"name":project.name,"directory":new}),
                WriteOptions { expected_revision: Some(project.revision), actor_ref: "system:projection".into(), ..WriteOptions::default() }).await?;
        }
        Ok(())
    }
}
