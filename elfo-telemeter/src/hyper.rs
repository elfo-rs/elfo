use std::{
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    string::ToString,
    task::Poll,
    time::{Duration, Instant},
};

use hyper::{body::Body, rt, server::conn, service, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use pin_project_lite::pin_project;
use tokio::{net::TcpListener, time::timeout};
use tracing::{debug, info, warn};

use elfo_core::{scope, tracing::TraceId, Context};

use crate::protocol::{Render, Rendered, ServerFailed};

const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(3);
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs a simple HTTP server that responds to `GET /metrics` requests.
/// * It supports only HTTP/1.
/// * It doesn't support keep-alive connections.
/// * It doesn't support TLS.
/// * It doesn't support compression.
/// * It handles requests one by one with some reasonable timeouts.
pub(crate) async fn server(addr: SocketAddr, ctx: Context) -> ServerFailed {
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(err) => return ServerFailed(format!("cannot bind a listener: {err}")),
    };

    info!(bind = %addr, "listening TCP connections");

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(err) => return ServerFailed(format!("cannot accept a connection: {err}")),
        };

        // The server doesn't support keep-alive connections, so every connection is a
        // new request. Thus, we can start a new trace right here.
        scope::set_trace_id(TraceId::generate());

        debug!(peer = %peer, "accepted a TCP connection");
        let ctx = ctx.clone();

        let serving = conn::http1::Builder::new()
            .timer(TokioTimer)
            .keep_alive(false) // KA is meaningless for rare requests.
            .header_read_timeout(HEADER_READ_TIMEOUT)
            .serve_connection(
                TokioIo::new(stream),
                service::service_fn(move |req| handle(req, ctx.clone())),
            );

        match flat_error(timeout(SERVE_TIMEOUT, serving).await) {
            Ok(()) => debug!(peer = %peer, "finished serving a HTTP connection"),
            Err(err) => warn!(
                message = "failed to serve a HTTP connection",
                error = %err,
                peer = %peer,
            ),
        }
    }
}

// Supports only `GET /metrics` requests.
async fn handle(req: Request<impl Body>, ctx: Context) -> Result<Response<String>, Infallible> {
    if req.method() != Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(String::new())
            .unwrap());
    }

    if req.uri().path() != "/metrics" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(String::new())
            .unwrap());
    }

    ctx.request_to(ctx.addr(), Render)
        .resolve()
        .await
        .map(|Rendered(text)| Response::new(text))
        .or_else(|err| {
            warn!(error = %err, "failed to render metrics for HTTP response");

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(String::new())
                .unwrap())
        })
}

fn flat_error(res: Result<Result<(), impl ToString>, impl ToString>) -> Result<(), String> {
    match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(err) => Err(err.to_string()),
    }
}

// === TokioTimer ===
// TODO: Replace once https://github.com/hyperium/hyper-util/pull/73 is released.
//       Don't forget to remove `pin-project-lite` from `Cargo.toml`.

#[derive(Clone, Debug)]
struct TokioTimer;

impl rt::Timer for TokioTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn rt::Sleep>> {
        Box::pin(TokioSleep {
            inner: tokio::time::sleep(duration),
        })
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn rt::Sleep>> {
        Box::pin(TokioSleep {
            inner: tokio::time::sleep_until(deadline.into()),
        })
    }

    fn reset(&self, sleep: &mut Pin<Box<dyn rt::Sleep>>, new_deadline: Instant) {
        if let Some(sleep) = sleep.as_mut().downcast_mut_pin::<TokioSleep>() {
            sleep.reset(new_deadline)
        }
    }
}

pin_project! {
    struct TokioSleep {
        #[pin]
        inner: tokio::time::Sleep,
    }
}

impl Future for TokioSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        self.project().inner.poll(cx)
    }
}

impl rt::Sleep for TokioSleep {}

impl TokioSleep {
    fn reset(self: Pin<&mut Self>, deadline: Instant) {
        self.project().inner.as_mut().reset(deadline.into());
    }
}
