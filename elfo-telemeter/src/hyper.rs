use std::{
    convert::Infallible,
    io::{self, Write},
    net::SocketAddr,
    string::ToString,
    time::Duration,
};

use http_body_util::Full;
use hyper::{
    body::Body,
    header::{HeaderMap, ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE},
    server::conn,
    service, Method, Request, Response, StatusCode,
};
use hyper_util::rt::{TokioIo, TokioTimer};
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
            .timer(TokioTimer::new())
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

    // Actually make a request to the telemeter actor to render metrics.
    let text = match ctx.request_to(ctx.addr(), Render).resolve().await {
        Ok(Rendered(text)) => text,
        Err(err) => {
            warn!(error = %err, "failed to render metrics for HTTP response");

            return Ok(Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(<_>::default())
                .unwrap());
        }
    };

    let builder = Response::builder();

    // We must set the Content-Type header. Otherwise, the prometheus rejects
    // responses printing "Failed to determine correct type of scrape target".
    //
    // Formally, we format metrics according to openmetrics v1, but still use simple
    // "text/plain" to avoid overcomplications to support older prometheus versions.
    //
    // Prometheus: https://github.com/prometheus/prometheus/blob/21fb899c3292829ec49b5fef63b3291bdc8a519d/config/config.go#L564-L570
    // VictoriaMetrics: https://github.com/VictoriaMetrics/VictoriaMetrics/blob/1b7f0172d2c7d1c90eef5b235ba8459e4ac5522c/lib/promscrape/client.go#L133
    let builder = builder.header(CONTENT_TYPE, "text/plain; charset=utf-8");

    let gzipped = if use_gzip(req.headers()) {
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

    Ok(if let Some(gzipped) = gzipped {
        builder
            .header(CONTENT_ENCODING, "gzip")
            .body(into_res_body(gzipped))
    } else {
        builder.body(into_res_body(text.into_bytes()))
    }
    .unwrap())
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
