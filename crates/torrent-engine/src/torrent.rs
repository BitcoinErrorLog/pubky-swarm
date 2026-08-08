//! A torrent managed by the engine's session.

use std::collections::HashSet;
use std::sync::Arc;

use librqbit::api::TorrentIdOrHash;
use librqbit::{ManagedTorrent, Session, TorrentStatsState};

/// `librqbit` does not re-export its `ManagedTorrentHandle` alias at the crate
/// root, so spell it out.
type ManagedTorrentHandle = Arc<ManagedTorrent>;

use crate::stream::FileStream;
use crate::types::{FileMeta, FileProgress, TorrentMeta, TorrentProgress, TorrentState};
use crate::{Error, Result};

#[derive(Debug, Default)]
struct LiveProgress {
    download_mbps: f64,
    upload_mbps: f64,
    peers_connected: usize,
    peers_seen: usize,
    ratio: f64,
    eta: Option<u64>,
}

#[allow(clippy::cast_precision_loss)]
fn safe_ratio(uploaded_bytes: u64, fetched_bytes: u64) -> f64 {
    if fetched_bytes == 0 {
        0.0
    } else {
        uploaded_bytes as f64 / fetched_bytes as f64
    }
}

fn eta_seconds(eta: &impl serde::Serialize) -> Option<u64> {
    serde_json::to_value(eta)
        .ok()?
        .get("duration")?
        .get("secs")?
        .as_u64()
}

/// Handle to a torrent managed by a [`crate::TorrentEngine`] session.
///
/// Cheap to clone; clones refer to the same underlying torrent.
#[derive(Clone)]
pub struct Torrent {
    session: Arc<Session>,
    handle: ManagedTorrentHandle,
}

impl Torrent {
    pub(crate) fn new(session: Arc<Session>, handle: ManagedTorrentHandle) -> Self {
        Self { session, handle }
    }

    /// Session-local torrent id.
    #[must_use]
    pub fn id(&self) -> usize {
        self.handle.id()
    }

    /// Hex-encoded v1 info hash.
    #[must_use]
    pub fn info_hash(&self) -> String {
        self.handle.info_hash().as_string()
    }

