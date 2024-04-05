use std::{
    convert::Infallible,
    future::Future,
    io::{self, Write},
    net::SocketAddr,
    pin::Pin,
    string::ToString,
    task::Poll,
    time::{Duration, Instant},
};

use http_body_util::Full;
use hyper::{
    body::Body,
    header::{HeaderMap, ACCEPT_ENCODING, CONTENT_ENCODING},
    rt,
    server::conn,
    service, Method, Request, Response, StatusCode,
};
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
/// * It supports gzip compression.
/// * It doesn't support keep-alive connections.
/// * It doesn't support TLS.
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

type ResBody = Full<io::Cursor<Vec<u8>>>;

// Supports only `GET /metrics` requests.
async fn handle(req: Request<impl Body>, ctx: Context) -> Result<Response<ResBody>, Infallible> {
    if req.method() != Method::GET {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(<_>::default())
            .unwrap());
    }

    if req.uri().path() != "/metrics" {
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(<_>::default())
            .unwrap());
    }

    let use_gzip = use_gzip(req.headers());

    ctx.request_to(ctx.addr(), Render)
        .resolve()
        .await
        .map(|Rendered(text)| {
            let builder = Response::builder();

            let gzipped = if use_gzip {
                match try_gzip(text.as_bytes()) {
                    Ok(gzipped) => Some(gzipped),
                    Err(err) => {
                        warn!(error = %err, "failed to gzip metrics, sending uncompressed");
                        None
                    }
                }
            } else {
                None
            };

            if let Some(gzipped) = gzipped {
                builder
                    .header(CONTENT_ENCODING, "gzip")
                    .body(into_res_body(gzipped))
            } else {
                builder.body(into_res_body(text.into_bytes()))
            }
            .unwrap()
        })
        .or_else(|err| {
            warn!(error = %err, "failed to render metrics for HTTP response");

            Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(<_>::default())
                .unwrap())
        })
}

fn use_gzip(headers: &HeaderMap) -> bool {
    let Some(encoding) = headers.get(ACCEPT_ENCODING) else {
        return false;
    };

    let Ok(encoding) = encoding.to_str() else {
        return false;
    };

    encoding.contains("gzip")
}

fn try_gzip(data: &[u8]) -> io::Result<Vec<u8>> {
    let out = Vec::with_capacity(data.len() / 4); // good enough estimation
    let mut encoder = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

fn into_res_body(data: Vec<u8>) -> ResBody {
    Full::new(io::Cursor::new(data))
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
