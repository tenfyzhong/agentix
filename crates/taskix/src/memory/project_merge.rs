use agentix_memory::{MemoryLocation, MemoryProjection, MemoryStore};
use agentix_task::{Service, Store};
use anyhow::{Result, ensure};
use std::path::Path;

pub(super) async fn migrate(tasks: &Store, memories: &MemoryStore) -> Result<()> {
    for (source, target) in tasks.project_merges().await? {
        memories.merge_project(&source, &target).await?;
    }
    Ok(())
}

pub(crate) async fn sync(path: &Path, service: &Service) -> Result<()> {
    let merges = service.store().project_merges().await?;
    if merges.is_empty() {
        return Ok(());
    }
    let location = MemoryLocation::load(path)?;
    if !location.path.exists() {
        return Ok(());
    }
    let _guard = service.lock_output().await?;
    let store = MemoryStore::open(&location.path).await?;
    migrate(service.store(), &store).await?;
    let config = service.config();
    let targets: std::collections::BTreeSet<_> =
        merges.into_iter().map(|(_, target)| target).collect();
    for target in targets {
        let project = service.store().project_result(&target).await?;
        let directory = config
            .vault_relative_path(Path::new(&project.document_directory()))
            .join("Memory");
        let projection = MemoryProjection::new(store.clone(), &config.documents.root, &directory)?
            .with_project_link(config.project_link(&project));
        let mut after = String::new();
        loop {
            let page = projection.sync(&target, &after, 100).await?;
            ensure!(
                page.conflicts.is_empty(),
                "Project memory migration requires resolving document conflicts and running taskix sync"
            );
            if page.complete {
                break;
            }
            after = page.next_cursor;
        }
    }
    Ok(())
}
