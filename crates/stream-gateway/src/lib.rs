//! Capability-protected loopback HTTP streaming for torrent files.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE};
use axum::http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
use axum::routing::get;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio_util::io::ReaderStream;
use torrent_engine::TorrentEngine;

/// Streaming gateway failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Loopback listener could not bind.
    #[error("failed to bind loopback stream listener: {0}")]
    Bind(#[source] std::io::Error),
    /// Operating-system randomness was unavailable.
    #[error("failed to generate stream capability: {0}")]
    Random(#[source] getrandom::Error),
    /// The shutdown lock was poisoned.
    #[error("stream gateway shutdown lock poisoned")]
    LockPoisoned,
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
struct GatewayState {
    engine: Arc<TorrentEngine>,
    token_hash: blake3::Hash,
}

/// Running loopback stream server.
#[derive(Debug)]
pub struct StreamGateway {
    address: SocketAddr,
    token: String,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl StreamGateway {
    /// Start on an operating-system-assigned loopback port.
    ///
    /// # Errors
    ///
    /// Returns bind or cryptographic-randomness failures.
    pub async fn start(engine: Arc<TorrentEngine>) -> Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(Error::Bind)?;
        let address = listener.local_addr().map_err(Error::Bind)?;
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(Error::Random)?;
        let token = hex_encode(secret);
        let state = GatewayState {
            engine,
            token_hash: blake3::hash(token.as_bytes()),
        };
        let router = Router::new()
            .route(
                "/stream/{token}/{torrent_id}/{file_index}",
                get(stream_file).head(stream_file),
            )
            .with_state(state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        tokio::spawn(async move {
            let server = axum::serve(listener, router).with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            });
            if let Err(error) = server.await {
                eprintln!("loopback stream server stopped with error: {error}");
            }
        });
        Ok(Self {
            address,
            token,
            shutdown: Mutex::new(Some(shutdown_tx)),
        })
    }

    /// Capability URL for a managed torrent file.
    #[must_use]
    pub fn url(&self, torrent_id: usize, file_index: usize) -> String {
        format!(
            "http://{}/stream/{}/{torrent_id}/{file_index}",
            self.address, self.token
        )
    }

    /// Bound loopback address.
    #[must_use]
    pub const fn address(&self) -> SocketAddr {
        self.address
    }

    /// Stop accepting new stream requests.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LockPoisoned`] if another thread panicked while
    /// holding the shutdown lock.
    pub fn shutdown(&self) -> Result<()> {
        if let Some(sender) = self
            .shutdown
            .lock()
            .map_err(|_| Error::LockPoisoned)?
            .take()
        {
            let _ = sender.send(());
        }
        Ok(())
    }
}

impl Drop for StreamGateway {
    fn drop(&mut self) {
        if let Ok(slot) = self.shutdown.get_mut()
            && let Some(sender) = slot.take()
        {
            let _ = sender.send(());
        }
    }
}

async fn stream_file(
    State(state): State<GatewayState>,
    Path((token, torrent_id, file_index)): Path<(String, usize, usize)>,
    method: Method,
    headers: HeaderMap,
) -> Response<Body> {
    if blake3::hash(token.as_bytes()) != state.token_hash {
        return plain_error(StatusCode::UNAUTHORIZED, "invalid stream capability");
    }
    let Some(torrent) = state.engine.get(torrent_id) else {
        return plain_error(StatusCode::NOT_FOUND, "unknown torrent");
    };
    let metadata = match torrent.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return plain_error(StatusCode::CONFLICT, &error.to_string()),
    };
    let Some(file) = metadata.files.get(file_index) else {
        return plain_error(StatusCode::NOT_FOUND, "unknown torrent file");
    };
    let range = match parse_range(headers.get(RANGE), file.length) {
        Ok(range) => range,
        Err(message) => {
            let mut response = plain_error(StatusCode::RANGE_NOT_SATISFIABLE, message);
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{}", file.length))
                    .expect("valid unsatisfied content range"),
            );
            return response;
        }
    };
    let length = range.end - range.start + 1;
    let mut response = Response::builder()
        .status(if range.partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(ACCEPT_RANGES, "bytes")
        .header(CONTENT_LENGTH, length)
        .header(
            CONTENT_TYPE,
            mime_guess::from_path(&file.path)
                .first_or_octet_stream()
                .as_ref(),
        );
    if range.partial {
        response = response.header(
            CONTENT_RANGE,
            format!("bytes {}-{}/{}", range.start, range.end, file.length),
        );
    }
    if method == Method::HEAD {
        return response
            .body(Body::empty())
            .expect("valid HEAD stream response");
    }
    let mut stream = match torrent.stream_file(file_index) {
        Ok(stream) => stream,
        Err(error) => return plain_error(StatusCode::CONFLICT, &error.to_string()),
    };
    if let Err(error) = stream.seek(std::io::SeekFrom::Start(range.start)).await {
        return plain_error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string());
    }
    let body = Body::from_stream(ReaderStream::new(stream.take(length)));
    response.body(body).expect("valid stream response")
}

