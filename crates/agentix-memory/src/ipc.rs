use crate::ServiceConfig;
use anyhow::{Context, Result, ensure};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Semaphore, watch},
    task::JoinSet,
};

#[async_trait]
pub trait RequestHandler: Send + Sync {
    async fn handle(&self, request: Value) -> Result<Value>;
}

pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
    _lock: File,
    config: ServiceConfig,
}
impl IpcServer {
    pub fn bind(database: &Path, config: ServiceConfig) -> Result<Self> {
        ensure!(
            (1..=256).contains(&config.max_query_concurrency)
                && (128..=4 * 1024 * 1024).contains(&config.max_request_bytes)
                && (128..=8 * 1024 * 1024).contains(&config.max_response_bytes),
            "invalid IPC budgets"
        );
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
            listener,
            path,
            _lock: lock,
            config,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub async fn serve(
        self,
        handler: Arc<dyn RequestHandler>,
        mut stop: watch::Receiver<bool>,
    ) -> Result<()> {
        let permits = Arc::new(Semaphore::new(
            self.config.max_query_concurrency + self.config.max_deep_queries + 4,
        ));
        let mut tasks = JoinSet::new();
        loop {
            if *stop.borrow() {
                break;
            }
            tokio::select! {
                changed=stop.changed()=>{if changed.is_err() || *stop.borrow(){break;}},
                Some(_)=tasks.join_next(),if !tasks.is_empty()=>{},
                accepted=self.listener.accept()=>{
                    let (stream,_)=accepted?;
                    if !stream.peer_cred().is_ok_and(|credentials| credentials.uid()==uid()){continue;}
                    let Ok(permit)=permits.clone().try_acquire_owned() else {continue;};
                    let handler=handler.clone();let config=self.config;
                    tasks.spawn(async move {let _permit=permit;let _=handle_connection(stream,handler,config).await;});
                }
            }
        }
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        Ok(())
    }
}
impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

pub struct IpcClient {
    path: PathBuf,
    config: ServiceConfig,
}
impl IpcClient {
    pub fn new(database: &Path, config: ServiceConfig) -> Result<Self> {
        Ok(Self {
            path: socket_path(database)?,
            config,
        })
    }
    pub async fn call(&self, request: Value) -> Result<Value> {
        self.call_with_timeout(request, Duration::from_secs(10))
            .await
    }
    pub async fn call_with_timeout(&self, request: Value, timeout: Duration) -> Result<Value> {
        tokio::time::timeout(timeout, self.call_inner(request))
            .await
            .context("memory service request timeout")?
    }
    async fn call_inner(&self, request: Value) -> Result<Value> {
        check_private(self.path.parent().context("missing socket parent")?, true)?;
        check_private(&self.path, false)?;
        let mut stream = UnixStream::connect(&self.path)
            .await
            .context("memory service unavailable")?;
        ensure!(
            stream.peer_cred()?.uid() == uid(),
            "foreign memory service peer"
        );
        write_frame(
            &mut stream,
            &json!({"version":1,"request":request}),
            self.config.max_request_bytes,
        )
        .await?;
        let reply = read_frame(&mut stream, self.config.max_response_bytes).await?;
        ensure!(reply["version"] == 1, "unsupported memory service protocol");
        ensure!(
            reply["ok"] == true,
            "{}",
            reply["error"]
                .as_str()
                .unwrap_or("memory service request failed")
        );
        Ok(reply["result"].clone())
    }
}
fn uid() -> u32 {
    rustix::process::geteuid().as_raw()
}
fn socket_path(database: &Path) -> Result<PathBuf> {
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
async fn handle_connection(
    mut stream: UnixStream,
    handler: Arc<dyn RequestHandler>,
    config: ServiceConfig,
) -> Result<()> {
    let request = tokio::time::timeout(
        Duration::from_secs(5),
        read_frame(&mut stream, config.max_request_bytes),
    )
    .await??;
    let result = if request["version"] == 1 {
        handler.handle(request["request"].clone()).await
    } else {
        Err(anyhow::anyhow!("unsupported memory service protocol"))
    };
    let mut reply = match result {
        Ok(result) => json!({"version":1,"ok":true,"result":result}),
        Err(error) => json!({"version":1,"ok":false,"error":error.to_string()}),
    };
    if serde_json::to_vec(&reply)?.len() > config.max_response_bytes {
        reply = json!({"version":1,"ok":false,"error":"memory response exceeds budget; use a smaller page"});
    }
    tokio::time::timeout(
        Duration::from_secs(5),
        write_frame(&mut stream, &reply, config.max_response_bytes),
    )
    .await??;
    Ok(())
}
async fn read_frame(stream: &mut UnixStream, limit: usize) -> Result<Value> {
    let length = usize::try_from(stream.read_u32().await?)?;
    ensure!(
        length > 0 && length <= limit,
        "memory request or response exceeds budget"
    );
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
async fn write_frame(stream: &mut UnixStream, value: &Value, limit: usize) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(
        bytes.len() <= limit,
        "memory request or response exceeds budget"
    );
    stream.write_u32(u32::try_from(bytes.len())?).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}
