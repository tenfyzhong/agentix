use crate::Metrics;
use anyhow::Result;
use bytes::Bytes;
use http_body_util::Full;
use hyper::{Request, Response, StatusCode, body::Incoming, service::service_fn};
use hyper_util::rt::TokioIo;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::Semaphore,
    task::{JoinHandle, JoinSet},
};

pub struct Exporter {
    address: SocketAddr,
    task: JoinHandle<()>,
    _process: Option<crate::process::Sampler>,
}
impl Exporter {
    #[must_use]
    pub fn address(&self) -> SocketAddr {
        self.address
    }
}
impl Drop for Exporter {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Metrics {
    pub async fn serve(&self, address: SocketAddr) -> Result<Exporter> {
        anyhow::ensure!(
            address.ip().is_loopback(),
            "metrics listener must be loopback"
        );
        let listener = TcpListener::bind(address).await?;
        let address = listener.local_addr()?;
        let metrics = self.clone();
        let task = tokio::spawn(async move {
            let permits = Arc::new(Semaphore::new(4));
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    _=connections.join_next(),if !connections.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let Ok((stream,_))=accepted else {break};
                        let Ok(permit)=permits.clone().try_acquire_owned() else {continue};
                        let metrics=metrics.clone();
                        connections.spawn(async move {
                            let _permit=permit;
                            let service=service_fn(move |request:Request<Incoming>|{
                                let metrics=metrics.clone();async move {
                                    let (status,body)=if request.uri().path()!="/metrics"{(StatusCode::NOT_FOUND,String::new())}
                                    else if request.method()!=hyper::Method::GET{(StatusCode::METHOD_NOT_ALLOWED,String::new())}
                                    else {match metrics.encode(){Ok(body)=>(StatusCode::OK,body),Err(error)=>{tracing::warn!(%error,"application metrics encoding failed");(StatusCode::INTERNAL_SERVER_ERROR,String::new())}}};
                                    Ok::<_,std::convert::Infallible>(Response::builder().status(status).header("content-type","application/openmetrics-text; version=1.0.0; charset=utf-8").body(Full::new(Bytes::from(body))).unwrap())
                                }
                            });
                            let _=tokio::time::timeout(Duration::from_secs(5),hyper::server::conn::http1::Builder::new().keep_alive(false).max_buf_size(8192).serve_connection(TokioIo::new(stream),service)).await;
                        });
                    }
                }
            }
        });
        Ok(Exporter {
            address,
            task,
            _process: crate::process::start(self.clone()),
        })
    }
}
