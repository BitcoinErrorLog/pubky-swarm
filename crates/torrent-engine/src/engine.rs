//! The torrent engine: a `librqbit` session plus torrent lifecycle operations.

use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::Arc;

use librqbit::api::TorrentIdOrHash;
use librqbit::limits::LimitsConfig;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Magnet, Session, SessionOptions,
    SessionPersistenceConfig,
};

use crate::torrent::Torrent;
use crate::types::{
    AddOptions, CreateOptions, CreatedTorrent, DhtMode, EngineConfig, MAX_PIECE_LENGTH,
    MIN_PIECE_LENGTH,
};
use crate::validate::{file_count, validate_metainfo};
use crate::{Error, Result};

fn nonzero_bps(bps: Option<u32>) -> Option<NonZeroU32> {
    bps.and_then(NonZeroU32::new)
}

/// Validate an explicit piece length for torrent creation.
fn validate_piece_length(piece_length: u32) -> Result<()> {
    if !piece_length.is_power_of_two() {
        return Err(Error::InvalidPieceLength {
            value: piece_length,
            reason: "must be a power of two",
        });
    }
    if piece_length < MIN_PIECE_LENGTH {
        return Err(Error::InvalidPieceLength {
            value: piece_length,
            reason: "below the minimum of 16 KiB",
        });
    }
    if piece_length > MAX_PIECE_LENGTH {
        return Err(Error::InvalidPieceLength {
            value: piece_length,
            reason: "above the maximum of 16 MiB",
        });
    }
    Ok(())
}

/// Validate a torrent name override.
fn validate_torrent_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
        return Err(Error::InvalidTorrentName(name.to_owned()));
    }
    Ok(())
}

/// Parse a magnet URI (or raw hexadecimal hash) and return its canonical v1
/// info hash when present.
///
/// # Errors
///
/// Returns [`Error::InvalidMagnet`] when the input is not a valid magnet URI.
pub fn magnet_v1_info_hash(magnet: &str) -> Result<Option<String>> {
    let parsed = Magnet::parse(magnet).map_err(|e| Error::InvalidMagnet(format!("{e:#}")))?;
    Ok(parsed.as_id20().map(|info_hash| info_hash.as_string()))
}

/// Create a v1 torrent from a file or directory.
///
/// This does not require a running engine. To seed the result, add
/// [`CreatedTorrent::metainfo_bytes`] via [`TorrentEngine::add_metainfo`] with
/// [`AddOptions::overwrite`] set and the content already in place under the
/// torrent's output folder.
///
/// # Errors
///
/// - [`Error::SourceNotFound`] if `source` does not exist.
/// - [`Error::InvalidPieceLength`] if an explicit piece length fails validation.
/// - [`Error::InvalidTorrentName`] if the name override is unusable.
/// - [`Error::EmptyContent`] if the source contains no data (empty file or
///   directory).
/// - [`Error::InvalidPathComponent`]/[`Error::DuplicateFilePath`] if the
///   source paths are not portable/safe.
/// - [`Error::Engine`]/[`Error::Io`] for filesystem or encoding failures.
pub async fn create_torrent(source: &Path, options: CreateOptions) -> Result<CreatedTorrent> {
    if !source.try_exists()? {
        return Err(Error::SourceNotFound(source.to_owned()));
    }
    if let Some(piece_length) = options.piece_length {
        validate_piece_length(piece_length)?;
    }
    if let Some(name) = &options.name {
        validate_torrent_name(name)?;
    }

    let created = librqbit::create_torrent(
        source,
        librqbit::CreateTorrentOptions {
            name: options.name.as_deref(),
            piece_length: options.piece_length,
        },
    )
    .await?;

    // Run the same structural checks we apply to untrusted metainfo, and
    // refuse to produce contentless torrents (empty file/directory sources).
    validate_metainfo(&created.as_info().info, None)?;

    let info = &created.as_info().info;
    let total_length: u64 = info.iter_file_lengths()?.sum();
    let file_count = info.iter_file_lengths()?.count();
    let name = info
        .name
        .as_ref()
        .map(|n| String::from_utf8_lossy(n.as_ref()).into_owned());

    Ok(CreatedTorrent::new(
        created.info_hash().as_string(),
        name,
        info.piece_length,
        total_length,
        file_count,
        created.as_bytes()?.to_vec(),
    ))
}

