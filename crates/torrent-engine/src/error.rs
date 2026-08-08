//! Typed errors returned by the torrent engine.

use std::path::PathBuf;

/// Error type for all fallible operations in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested piece length failed validation.
    #[error("invalid piece length {value}: {reason}")]
    InvalidPieceLength {
        /// The rejected piece length in bytes.
        value: u32,
        /// Why the value was rejected.
        reason: &'static str,
    },

    /// The requested torrent name is not usable.
    #[error("invalid torrent name {0:?}: must be non-empty and contain no path separators")]
    InvalidTorrentName(String),

    /// The source path passed to torrent creation does not exist.
    #[error("source path does not exist: {0}")]
    SourceNotFound(PathBuf),

    /// The configured download directory is unusable.
    #[error("download directory path exists but is not a directory: {0}")]
    InvalidDownloadDir(PathBuf),

    /// A per-torrent output directory override is unusable.
    #[error("output directory does not exist or is not a directory: {0}")]
    InvalidOutputDir(PathBuf),

    /// A piece of engine configuration is inconsistent.
    #[error("invalid engine configuration: {0}")]
    InvalidConfig(&'static str),

    /// The supplied magnet link could not be parsed or lacks a v1 info hash.
    #[error("invalid magnet link: {0}")]
    InvalidMagnet(String),

    /// The supplied metainfo bytes could not be decoded as a v1 torrent.
    #[error("invalid torrent metainfo: {0}")]
    InvalidMetainfo(String),

    /// A configured defensive limit was exceeded by untrusted metainfo.
    #[error("resource limit exceeded: {limit} is {value}, maximum allowed is {max}")]
    LimitExceeded {
        /// Which limit was exceeded.
        limit: &'static str,
        /// The observed value.
        value: u64,
        /// The configured maximum.
        max: u64,
    },

    /// A file path inside torrent metainfo is unsafe or malformed.
    #[error("invalid path component in {path:?}: {reason}")]
    InvalidPathComponent {
        /// Lossy rendering of the offending path.
        path: String,
        /// Why the component was rejected.
        reason: &'static str,
    },

    /// Two files in the torrent resolve to the same relative path.
    #[error("duplicate file path in torrent metainfo: {0}")]
    DuplicateFilePath(String),

    /// One file's path is a strict prefix of another's (file-vs-directory
    /// collision, e.g. `a` and `a/b`).
    #[error("file path {file:?} conflicts with directory path {directory:?}")]
    PrefixPathCollision {
        /// The path that is a parent of another file's path.
        directory: String,
        /// The path nested under `directory`.
        file: String,
    },

    /// The torrent declares zero bytes of content (e.g. created from an empty
    /// file or directory).
    #[error("torrent declares no content")]
    EmptyContent,

    /// The `only_files` selection was present but empty.
    #[error("file selection must contain at least one file index")]
    EmptyFileSelection,

    /// A file index was outside the torrent's file list.
    #[error("file index {index} out of range: torrent has {file_count} file(s)")]
    FileIndexOutOfRange {
        /// The rejected index.
        index: usize,
        /// Number of files in the torrent.
        file_count: usize,
    },

    /// The torrent's metadata has not been resolved yet (e.g. a magnet that has
    /// not fetched the info dictionary from peers).
    #[error("torrent metadata is not available yet")]
    MetadataUnavailable,

    /// Anything reported by the underlying librqbit engine.
    #[error(transparent)]
    Engine(#[from] anyhow::Error),

    /// Filesystem failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Convenience alias for results returned by this crate.
pub type Result<T> = std::result::Result<T, Error>;
