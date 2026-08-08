//! # torrent-engine
//!
//! Phase 0 torrent adapter for pubky-swarm, built on the pinned
//! [`librqbit`] 8.1.1 dependency.
//!
//! Capabilities:
//!
//! - Configurable session via [`EngineConfig`]: download directory, listen
//!   port range, DHT toggle, `UPnP` toggle, and JSON session persistence with
//!   optional fastresume.
//! - V1 torrent creation from a file or directory with explicit piece-length
//!   validation ([`create_torrent`]).
//! - Adding torrents by magnet link ([`TorrentEngine::add_magnet`]) or
//!   bencoded metainfo ([`TorrentEngine::add_metainfo`]), for downloading or
//!   seeding (with [`AddOptions::overwrite`]). Untrusted metainfo is validated
//!   against configurable defensive limits ([`MetainfoLimits`]) and
//!   path-safety rules before it reaches the session.
//! - Selective file download via [`AddOptions::only_files`] and
//!   [`Torrent::update_only_files`].
//! - Typed metadata and progress snapshots ([`TorrentMeta`],
//!   [`TorrentProgress`]).
//! - Seekable async streaming of a single file ([`Torrent::stream_file`],
//!   [`FileStream`]).
//! - Clean pause/resume ([`Torrent::pause`], [`Torrent::resume`]) and removal
//!   ([`Torrent::forget`]).
//!
//! All fallible operations return the typed [`Error`].

#![forbid(unsafe_code)]

mod engine;
mod error;
mod stream;
mod torrent;
mod types;
mod validate;

pub use engine::{TorrentEngine, create_torrent, magnet_v1_info_hash};
pub use error::{Error, Result};
pub use stream::{AsyncFileStream, FileStream};
pub use torrent::Torrent;
pub use types::{
    AddOptions, CreateOptions, CreatedTorrent, DhtMode, EngineConfig, FileMeta, FileProgress,
    MAX_PIECE_LENGTH, MIN_PIECE_LENGTH, MetainfoLimits, TorrentMeta, TorrentProgress, TorrentState,
};