/// The torrent engine. Owns a librqbit session: a TCP peer listener, an
/// optional DHT node, optional JSON session persistence, and all managed
/// torrents.
pub struct TorrentEngine {
    session: Arc<Session>,
    config: EngineConfig,
}

impl TorrentEngine {
    /// Start a new engine session.
    ///
    /// Creates the download directory (and persistence directory, if
    /// configured) when missing. When persistence is enabled, torrents from
    /// the previous run are restored into this session.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidDownloadDir`] if the download dir is not a directory.
    /// - [`Error::InvalidConfig`] for inconsistent configuration.
    /// - [`Error::Io`] if directories cannot be created.
    /// - [`Error::Engine`] if the librqbit session fails to start (e.g. no
    ///   free listen ports).
    pub async fn new(config: EngineConfig) -> Result<Self> {
        if config.download_dir.exists() && !config.download_dir.is_dir() {
            return Err(Error::InvalidDownloadDir(config.download_dir.clone()));
        }
        std::fs::create_dir_all(&config.download_dir)?;

        if let Some(range) = &config.listen_port_range
            && range.is_empty()
        {
            return Err(Error::InvalidConfig(
                "listen_port_range must not be empty (use 0..1 for an ephemeral port)",
            ));
        }
        let limits = &config.metainfo_limits;
        if limits.max_metainfo_bytes == 0
            || limits.max_files == 0
            || limits.max_total_bytes == 0
            || limits.max_path_components == 0
            || limits.max_component_bytes == 0
            || limits.max_path_bytes == 0
        {
            return Err(Error::InvalidConfig("metainfo limits must all be non-zero"));
        }
        if config.fastresume && config.persistence_dir.is_none() {
            return Err(Error::InvalidConfig(
                "fastresume requires persistence_dir to be set",
            ));
        }
        if let Some(dir) = &config.persistence_dir {
            if dir.exists() && !dir.is_dir() {
                return Err(Error::InvalidConfig(
                    "persistence_dir exists but is not a directory",
                ));
            }
            std::fs::create_dir_all(dir)?;
        }

        let (disable_dht, disable_dht_persistence) = match config.dht_mode {
            DhtMode::Disabled => (true, true),
            DhtMode::Ephemeral => (false, true),
            DhtMode::Persistent => (false, false),
        };
        let options = SessionOptions {
            disable_dht,
            disable_dht_persistence,
            fastresume: config.fastresume,
            persistence: config.persistence_dir.clone().map(|folder| {
                SessionPersistenceConfig::Json {
                    folder: Some(folder),
                }
            }),
            listen_port_range: config.listen_port_range.clone(),
            enable_upnp_port_forwarding: config.enable_upnp_port_forwarding,
            ratelimits: LimitsConfig {
                upload_bps: nonzero_bps(config.upload_bps),
                download_bps: nonzero_bps(config.download_bps),
            },
            ..Default::default()
        };
        let session = Session::new_with_opts(config.download_dir.clone(), options).await?;
        Ok(Self { session, config })
    }

    /// The engine configuration this session was started with.
    #[must_use]
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// The session's default download directory.
    #[must_use]
    pub fn download_dir(&self) -> &Path {
        &self.config.download_dir
    }

    /// The TCP port the peer listener is bound to, if any.
    #[must_use]
    pub fn listen_port(&self) -> Option<u16> {
        self.session.tcp_listen_port()
    }

    /// Update the session upload throttle. `None` or `0` means unlimited.
    pub fn set_upload_bps(&self, bps: Option<u32>) {
        self.session
            .ratelimits
            .set_upload_bps(nonzero_bps(bps));
    }

    /// Update the session download throttle. `None` or `0` means unlimited.
    pub fn set_download_bps(&self, bps: Option<u32>) {
        self.session
            .ratelimits
            .set_download_bps(nonzero_bps(bps));
    }

