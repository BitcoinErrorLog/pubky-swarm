//! Configuration and typed metadata exposed by the torrent engine.

use std::net::SocketAddr;
use std::ops::Range;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Smallest piece length accepted when creating torrents (one librqbit chunk).
pub const MIN_PIECE_LENGTH: u32 = 16 * 1024;

/// Largest piece length accepted when creating torrents.
pub const MAX_PIECE_LENGTH: u32 = 16 * 1024 * 1024;

/// Defensive limits applied to untrusted torrent metainfo before it reaches
/// the session. All limits must be non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MetainfoLimits {
    /// Maximum size of the bencoded `.torrent` payload in bytes.
    pub max_metainfo_bytes: usize,
    /// Maximum number of files declared by one torrent.
    pub max_files: usize,
    /// Maximum total declared content bytes.
    pub max_total_bytes: u64,
    /// Maximum number of path components in one file's relative path.
    pub max_path_components: usize,
    /// Maximum byte length of a single path component (matches common
    /// filesystem `NAME_MAX`).
    pub max_component_bytes: usize,
    /// Maximum byte length of a file's whole relative path, separators
    /// included (matches common `PATH_MAX`).
    pub max_path_bytes: usize,
}

impl Default for MetainfoLimits {
    fn default() -> Self {
        Self {
            max_metainfo_bytes: 4 * 1024 * 1024,
            max_files: 65_536,
            max_total_bytes: 1 << 40, // 1 TiB
            max_path_components: 64,
            max_component_bytes: 255,
            max_path_bytes: 4096,
        }
    }
}

/// Mainline DHT lifecycle policy for an engine session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhtMode {
    /// Do not create a DHT node.
    Disabled,
    /// Use the DHT without reading or writing shared routing-state persistence.
    Ephemeral,
    /// Use the DHT and persist routing state between sessions.
    Persistent,
}

/// Configuration for [`crate::TorrentEngine`].
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Default directory torrents are downloaded into. Created if missing.
    pub download_dir: PathBuf,
    /// If set, session state (torrent list, resume data) is persisted as JSON
    /// in this directory and restored on the next engine start.
    pub persistence_dir: Option<PathBuf>,
    /// TCP port range for the peer listener. `None` disables the listener.
    ///
    /// Note (librqbit 8.1.1): a range starting at 0 binds an OS-assigned
    /// ephemeral port, but the session reports the *requested* port (0), not
    /// the bound one, so the actual port is undiscoverable in that case.
    /// Prefer offering a range around a pre-allocated free port when the
    /// listen address must be known afterwards.
    pub listen_port_range: Option<Range<u16>>,
    /// Defensive limits for untrusted metainfo added via
    /// [`crate::TorrentEngine::add_metainfo`].
    pub metainfo_limits: MetainfoLimits,
    /// Mainline DHT lifecycle policy.
    pub dht_mode: DhtMode,
    /// Ask `librqbit` to forward the listen port via `UPnP`.
    pub enable_upnp_port_forwarding: bool,
    /// Enable fastresume. Requires [`EngineConfig::persistence_dir`].
    pub fastresume: bool,
}

impl EngineConfig {
    /// Create a config with defaults: no persistence, DHT enabled, no `UPnP`,
    /// default listen ports.
    #[must_use]
    pub fn new(download_dir: impl Into<PathBuf>) -> Self {
        Self {
            download_dir: download_dir.into(),
            persistence_dir: None,
            listen_port_range: None,
            metainfo_limits: MetainfoLimits::default(),
            dht_mode: DhtMode::Persistent,
            enable_upnp_port_forwarding: false,
            fastresume: false,
        }
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self::new("downloads")
    }
}

/// Options for creating a v1 torrent from a file or directory.
#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    /// Torrent name. Defaults to the file/directory basename.
    pub name: Option<String>,
    /// Piece length in bytes. Must be a power of two within
    /// [`MIN_PIECE_LENGTH`]..=[`MAX_PIECE_LENGTH`]. `None` lets librqbit choose.
    pub piece_length: Option<u32>,
}

/// A newly created v1 torrent (not yet added to any session).
#[derive(Debug, Clone)]
pub struct CreatedTorrent {
    info_hash_hex: String,
    name: Option<String>,
    piece_length: u32,
    total_length: u64,
    file_count: usize,
    metainfo: Vec<u8>,
}

impl CreatedTorrent {
    pub(crate) fn new(
        info_hash_hex: String,
        name: Option<String>,
        piece_length: u32,
        total_length: u64,
        file_count: usize,
        metainfo: Vec<u8>,
    ) -> Self {
        Self {
            info_hash_hex,
            name,
            piece_length,
            total_length,
            file_count,
            metainfo,
        }
    }

    /// Hex-encoded v1 info hash.
    #[must_use]
    pub fn info_hash_hex(&self) -> &str {
        &self.info_hash_hex
    }

    /// A trackerless magnet URI for this torrent.
    ///
    /// The `dn` display name is percent-encoded with a real query encoder
    /// ([`url::form_urlencoded::byte_serialize`]), so names containing
    /// spaces, `&`, `=`, `?`, slashes, or non-ASCII text cannot corrupt the
    /// query structure. The `xt` value is a plain hex URN and needs no
    /// encoding.
    #[must_use]
    pub fn magnet(&self) -> String {
        let mut magnet = format!("magnet:?xt=urn:btih:{}", self.info_hash_hex);
        if let Some(name) = &self.name {
            magnet.push_str("&dn=");
            magnet.extend(url::form_urlencoded::byte_serialize(name.as_bytes()));
        }
        magnet
    }

    /// The torrent name, if one is set in the metainfo.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Piece length in bytes.
    #[must_use]
    pub fn piece_length(&self) -> u32 {
        self.piece_length
    }