    /// Torrent name, if known (from metadata or the magnet `dn` parameter).
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.handle.name()
    }

    /// Whether the torrent is currently paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.handle.is_paused()
    }

    /// Wait until the torrent leaves the initializing state (metadata resolved
    /// and on-disk contents checked). Fails if the torrent errors out.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if the torrent enters the error state.
    pub async fn wait_until_initialized(&self) -> Result<()> {
        self.handle.wait_until_initialized().await?;
        Ok(())
    }

    /// Wait until all selected files are fully downloaded and verified.
    ///
    /// Note: this waits indefinitely while the torrent is paused.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if the torrent enters the error state.
    pub async fn wait_until_completed(&self) -> Result<()> {
        self.handle.wait_until_completed().await?;
        Ok(())
    }

    /// Typed metadata snapshot. Fails with [`Error::MetadataUnavailable`] if
    /// the info dictionary has not been resolved yet.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MetadataUnavailable`] until metadata is resolved.
    pub fn metadata(&self) -> Result<TorrentMeta> {
        let metadata = self
            .handle
            .metadata
            .load_full()
            .ok_or(Error::MetadataUnavailable)?;
        let only_files = self.handle.only_files();
        let files = metadata
            .file_infos
            .iter()
            .enumerate()
            .map(|(index, fi)| FileMeta {
                index,
                path: fi.relative_filename.clone(),
                length: fi.len,
                included: only_files.as_ref().is_none_or(|o| o.contains(&index)),
            })
            .collect();
        Ok(TorrentMeta {
            id: self.id(),
            info_hash: self.info_hash(),
            name: metadata.name.clone(),
            piece_length: metadata.info.piece_length,
            total_length: metadata.lengths.total_length(),
            files,
        })
    }

    /// Typed progress snapshot.
    #[must_use]
    pub fn progress(&self) -> TorrentProgress {
        let stats = self.handle.stats();
        let torrent_state = match stats.state {
            TorrentStatsState::Initializing => TorrentState::Initializing,
            TorrentStatsState::Live => TorrentState::Live,
            TorrentStatsState::Paused => TorrentState::Paused,
            TorrentStatsState::Error => TorrentState::Error,
        };
        let live = stats
            .live
            .as_ref()
            .map_or_else(LiveProgress::default, |live| LiveProgress {
                download_mbps: live.download_speed.mbps,
                upload_mbps: live.upload_speed.mbps,
                peers_connected: live.snapshot.peer_stats.live,
                peers_seen: live.snapshot.peer_stats.seen,
                ratio: safe_ratio(live.snapshot.uploaded_bytes, live.snapshot.fetched_bytes),
                eta: live.time_remaining.as_ref().and_then(eta_seconds),
            });
        TorrentProgress {
            state: torrent_state,
            total_bytes: stats.total_bytes,
            progress_bytes: stats.progress_bytes,
            uploaded_bytes: stats.uploaded_bytes,
            download_mbps: live.download_mbps,
            upload_mbps: live.upload_mbps,
            peers_connected: live.peers_connected,
            peers_seen: live.peers_seen,
            ratio: live.ratio,
            eta: live.eta,
            finished: stats.finished,
            error: stats.error,
            files: stats
                .file_progress
                .into_iter()
                .enumerate()
                .map(|(index, have_bytes)| FileProgress { index, have_bytes })
                .collect(),
        }
    }

    /// Open a seekable async stream over a single file of this torrent.
    ///
    /// The torrent must be initialized (metadata resolved) and live or paused.
    /// Reads yield data once the covering pieces are downloaded and verified.
    ///
    /// # Errors
    ///
    /// - [`Error::MetadataUnavailable`] if metadata is not resolved yet.
    /// - [`Error::FileIndexOutOfRange`] for an invalid file index.
    /// - [`Error::Engine`] if the torrent is in a state that cannot stream.
    pub fn stream_file(&self, file_index: usize) -> Result<FileStream> {
        let file_count = self
            .handle
            .metadata
            .load()
            .as_ref()
            .map(|m| m.file_infos.len())
            .ok_or(Error::MetadataUnavailable)?;
        if file_index >= file_count {
            return Err(Error::FileIndexOutOfRange {
                index: file_index,
                file_count,
            });
        }
        let stream = self.handle.clone().stream(file_index)?;
        Ok(FileStream::new(stream))
    }

    /// Pause the torrent. Idempotent: pausing an already paused torrent is a no-op.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if librqbit refuses the transition (e.g. the
    /// torrent is still initializing or is in the error state).
    pub async fn pause(&self) -> Result<()> {
        if self.handle.is_paused() {
            return Ok(());
        }
        self.session.pause(&self.handle).await?;
        Ok(())
    }

    /// Resume a paused torrent. Idempotent.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if librqbit fails to restart the torrent.
    pub async fn resume(&self) -> Result<()> {
        if !self.handle.is_paused() {
            return Ok(());
        }
        self.session.unpause(&self.handle).await?;
        Ok(())
    }

    /// Change which files are selected for download.
    ///
    /// # Errors
    ///
    /// - [`Error::EmptyFileSelection`] if `files` is empty.
    /// - [`Error::MetadataUnavailable`] if metadata is not resolved yet.
    /// - [`Error::FileIndexOutOfRange`] for an invalid file index.
    /// - [`Error::Engine`] if librqbit fails to apply the selection.
    pub async fn update_only_files(&self, files: &[usize]) -> Result<()> {
        if files.is_empty() {
            return Err(Error::EmptyFileSelection);
        }
        let file_count = self
            .handle
            .metadata
            .load()
            .as_ref()
            .map(|m| m.file_infos.len())
            .ok_or(Error::MetadataUnavailable)?;
        for &index in files {
            if index >= file_count {
                return Err(Error::FileIndexOutOfRange { index, file_count });
            }
        }
        let set: HashSet<usize> = files.iter().copied().collect();
        self.session.update_only_files(&self.handle, &set).await?;
        Ok(())
    }

    /// Remove the torrent from the session ("forget"). Optionally deletes the
    /// downloaded files from disk as well.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Engine`] if the removal fails.
    pub async fn forget(self, delete_files: bool) -> Result<()> {
        self.session
            .delete(TorrentIdOrHash::Id(self.id()), delete_files)
            .await?;
        Ok(())
    }
}

impl std::fmt::Debug for Torrent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Torrent")
            .field("id", &self.id())
            .field("info_hash", &self.info_hash())
            .field("name", &self.name())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_is_finite_for_empty_and_non_empty_downloads() {
        assert!(safe_ratio(500, 0).abs() < f64::EPSILON);
        assert!(safe_ratio(0, 500).abs() < f64::EPSILON);
        assert!((safe_ratio(750, 500) - 1.5).abs() < f64::EPSILON);
        assert!(safe_ratio(u64::MAX, 1).is_finite());
    }
}