    /// Add a torrent from a magnet link (or a raw 40-char hex info hash).
    ///
    /// This is a two-stage flow so that magnet-resolved content is never
    /// accepted or written before our validator runs:
    ///
    /// 1. The magnet is resolved through librqbit's `list_only` add path,
    ///    which fetches the metainfo from peers without creating storage or a
    ///    managed torrent.
    /// 2. The resolved metainfo is size-checked against
    ///    [`EngineConfig::metainfo_limits`], structurally validated, and only
    ///    then re-added from the returned `torrent_bytes` (which carry the
    ///    magnet's trackers) with the caller's options. Peers seen during
    ///    resolution are merged into the caller's `initial_peers`.
    ///
    /// Unavoidable caveat (librqbit 8.1.1): during stage 1, librqbit's BEP 9
    /// metadata reader allocates a buffer of the peer-advertised
    /// `metadata_size` *before* our limit can run. librqbit enforces its own
    /// hard cap of 32 MiB (`peer_info_reader/mod.rs`), and no public option
    /// lowers it; our `max_metainfo_bytes` applies immediately after
    /// resolution, before anything is added or written to disk.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidMagnet`] if the link does not parse or has no v1 hash.
    /// - [`Error::LimitExceeded`] / [`Error::InvalidPathComponent`] /
    ///   [`Error::DuplicateFilePath`] / [`Error::PrefixPathCollision`] /
    ///   [`Error::EmptyContent`] if the resolved metainfo fails validation.
    /// - [`Error::FileIndexOutOfRange`] for a bad [`AddOptions::only_files`]
    ///   index (checked against the resolved metadata).
    /// - [`Error::EmptyFileSelection`] / [`Error::InvalidOutputDir`] for bad
    ///   [`AddOptions`].
    /// - [`Error::Engine`] if resolution or the session rejects the add.
    pub async fn add_magnet(&self, magnet: &str, options: AddOptions) -> Result<Torrent> {
        if magnet_v1_info_hash(magnet)?.is_none() {
            return Err(Error::InvalidMagnet(
                "magnet link does not contain a v1 (btih) info hash".to_owned(),
            ));
        }
        if let Some(only_files) = &options.only_files
            && only_files.is_empty()
        {
            return Err(Error::EmptyFileSelection);
        }

        // Stage 1: resolve metadata without storage or a managed torrent.
        let stage1 = AddTorrentOptions {
            list_only: true,
            initial_peers: options.initial_peers.clone(),
            disable_trackers: options.disable_trackers,
            ..Default::default()
        };
        let resolved = self
            .session
            .add_torrent(AddTorrent::from_url(magnet), Some(stage1))
            .await?;
        let AddTorrentResponse::ListOnly(list) = resolved else {
            return Err(Error::Engine(anyhow::anyhow!(
                "list_only add did not return a ListOnly response"
            )));
        };

        // librqbit returns ListOnly before inserting anything into the
        // session; verify defensively that resolution left nothing managed.
        if self
            .session
            .get(TorrentIdOrHash::Hash(list.info_hash))
            .is_some()
        {
            return Err(Error::Engine(anyhow::anyhow!(
                "list_only magnet resolution left a managed torrent behind"
            )));
        }

        // Stage 2 gate: enforce our limits and structural rules on the
        // resolved metainfo before it is added.
        let limits = &self.config.metainfo_limits;
        if list.torrent_bytes.len() > limits.max_metainfo_bytes {
            return Err(Error::LimitExceeded {
                limit: "metainfo bytes",
                value: list.torrent_bytes.len() as u64,
                max: limits.max_metainfo_bytes as u64,
            });
        }
        validate_metainfo(&list.info, Some(limits))?;

        // Validate the caller's file selection against the resolved metadata.
        if let Some(only_files) = &options.only_files {
            let count = file_count(&list.info);
            for &index in only_files {
                if index >= count {
                    return Err(Error::FileIndexOutOfRange {
                        index,
                        file_count: count,
                    });
                }
            }
        }

        // Merge peers discovered during resolution with the caller's,
        // keeping caller order and dropping duplicates.
        let mut merged: Vec<SocketAddr> = options.initial_peers.clone().unwrap_or_default();
        for peer in list.seen_peers {
            if !merged.contains(&peer) {
                merged.push(peer);
            }
        }

        // Stage 2: add the validated metainfo. The returned torrent_bytes
        // carry the magnet's trackers, so tracker data is preserved.
        let stage2 = AddOptions {
            initial_peers: if merged.is_empty() {
                None
            } else {
                Some(merged)
            },
            ..options
        };
        self.add_metainfo(&list.torrent_bytes, stage2).await
    }

