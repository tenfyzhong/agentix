use anyhow::{Context, Result, ensure};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};
use tokio::net::{UnixListener, UnixStream};
pub(super) struct Listener {
    inner: UnixListener,
    path: PathBuf,
    _lock: File,
}
impl Listener {
    pub(super) fn bind(database: &Path) -> Result<Self> {
        let path = socket_path(database)?;
        let parent = path.parent().context("missing socket parent")?;
        match std::fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        check_private(parent, true)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(i32::try_from(rustix::fs::OFlags::NOFOLLOW.bits())?)
            .open(path.with_extension("lock"))?;
        ensure!(
            lock.metadata()?.uid() == uid(),
            "foreign memory service lock"
        );
        lock.try_lock()
            .context("conflict: memory service already running")?;
        if path.exists() {
            check_private(&path, false)?;
            ensure!(
                std::fs::symlink_metadata(&path)?.file_type().is_socket(),
                "refusing to replace a non-socket path"
            );
            std::fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok(Self {
            inner: listener,
            path,
            _lock: lock,
        })
    }
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
    pub(super) async fn accept(&self) -> Result<UnixStream> {
        loop {
            let (stream, _) = self.inner.accept().await?;
            if stream.peer_cred().is_ok_and(|cred| cred.uid() == uid()) {
                return Ok(stream);
            }
        }
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
pub(super) async fn connect(path: &Path) -> Result<UnixStream> {
    check_private(path.parent().context("missing socket parent")?, true)?;
    check_private(path, false)?;
    let stream = UnixStream::connect(path)
        .await
        .context("memory service unavailable")?;
    ensure!(
        stream.peer_cred()?.uid() == uid(),
        "foreign memory service peer"
    );
    Ok(stream)
}
fn uid() -> u32 {
    rustix::process::geteuid().as_raw()
}
pub(super) fn socket_path(database: &Path) -> Result<PathBuf> {
    let canonical = if database.exists() {
        database.canonicalize()?
    } else {
        database
            .parent()
            .context("missing database parent")?
            .canonicalize()?
            .join(database.file_name().context("missing database filename")?)
    };
    let key = crate::retrieval::digest(&canonical.to_string_lossy());
    Ok(PathBuf::from(format!("/tmp/taskix-memory-{}", uid())).join(format!("{}.sock", &key[..24])))
}
fn check_private(path: &Path, directory: bool) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    ensure!(
        meta.uid() == uid()
            && !meta.file_type().is_symlink()
            && (!directory || meta.is_dir())
            && meta.permissions().mode().trailing_zeros() >= 6,
        "memory IPC path must be private and owned by the current user"
    );
    Ok(())
}