#[derive(Debug, Clone, Copy)]
struct ByteRange {
    start: u64,
    end: u64,
    partial: bool,
}

fn parse_range(
    header: Option<&HeaderValue>,
    file_length: u64,
) -> std::result::Result<ByteRange, &'static str> {
    if file_length == 0 {
        return Err("empty files cannot be streamed");
    }
    let Some(header) = header else {
        return Ok(ByteRange {
            start: 0,
            end: file_length - 1,
            partial: false,
        });
    };
    let value = header.to_str().map_err(|_| "invalid Range header")?;
    let value = value
        .strip_prefix("bytes=")
        .ok_or("only byte ranges are supported")?;
    if value.contains(',') {
        return Err("multiple ranges are not supported");
    }
    let (start, end) = value.split_once('-').ok_or("malformed byte range")?;
    let (start, end) = if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| "invalid suffix range")?;
        if suffix == 0 {
            return Err("suffix range must be positive");
        }
        (file_length.saturating_sub(suffix), file_length - 1)
    } else {
        let start = start.parse::<u64>().map_err(|_| "invalid range start")?;
        let end = if end.is_empty() {
            file_length - 1
        } else {
            end.parse::<u64>().map_err(|_| "invalid range end")?
        };
        (start, end.min(file_length - 1))
    };
    if start >= file_length || end < start {
        return Err("range is outside the file");
    }
    Ok(ByteRange {
        start,
        end,
        partial: true,
    })
}

fn plain_error(status: StatusCode, message: &str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message.to_owned()))
        .expect("valid error response")
}

fn hex_encode(bytes: [u8; 32]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(64);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use torrent_engine::{AddOptions, CreateOptions, DhtMode, EngineConfig};

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn serves_authenticated_seekable_byte_ranges() {
        let directory = tempfile::tempdir().unwrap();
        let payload: Vec<u8> = (0..80_000_u32).map(|value| (value % 251) as u8).collect();
        let path = directory.path().join("video.mp4");
        std::fs::write(&path, &payload).unwrap();
        let created = torrent_engine::create_torrent(
            &path,
            CreateOptions {
                name: None,
                piece_length: Some(16 * 1024),
            },
        )
        .await
        .unwrap();
        let mut config = EngineConfig::new(directory.path());
        config.dht_mode = DhtMode::Disabled;
        let engine = Arc::new(TorrentEngine::new(config).await.unwrap());
        let torrent = engine
            .add_metainfo(
                created.metainfo_bytes(),
                AddOptions {
                    overwrite: true,
                    ..AddOptions::default()
                },
            )
            .await
            .unwrap();
        torrent.wait_until_completed().await.unwrap();
        let gateway = StreamGateway::start(engine.clone()).await.unwrap();
        let url = gateway.url(torrent.id(), 0);
        let client = reqwest::Client::new();

        let response = client
            .get(&url)
            .header(RANGE, "bytes=100-199")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers()[CONTENT_RANGE],
            format!("bytes 100-199/{}", payload.len())
        );
        assert_eq!(response.bytes().await.unwrap().as_ref(), &payload[100..200]);

        let unauthorized = url.replacen("/stream/", "/stream/wrong-", 1);
        assert_eq!(
            client.get(unauthorized).send().await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(&url)
                .header(RANGE, "bytes=999999-")
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::RANGE_NOT_SATISFIABLE
        );

        gateway.shutdown().unwrap();
        engine.shutdown().await;
    }

    #[test]
    fn parses_standard_range_forms() {
        assert_eq!(parse_range(None, 100).unwrap().end, 99);
        let suffix = HeaderValue::from_static("bytes=-10");
        let range = parse_range(Some(&suffix), 100).unwrap();
        assert_eq!((range.start, range.end), (90, 99));
        let open = HeaderValue::from_static("bytes=90-");
        let range = parse_range(Some(&open), 100).unwrap();
        assert_eq!((range.start, range.end), (90, 99));
    }
}