    /// Add a torrent from bencoded v1 metainfo (`.torrent` file contents).
    ///
    /// The metainfo is treated as untrusted: it is parsed and validated
    /// against [`EngineConfig::metainfo_limits`] and path-safety rules before
    /// it reaches the session.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidMetainfo`] if the bytes are not a valid v1 torrent.
    /// - [`Error::LimitExceeded`] if a configured defensive limit is exceeded.
    /// - [`Error::InvalidPathComponent`] / [`Error::DuplicateFilePath`] /
    ///   [`Error::EmptyContent`] for unsafe or useless metainfo.
    /// - [`Error::EmptyFileSelection`] / [`Error::InvalidOutputDir`] for bad
    ///   [`AddOptions`].
    /// - [`Error::Engine`] if the session rejects the add.
    pub async fn add_metainfo(&self, metainfo: &[u8], options: AddOptions) -> Result<Torrent> {
        if metainfo.is_empty() {
            return Err(Error::InvalidMetainfo("empty input".to_owned()));
        }
        let limits = &self.config.metainfo_limits;
        if metainfo.len() > limits.max_metainfo_bytes {
            return Err(Error::LimitExceeded {
                limit: "metainfo bytes",
                value: metainfo.len() as u64,
                max: limits.max_metainfo_bytes as u64,
            });
        }
        let parsed = librqbit::torrent_from_bytes::<librqbit::ByteBuf>(metainfo)
            .map_err(|e| Error::InvalidMetainfo(format!("{e:#}")))?;
        validate_metainfo(&parsed.info, Some(limits))?;
        if let Some(only_files) = &options.only_files {
            let count = file_count(&parsed.info);
            for &index in only_files {
                if index >= count {
                    return Err(Error::FileIndexOutOfRange {
                        index,
                        file_count: count,
                    });
                }
            }
        }
        self.add(AddTorrent::from_bytes(metainfo.to_vec()), options)
            .await
    }

    /// Shared implementation for adding a torrent source to the session.
    async fn add(&self, source: AddTorrent<'_>, options: AddOptions) -> Result<Torrent> {
        if let Some(only_files) = &options.only_files
            && only_files.is_empty()
        {
            return Err(Error::EmptyFileSelection);
        }
        if let Some(dir) = &options.output_dir
            && !dir.is_dir()
        {
            return Err(Error::InvalidOutputDir(dir.clone()));
        }

        let rqbit_options = AddTorrentOptions {
            paused: options.paused,
            only_files: options.only_files,
            overwrite: options.overwrite,
            output_folder: options.output_dir.map(|d| d.to_string_lossy().into_owned()),
            initial_peers: options.initial_peers,
            disable_trackers: options.disable_trackers,
            ..Default::default()
        };

        match self
            .session
            .add_torrent(source, Some(rqbit_options))
            .await?
        {
            AddTorrentResponse::Added(_, handle)
            | AddTorrentResponse::AlreadyManaged(_, handle) => {
                Ok(Torrent::new(self.session.clone(), handle))
            }
            AddTorrentResponse::ListOnly(_) => Err(Error::Engine(anyhow::anyhow!(
                "unexpected list-only response: list_only was not requested"
            ))),
        }
    }

    /// Look up a managed torrent by session id.
    #[must_use]
    pub fn get(&self, id: usize) -> Option<Torrent> {
        self.session
            .get(TorrentIdOrHash::Id(id))
            .map(|handle| Torrent::new(self.session.clone(), handle))
    }

    /// List all torrents currently managed by the session.
    #[must_use]
    pub fn list(&self) -> Vec<Torrent> {
        self.session.with_torrents(|torrents| {
            torrents
                .map(|(_, handle)| Torrent::new(self.session.clone(), handle.clone()))
                .collect()
        })
    }

    /// Stop the session: pauses all torrents and shuts down background tasks
    /// (listener, DHT, persistence flushes).
    pub async fn shutdown(&self) {
        self.session.stop().await;
    }
}