    /// Total content length in bytes.
    #[must_use]
    pub fn total_length(&self) -> u64 {
        self.total_length
    }

    /// Number of files in the torrent.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.file_count
    }

    /// Bencoded `.torrent` file contents.
    #[must_use]
    pub fn metainfo_bytes(&self) -> &[u8] {
        &self.metainfo
    }

    /// Write the bencoded metainfo to a `.torrent` file on disk.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Io`] if the file cannot be written.
    pub fn write_metainfo(&self, path: impl AsRef<std::path::Path>) -> crate::Result<()> {
        std::fs::write(path, &self.metainfo)?;
        Ok(())
    }
}

/// Options applied when adding a torrent to the session.
#[derive(Debug, Clone, Default)]
pub struct AddOptions {
    /// Restrict download to these file indices. Indices are validated once
    /// metadata is available (immediately for metainfo adds).
    pub only_files: Option<Vec<usize>>,
    /// Override the session download directory for this torrent.
    pub output_dir: Option<PathBuf>,
    /// Allow writing on top of existing files. Required when seeding content
    /// that is already present in the output directory.
    pub overwrite: bool,
    /// Add the torrent in paused state.
    pub paused: bool,
    /// Peers to connect to immediately (e.g. `127.0.0.1:port` for local swarms).
    pub initial_peers: Option<Vec<SocketAddr>>,
    /// Ignore trackers carried by the magnet/metainfo.
    pub disable_trackers: bool,
}

/// High-level state of a managed torrent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TorrentState {
    /// Metadata/resume check in progress.
    Initializing,
    /// Actively connected to the swarm (this includes completed/seeding).
    Live,
    /// Paused.
    Paused,
    /// Fatal error; see [`TorrentProgress::error`].
    Error,
}

/// Static metadata for one file inside a torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FileMeta {
    /// Index of the file within the torrent.
    pub index: usize,
    /// Path relative to the torrent's output folder.
    pub path: PathBuf,
    /// File length in bytes.
    pub length: u64,
    /// Whether the file is currently selected for download.
    pub included: bool,
}

/// Typed metadata for a torrent.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentMeta {
    /// Session-local torrent id.
    pub id: usize,
    /// Hex-encoded v1 info hash.
    pub info_hash: String,
    /// Torrent name, if known.
    pub name: Option<String>,
    /// Piece length in bytes.
    pub piece_length: u32,
    /// Total content length in bytes.
    pub total_length: u64,
    /// Files in torrent order.
    pub files: Vec<FileMeta>,
}

/// Download progress for one file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FileProgress {
    /// Index of the file within the torrent.
    pub index: usize,
    /// Bytes of this file already verified on disk.
    pub have_bytes: u64,
}

/// Typed progress snapshot for a torrent.
#[derive(Debug, Clone, Serialize)]
pub struct TorrentProgress {
    /// High-level state.
    pub state: TorrentState,
    /// Total content length in bytes (of selected files once known).
    pub total_bytes: u64,
    /// Verified bytes on disk.
    pub progress_bytes: u64,
    /// Bytes uploaded to peers.
    pub uploaded_bytes: u64,
    /// Current download throughput in mebibytes per second.
    pub download_mbps: f64,
    /// Current upload throughput in mebibytes per second.
    pub upload_mbps: f64,
    /// Number of peers with a live connection.
    pub peers_connected: usize,
    /// Number of peers observed by the live torrent.
    pub peers_seen: usize,
    /// Uploaded bytes divided by bytes fetched from peers. Zero until at
    /// least one byte has been fetched, so this value is always finite.
    pub ratio: f64,
    /// Estimated seconds remaining, when librqbit has an estimate.
    pub eta: Option<u64>,
    /// True when every selected file is complete (i.e. seeding).
    pub finished: bool,
    /// Error message when [`TorrentProgress::state`] is [`TorrentState::Error`].
    pub error: Option<String>,
    /// Per-file progress; empty while the torrent is initializing.
    pub files: Vec<FileProgress>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn created_with_name(name: Option<&str>) -> CreatedTorrent {
        CreatedTorrent::new(
            "ab".repeat(20), // valid 40-char hex info hash
            name.map(str::to_owned),
            16 * 1024,
            1234,
            1,
            Vec::new(),
        )
    }

    #[test]
    fn magnet_display_name_cannot_corrupt_query_structure() {
        for name in [
            "plain",
            "with spaces",
            "a&b=c",
            "question?",
            "ünïcödé 名前",
            "100% legit",
            // Slashes are rejected at creation time, but even if one slipped
            // through it must stay inside the dn value.
            "a/b\\c",
        ] {
            let magnet = created_with_name(Some(name)).magnet();
            let url = url::Url::parse(&magnet).expect("magnet must be a valid URL");
            assert_eq!(url.scheme(), "magnet");

            let pairs: Vec<_> = url.query_pairs().collect();
            assert_eq!(pairs.len(), 2, "query must contain exactly xt and dn");
            assert_eq!(pairs[0].0, "xt");
            assert_eq!(pairs[0].1, format!("urn:btih:{}", "ab".repeat(20)));
            assert_eq!(pairs[1].0, "dn");
            assert_eq!(pairs[1].1, name, "dn must round-trip for {name:?}");

            // librqbit's own magnet parser must decode the same name.
            let parsed = librqbit::Magnet::parse(&magnet).unwrap();
            assert_eq!(parsed.name.as_deref(), Some(name));
            assert_eq!(parsed.as_id20().unwrap().as_string(), "ab".repeat(20));
        }
    }

    #[test]
    fn magnet_without_name_has_only_xt() {
        let magnet = created_with_name(None).magnet();
        let url = url::Url::parse(&magnet).unwrap();
        let pairs: Vec<_> = url.query_pairs().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "xt");
    }
}
