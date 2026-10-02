//! Relocate project projections under the output lock; workspace roots never move.
use anyhow::{Context, Result, ensure};
use serde_json::json;
use sqlx::Row;

use crate::{Project, Service, WriteOptions};

impl Service {
    /// Move pre-v18 output-relative archives without retaining configuration paths.
    pub(crate) async fn migrate_legacy_archive_folders_locked(&self) -> Result<()> {
        let rows = sqlx::query("SELECT p.data,d.path FROM projects p JOIN document_registry d ON d.key='board:'||p.id WHERE json_extract(p.data,'$.archived_at') IS NOT NULL")
            .fetch_all(&self.store().pool).await?;
        for row in rows {
            let project: Project = crate::stored_paths::from_str(&row.get::<String, _>("data"))?;
            let relative = project.document_directory();
            if !std::path::Path::new(&relative).starts_with("Archived Projects") {
                continue;
            }
            let destination = self.safe_path(&relative)?;
            let source = self.config().output_dir().join(&relative);
            if destination.exists() || !source.exists() {
                continue;
            }
            ensure!(
                source
                    .canonicalize()?
                    .starts_with(self.config().output_dir().canonicalize()?),
                "legacy archive escapes document output"
            );
            let board = source.join("Board.md");
            ensure!(
                board.canonicalize()?.starts_with(source.canonicalize()?),
                "legacy archive Board escapes Project folder"
            );
            let (properties, _) =
                crate::projection::split_properties(&std::fs::read_to_string(board)?)?;
            ensure!(
                properties["taskix-generated"] == true
                    && properties["id"] == project.id
                    && properties["root"] == project.root
                    && row.get::<String, _>("path") == format!("{relative}/Board.md"),
                "conflict: legacy archive is not the registered Project"
            );
            std::fs::create_dir_all(destination.parent().context("missing archive parent")?)?;
            std::fs::rename(source, destination)?;
        }
        Ok(())
    }

    pub(crate) async fn relocate_archived_projects_locked(&self) -> Result<()> {
        let rows = sqlx::query("SELECT p.data,d.path FROM projects p LEFT JOIN document_registry d ON d.key='board:'||p.id")
            .fetch_all(&self.store().pool).await?;
        for row in rows {
            let project: Project = crate::stored_paths::from_str(&row.get::<String, _>("data"))?;
            let old = project.document_directory();
            let new = if project.archived_at.is_some() {
                format!("Archived Projects/{}", project.key)
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
                let board = self.safe_path(&format!("{new}/Board.md"))?;
                let source = std::fs::read_to_string(board)?;
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