impl std::fmt::Debug for TorrentEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentEngine")
            .field("download_dir", &self.config.download_dir)
            .field("listen_port", &self.listen_port())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MIN_PIECE_LENGTH;

    #[test]
    fn piece_length_validation() {
        assert!(validate_piece_length(MIN_PIECE_LENGTH).is_ok());
        assert!(validate_piece_length(MAX_PIECE_LENGTH).is_ok());
        assert!(validate_piece_length(64 * 1024).is_ok());

        assert!(matches!(
            validate_piece_length(0),
            Err(Error::InvalidPieceLength { value: 0, .. })
        ));
        assert!(matches!(
            validate_piece_length(1000),
            Err(Error::InvalidPieceLength { value: 1000, .. })
        ));
        assert!(matches!(
            validate_piece_length(8192),
            Err(Error::InvalidPieceLength { value: 8192, .. })
        ));
        assert!(matches!(
            validate_piece_length(32 * 1024 * 1024),
            Err(Error::InvalidPieceLength { .. })
        ));
    }

    #[test]
    fn torrent_name_validation() {
        assert!(validate_torrent_name("data").is_ok());
        assert!(validate_torrent_name("my release v1").is_ok());

        for bad in ["", "a/b", "a\\b", ".", ".."] {
            assert!(
                matches!(
                    validate_torrent_name(bad),
                    Err(Error::InvalidTorrentName(_))
                ),
                "expected {bad:?} to be rejected"
            );
        }
    }

    #[test]
    fn parses_v1_info_hash_without_dropping_magnet_parameters() {
        let hash = "ab".repeat(20);
        let magnet =
            format!("magnet:?xt=urn:btih:{hash}&dn=release&tr=https%3A%2F%2Ftracker.example");

        assert_eq!(magnet_v1_info_hash(&magnet).unwrap(), Some(hash));
        assert!(matches!(
            magnet_v1_info_hash("not a magnet"),
            Err(Error::InvalidMagnet(_))
        ));
        let v2_hash = "cd".repeat(32);
        assert_eq!(
            magnet_v1_info_hash(&format!("magnet:?xt=urn:btmh:1220{v2_hash}")).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn create_torrent_from_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("payload.bin");
        let content: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&source, &content).unwrap();

        let created = create_torrent(
            &source,
            CreateOptions {
                name: None,
                piece_length: Some(16 * 1024),
            },
        )
        .await
        .unwrap();

        assert_eq!(created.name(), Some("payload.bin"));
        assert_eq!(created.piece_length(), 16 * 1024);
        assert_eq!(created.total_length(), content.len() as u64);
        assert_eq!(created.file_count(), 1);
        assert_eq!(created.info_hash_hex().len(), 40);

        // The produced metainfo must be parseable as a v1 torrent.
        let parsed =
            librqbit::torrent_from_bytes::<librqbit::ByteBuf>(created.metainfo_bytes()).unwrap();
        assert_eq!(parsed.info_hash.as_string(), created.info_hash_hex());

        // Creation is deterministic for identical inputs.
        let again = create_torrent(
            &source,
            CreateOptions {
                name: None,
                piece_length: Some(16 * 1024),
            },
        )
        .await
        .unwrap();
        assert_eq!(created.info_hash_hex(), again.info_hash_hex());
    }

    #[tokio::test]
    async fn create_torrent_from_directory_with_name_override() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("release");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.bin"), vec![7u8; 40_000]).unwrap();
        std::fs::write(dir.join("sub").join("b.bin"), vec![9u8; 20_000]).unwrap();

        let created = create_torrent(
            &dir,
            CreateOptions {
                name: Some("my-release".to_owned()),
                piece_length: Some(32 * 1024),
            },
        )
        .await
        .unwrap();

        assert_eq!(created.name(), Some("my-release"));
        assert_eq!(created.piece_length(), 32 * 1024);
        assert_eq!(created.total_length(), 60_000);
        assert_eq!(created.file_count(), 2);
        assert!(created.magnet().contains("xt=urn:btih:"));
        assert!(created.magnet().contains("dn=my-release"));
    }

    #[tokio::test]
    async fn create_torrent_rejects_bad_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("f.bin");
        std::fs::write(&source, b"x").unwrap();

        // Missing source path.
        let missing = tmp.path().join("nope.bin");
        assert!(matches!(
            create_torrent(&missing, CreateOptions::default()).await,
            Err(Error::SourceNotFound(_))
        ));

        // Invalid piece length.
        assert!(matches!(
            create_torrent(
                &source,
                CreateOptions {
                    name: None,
                    piece_length: Some(1000),
                },
            )
            .await,
            Err(Error::InvalidPieceLength { value: 1000, .. })
        ));

        // Invalid name.
        assert!(matches!(
            create_torrent(
                &source,
                CreateOptions {
                    name: Some("a/b".to_owned()),
                    piece_length: None,
                },
            )
            .await,
            Err(Error::InvalidTorrentName(_))
        ));
    }

    #[tokio::test]
    async fn engine_config_validation() {
        let tmp = tempfile::tempdir().unwrap();

        // Download dir that is a regular file.
        let file = tmp.path().join("a-file");
        std::fs::write(&file, b"x").unwrap();
        assert!(matches!(
            TorrentEngine::new(EngineConfig::new(&file)).await,
            Err(Error::InvalidDownloadDir(_))
        ));

        // Empty port range.
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.listen_port_range = Some(10..10);
        assert!(matches!(
            TorrentEngine::new(cfg).await,
            Err(Error::InvalidConfig(_))
        ));

        // fastresume without persistence.
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.fastresume = true;
        assert!(matches!(
            TorrentEngine::new(cfg).await,
            Err(Error::InvalidConfig(_))
        ));

        // Zeroed metainfo limits.
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.metainfo_limits.max_files = 0;
        assert!(matches!(
            TorrentEngine::new(cfg).await,
            Err(Error::InvalidConfig(_))
        ));
    }

    #[tokio::test]
    async fn rate_limit_setters_accept_none_and_nonzero() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        cfg.listen_port_range = Some({
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port..(port + 1)
        });
        cfg.upload_bps = Some(1024);
        cfg.download_bps = Some(2048);
        let engine = TorrentEngine::new(cfg).await.unwrap();
        engine.set_upload_bps(None);
        engine.set_upload_bps(Some(0));
        engine.set_upload_bps(Some(4096));
        engine.set_download_bps(Some(8192));
        engine.shutdown().await;
    }

    #[tokio::test]
    async fn create_torrent_rejects_empty_content() {
        let tmp = tempfile::tempdir().unwrap();

        // Empty file.
        let empty_file = tmp.path().join("empty.bin");
        std::fs::write(&empty_file, b"").unwrap();
        assert!(matches!(
            create_torrent(&empty_file, CreateOptions::default()).await,
            Err(Error::EmptyContent)
        ));

        // Empty directory.
        let empty_dir = tmp.path().join("empty-dir");
        std::fs::create_dir(&empty_dir).unwrap();
        assert!(matches!(
            create_torrent(&empty_dir, CreateOptions::default()).await,
            Err(Error::EmptyContent)
        ));
    }

    #[tokio::test]
    async fn add_metainfo_enforces_limits_and_path_safety() {
        use crate::validate::testutil::*;

        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        cfg.metainfo_limits.max_metainfo_bytes = 32;
        let engine = TorrentEngine::new(cfg).await.unwrap();

        // A valid torrent that is simply too large for the configured limit
        // (any well-formed metainfo is bigger than 32 bytes).
        let source = tmp.path().join("big.bin");
        std::fs::write(&source, vec![3u8; 100_000]).unwrap();
        let created = create_torrent(&source, CreateOptions::default())
            .await
            .unwrap();
        assert!(
            created.metainfo_bytes().len() > 32,
            "test requires the metainfo to exceed the configured limit"
        );
        assert!(matches!(
            engine
                .add_metainfo(created.metainfo_bytes(), AddOptions::default())
                .await,
            Err(Error::LimitExceeded {
                limit: "metainfo bytes",
                ..
            })
        ));

        engine.shutdown().await;

        // A small metainfo payload with a path-traversal attempt never
        // reaches the session.
        let mut cfg = EngineConfig::new(tmp.path().join("dl2"));
        cfg.dht_mode = DhtMode::Disabled;
        let engine = TorrentEngine::new(cfg).await.unwrap();
        let evil = torrent_bytes(multifile_info(
            b"release",
            vec![(vec![b"..", b"escape.bin"], 10)],
        ));
        assert!(matches!(
            engine.add_metainfo(&evil, AddOptions::default()).await,
            Err(Error::InvalidPathComponent { .. })
        ));
        assert!(engine.list().is_empty());

        engine.shutdown().await;
    }

    #[tokio::test]
    async fn add_metainfo_rejects_out_of_range_file_selection() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        let engine = TorrentEngine::new(cfg).await.unwrap();

        let source = tmp.path().join("f.bin");
        std::fs::write(&source, vec![1u8; 20_000]).unwrap();
        let created = create_torrent(&source, CreateOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            engine
                .add_metainfo(
                    created.metainfo_bytes(),
                    AddOptions {
                        only_files: Some(vec![7]),
                        ..Default::default()
                    },
                )
                .await,
            Err(Error::FileIndexOutOfRange {
                index: 7,
                file_count: 1
            })
        ));
        assert!(engine.list().is_empty());

        engine.shutdown().await;
    }

    /// Decode a 40-char hex string into 20 bytes (test helper, no extra deps).
    fn hex_decode_20(hex: &str) -> [u8; 20] {
        fn nibble(b: u8) -> u8 {
            match b {
                b'0'..=b'9' => b - b'0',
                b'a'..=b'f' => b - b'a' + 10,
                _ => panic!("invalid hex digit"),
            }
        }
        let bytes = hex.as_bytes();
        assert_eq!(bytes.len(), 40);
        let mut out = [0u8; 20];
        for (i, pair) in bytes.chunks_exact(2).enumerate() {
            out[i] = (nibble(pair[0]) << 4) | nibble(pair[1]);
        }
        out
    }

    /// Minimal BEP 9 (`ut_metadata`) server speaking the real peer wire
    /// protocol over TCP: it answers metadata requests with attacker-crafted
    /// bytes. Used to prove our validator gates magnet-resolved content.
    async fn serve_crafted_metadata(
        listener: tokio::net::TcpListener,
        info_dict: Vec<u8>,
        info_hash: [u8; 20],
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        async fn send_frame(
            socket: &mut tokio::net::TcpStream,
            id: u8,
            payload: &[u8],
        ) -> std::io::Result<()> {
            let len = u32::try_from(payload.len() + 1).unwrap().to_be_bytes();
            socket.write_all(&len).await?;
            socket.write_all(&[id]).await?;
            socket.write_all(payload).await
        }

        // Find an integer value behind a bencoded `"key"i` pattern.
        fn find_int(payload: &[u8], key: &[u8]) -> Option<usize> {
            let pos = payload.windows(key.len()).position(|w| w == key)?;
            let start = pos + key.len();
            let end = start + payload[start..].iter().position(|&b| b == b'e')?;
            std::str::from_utf8(&payload[start..end]).ok()?.parse().ok()
        }

        // Extract the piece number from a ut_metadata request dict.
        fn parse_metadata_request(payload: &[u8]) -> Option<usize> {
            match find_int(payload, b"8:msg_typei")? {
                0 => find_int(payload, b"5:piecei"),
                _ => None, // not a request
            }
        }

        let (mut socket, _) = listener.accept().await.unwrap();

        // Peer handshake in, then ours out (with the BEP 10 extension bit).
        let mut handshake = [0u8; 68];
        socket.read_exact(&mut handshake).await.unwrap();
        assert_eq!(&handshake[1..20], b"BitTorrent protocol");
        let mut response = [0u8; 68];
        response[0] = 19;
        response[1..20].copy_from_slice(b"BitTorrent protocol");
        response[25] = 0x10; // reserved[5]: supports the extension protocol
        response[28..48].copy_from_slice(&info_hash);
        response[48..68].copy_from_slice(b"-TE0001-craftedmeta0");
        socket.write_all(&response).await.unwrap();

        // Extended handshake advertising ut_metadata as extension id 1.
        let extended = format!(
            "d1:md11:ut_metadatai1ee13:metadata_sizei{}ee",
            info_dict.len()
        );
        let mut payload = vec![0u8]; // extended handshake id
        payload.extend_from_slice(extended.as_bytes());
        send_frame(&mut socket, 20, &payload).await.unwrap();

        let mut len_buf = [0u8; 4];
        // BEP 10 extension ids are local to each peer: our replies must use
        // the id the *peer* advertised for ut_metadata, learned from its
        // extended handshake.
        let mut peer_ut_metadata_id: Option<u8> = None;
        loop {
            if socket.read_exact(&mut len_buf).await.is_err() {
                return; // peer disconnected
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            if len == 0 {
                continue; // keepalive
            }
            let mut message = vec![0u8; len];
            if socket.read_exact(&mut message).await.is_err() {
                return;
            }
            if message[0] != 20 {
                continue; // not an extended message
            }
            if message[1] == 0 {
                // Peer's extended handshake.
                if let Some(id) = find_int(&message[2..], b"11:ut_metadatai") {
                    peer_ut_metadata_id = u8::try_from(id).ok();
                }
                continue;
            }
            let Some(piece) = parse_metadata_request(&message[2..]) else {
                continue;
            };
            let Some(ext_id) = peer_ut_metadata_id else {
                continue; // cannot reply before the peer's handshake
            };
            let start = piece * 16_384;
            let end = (start + 16_384).min(info_dict.len());
            let header = format!(
                "d8:msg_typei1e5:piecei{piece}e10:total_sizei{}ee",
                info_dict.len()
            );
            let mut body = vec![ext_id];
            body.extend_from_slice(header.as_bytes());
            body.extend_from_slice(&info_dict[start..end]);
            if send_frame(&mut socket, 20, &body).await.is_err() {
                return;
            }
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn magnet_resolve_rejects_invalid_metainfo_from_peer() {
        use crate::validate::testutil::*;

        let tmp = tempfile::tempdir().unwrap();

        // Craft metainfo with a Windows-reserved device name as a file path
        // component. librqbit 8.1.1 accepts this (it only rejects literal
        // ".." traversal during resolution), so our validator is the only
        // gate for this class of unsafe resolved content.
        let info_dict = multifile_info(b"release", vec![(vec![b"dir", b"NUL.txt"], 10)]);
        let full_torrent = torrent_bytes(info_dict.clone());
        let info_hash_hex = librqbit::torrent_from_bytes::<librqbit::ByteBuf>(&full_torrent)
            .unwrap()
            .info_hash
            .as_string();
        let info_hash = hex_decode_20(&info_hash_hex);

        // Serve the crafted metadata over real BEP 9 on loopback.
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let server_addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_crafted_metadata(listener, info_dict, info_hash));

        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        let engine = TorrentEngine::new(cfg).await.unwrap();

        let magnet = format!("magnet:?xt=urn:btih:{info_hash_hex}");
        let result = engine
            .add_magnet(
                &magnet,
                AddOptions {
                    initial_peers: Some(vec![server_addr]),
                    disable_trackers: true,
                    ..Default::default()
                },
            )
            .await;

        assert!(
            matches!(
                result,
                Err(Error::InvalidPathComponent {
                    reason: "component is a reserved Windows device name",
                    ..
                })
            ),
            "expected InvalidPathComponent (reserved device name), got {result:?}"
        );
        assert!(
            engine.list().is_empty(),
            "rejected metainfo must not be managed"
        );
        assert_eq!(
            std::fs::read_dir(tmp.path().join("dl")).unwrap().count(),
            0,
            "rejected metainfo must not write anything to disk"
        );

        engine.shutdown().await;
        server.abort();
    }

    #[tokio::test]
    async fn add_metainfo_rejects_garbage() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        let engine = TorrentEngine::new(cfg).await.unwrap();

        assert!(matches!(
            engine.add_metainfo(b"", AddOptions::default()).await,
            Err(Error::InvalidMetainfo(_))
        ));
        assert!(matches!(
            engine
                .add_metainfo(b"this is not bencode", AddOptions::default())
                .await,
            Err(Error::InvalidMetainfo(_))
        ));

        // Empty file selection is rejected before touching the session.
        let source = tmp.path().join("f.bin");
        std::fs::write(&source, vec![1u8; 20_000]).unwrap();
        let created = create_torrent(&source, CreateOptions::default())
            .await
            .unwrap();
        assert!(matches!(
            engine
                .add_metainfo(
                    created.metainfo_bytes(),
                    AddOptions {
                        only_files: Some(vec![]),
                        ..Default::default()
                    },
                )
                .await,
            Err(Error::EmptyFileSelection)
        ));

        // Nonexistent output dir override.
        assert!(matches!(
            engine
                .add_metainfo(
                    created.metainfo_bytes(),
                    AddOptions {
                        output_dir: Some(tmp.path().join("missing")),
                        ..Default::default()
                    },
                )
                .await,
            Err(Error::InvalidOutputDir(_))
        ));

        engine.shutdown().await;
    }

    #[tokio::test]
    async fn add_magnet_rejects_invalid_input() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = EngineConfig::new(tmp.path().join("dl"));
        cfg.dht_mode = DhtMode::Disabled;
        let engine = TorrentEngine::new(cfg).await.unwrap();

        assert!(matches!(
            engine
                .add_magnet("not a magnet", AddOptions::default())
                .await,
            Err(Error::InvalidMagnet(_))
        ));
        // Valid magnet syntax but no btih hash.
        assert!(matches!(
            engine
                .add_magnet("magnet:?dn=name-only", AddOptions::default())
                .await,
            Err(Error::InvalidMagnet(_))
        ));

        engine.shutdown().await;
    }
}
