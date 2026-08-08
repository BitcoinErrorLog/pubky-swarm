//! Safe, owned Rust lifecycle facade for the pinned libtorrent 2.0.13 build.
//!
//! Native handles and borrowed libtorrent objects never cross the private CXX
//! boundary. All public observations are owned point-in-time snapshots.

#![forbid(unsafe_code)]

use std::pin::Pin;

use libtorrent_engine_sys::bridge;
use thiserror::Error;

/// Maximum accepted magnet URI length.
pub const MAX_MAGNET_URI_BYTES: usize = 32 * 1024;
/// Maximum accepted bencoded `.torrent` metainfo length.
pub const MAX_TORRENT_BYTES: usize = 8 * 1024 * 1024;
/// Maximum accepted or emitted bencoded fast-resume data length.
pub const MAX_RESUME_DATA_BYTES: usize = 16 * 1024 * 1024;
/// Maximum accepted UTF-8 save-path length.
pub const MAX_SAVE_PATH_BYTES: usize = 4096;
/// Maximum accepted file count or file-priority count.
pub const MAX_FILE_COUNT: usize = 100_000;

/// Errors returned by the libtorrent lifecycle facade.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum BridgeError {
    /// Rust rejected an input before it reached native code.
    #[error("{operation}: {message}")]
    InvalidInput {
        /// Safe API operation that rejected the input.
        operation: &'static str,
        /// Owned validation diagnostic.
        message: String,
    },
    /// The C++ facade caught or received a native error.
    #[error("{operation}: {message}")]
    Native {
        /// Safe API operation that failed.
        operation: &'static str,
        /// Owned diagnostic copied from the native facade.
        message: String,
    },
    /// Native session construction returned no handle and no diagnostic.
    #[error("creating libtorrent session returned a null handle")]
    NullSession,
}

/// Result type for lifecycle operations.
pub type Result<T> = std::result::Result<T, BridgeError>;

/// Runtime version and compile-time feature information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// Runtime version reported by `libtorrent::version()`.
    pub version: String,
    /// Revision embedded in the official version header.
    pub revision: String,
    /// Active libtorrent ABI version.
    pub abi_version: u32,
    /// Stable descriptions of native build switches.
    pub flags: Vec<String>,
}

/// Owned fields parsed from a magnet URI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MagnetInfo {
    /// Lowercase hexadecimal v1 SHA-1 info hash.
    pub v1_hash: Option<String>,
    /// Lowercase hexadecimal v2 SHA-256 info hash.
    pub v2_hash: Option<String>,
    /// Display name from the magnet URI.
    pub name: Option<String>,
    /// Tracker URLs in libtorrent's parsed order.
    pub trackers: Vec<String>,
}

/// Explicit settings for the loopback-only, discovery-free session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionConfig {
    /// Loopback TCP/UDP listen port; zero requests an ephemeral port.
    pub listen_port: u16,
}

/// Facade-owned torrent identifier, stable until removal or session shutdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TorrentId(u64);

impl TorrentId {
    /// Returns the opaque numeric value for persistence in application state.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Explicit flags applied when a torrent is added or restored.
#[allow(clippy::struct_excessive_bools)] // Mirrors independent libtorrent flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddTorrentFlags {
    /// Add in paused state.
    pub paused: bool,
    /// Permit libtorrent queue management.
    pub auto_managed: bool,
    /// Trust local payload as a seed until verification fails.
    pub seed_mode: bool,
    /// Prevent piece requests while still permitting uploads.
    pub upload_mode: bool,
    /// Participate only to improve share ratio.
    pub share_mode: bool,
    /// Prefer pieces in sequential order.
    pub sequential_download: bool,
    /// Pause when the torrent first becomes ready.
    pub stop_when_ready: bool,
    /// Reject a torrent already present in this session.
    pub duplicate_is_error: bool,
    /// Give unspecified files priority zero.
    pub default_dont_download: bool,
}

impl Default for AddTorrentFlags {
    fn default() -> Self {
        Self {
            paused: false,
            auto_managed: false,
            seed_mode: false,
            upload_mode: false,
            share_mode: false,
            sequential_download: false,
            stop_when_ready: false,
            duplicate_is_error: true,
            default_dont_download: false,
        }
    }
}

/// Download and upload limits in bytes per second.
///
/// `None` means unlimited. A configured value must be positive and no greater
/// than `i32::MAX`, matching libtorrent 2.0's native setting width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RateLimits {
    /// Download bytes per second, or unlimited.
    pub download_bytes_per_second: Option<u32>,
    /// Upload bytes per second, or unlimited.
    pub upload_bytes_per_second: Option<u32>,
}

/// Parameters that must be supplied whenever a torrent is added or restored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddTorrentOptions {
    /// UTF-8 directory in which payload files are stored.
    pub save_path: String,
    /// Native lifecycle flags to apply explicitly.
    pub flags: AddTorrentFlags,
    /// Initial per-torrent transfer limits.
    pub limits: RateLimits,
    /// Initial priorities in the inclusive range `0..=7`.
    pub file_priorities: Vec<u8>,
}

impl AddTorrentOptions {
    /// Creates options with an explicit save path and manual, active flags.
    #[must_use]
    pub fn new(save_path: impl Into<String>) -> Self {
        Self {
            save_path: save_path.into(),
            flags: AddTorrentFlags::default(),
            limits: RateLimits::default(),
            file_priorities: Vec::new(),
        }
    }
}

/// Stable mapping of libtorrent's torrent lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TorrentState {
    /// Existing payload files are being hash checked.
    CheckingFiles,
    /// Metadata is being fetched for a magnet.
    DownloadingMetadata,
    /// Selected payload data is being downloaded.
    Downloading,
    /// All selected files are complete.
    Finished,
    /// All files are complete.
    Seeding,
    /// Fast-resume data is being checked.
    CheckingResumeData,
    /// A state introduced by a newer native library.
    Unknown(u8),
}

/// Owned point-in-time file metadata and selection state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSnapshot {
    /// Zero-based file index in metainfo.
    pub index: u32,
    /// Relative path recorded in metainfo.
    pub path: String,
    /// File length in bytes.
    pub size: i64,
    /// Current libtorrent priority in `0..=7`.
    pub priority: u8,
    /// Whether priority is nonzero.
    pub is_selected: bool,
    /// Whether this is an internal v2 alignment file.
    pub is_pad_file: bool,
}

/// Owned point-in-time torrent metadata and status.
#[allow(clippy::struct_excessive_bools)] // Snapshot fields preserve native state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentSnapshot {
    /// Facade-owned stable identifier.
    pub id: TorrentId,
    /// Lowercase v1 SHA-1 info hash.
    pub v1_hash: Option<String>,
    /// Lowercase v2 SHA-256 info hash.
    pub v2_hash: Option<String>,
    /// Current display name.
    pub name: String,
    /// Current payload save path.
    pub save_path: String,
    /// Current lifecycle state.
    pub state: TorrentState,
    /// Current task progress in parts per million.
    pub progress_ppm: u32,
    /// Whether complete metainfo is available.
    pub has_metadata: bool,
    /// Whether this torrent is paused.
    pub is_paused: bool,
    /// Whether libtorrent queue management is enabled.
    pub is_auto_managed: bool,
    /// Whether sequential piece selection is enabled.
    pub is_sequential_download: bool,
    /// Whether optimistic seed mode remains enabled.
    pub is_seed_mode: bool,
    /// Whether payload download requests are disabled.
    pub is_upload_mode: bool,
    /// Whether share mode is enabled.
    pub is_share_mode: bool,
    /// Whether all selected files are complete.
    pub is_finished: bool,
    /// Whether all files are complete.
    pub is_seeding: bool,
    /// Total non-pad payload bytes.
    pub total_bytes: i64,
    /// Total selected payload bytes.
    pub wanted_bytes: i64,
    /// Completed selected payload bytes.
    pub wanted_done_bytes: i64,
    /// Persistent downloaded payload counter.
    pub all_time_download_bytes: i64,
    /// Persistent uploaded payload counter.
    pub all_time_upload_bytes: i64,
    /// Current aggregate download rate.
    pub download_rate: i32,
    /// Current aggregate upload rate.
    pub upload_rate: i32,
    /// Connected peer count.
    pub connected_peers: i32,
    /// Connected seed count.
    pub connected_seeds: i32,
    /// Current per-torrent transfer limits.
    pub limits: RateLimits,
    /// Native torrent error, when present.
    pub error_message: Option<String>,
    /// Current owned file snapshots.
    pub files: Vec<FileSnapshot>,
}

/// Owned copy of one libtorrent alert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertSnapshot {
    /// Numeric native alert type identifier.
    pub type_id: i32,
    /// Stable native alert type name.
    pub type_name: String,
    /// Human-readable owned message.
    pub message: String,
    /// Raw native alert-category mask.
    pub category: u32,
    /// Associated facade torrent ID, when the alert is torrent-scoped.
    pub torrent_id: Option<TorrentId>,
}

/// Owned point-in-time view of the native session and all active torrents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Whether the whole native session is paused.
    pub is_paused: bool,
    /// Whether the loopback listener is active.
    pub is_listening: bool,
    /// Active loopback listen port.
    pub listen_port: u16,
    /// Number of active facade torrents.
    pub torrent_count: u64,
    /// Current session-global transfer limits.
    pub global_limits: RateLimits,
    /// Owned snapshots for every active torrent.
    pub torrents: Vec<TorrentSnapshot>,
    /// Native alerts drained since the previous observation.
    pub alerts: Vec<AlertSnapshot>,
}

/// Token for one in-flight asynchronous fast-resume serialization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResumeDataRequest(u64);

/// Result of explicitly polling a fast-resume request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeDataPoll {
    /// Native disk-thread serialization is still in flight.
    Pending,
    /// Complete owned bencoded fast-resume bytes.
    Ready(Vec<u8>),
}

/// Owning handle for one isolated native libtorrent session.
pub struct Session {
    native: cxx::UniquePtr<bridge::SessionHandle>,
}

/// Returns version and build switches from the linked library.
///
/// # Errors
///
/// Returns a native error if linked build information cannot be copied.
pub fn build_info() -> Result<BuildInfo> {
    let native = bridge::build_info();
    ensure_native_success("reading build information", &native.error)?;
    Ok(BuildInfo {
        version: native.version,
        revision: native.revision,
        abi_version: native.abi_version,
        flags: native.flags,
    })
}

/// Parses a bounded magnet URI into owned values.
///
/// # Errors
///
/// Returns an input error for an empty or oversized URI, or a native parse
/// error when libtorrent rejects the URI.
pub fn parse_magnet(uri: &str) -> Result<MagnetInfo> {
    validate_text(
        "parsing magnet URI",
        "magnet URI",
        uri,
        MAX_MAGNET_URI_BYTES,
    )?;
    let native = bridge::parse_magnet(uri.to_owned());
    ensure_native_success("parsing magnet URI", &native.error)?;
    Ok(MagnetInfo {
        v1_hash: native.has_v1.then_some(native.v1_hash),
        v2_hash: native.has_v2.then_some(native.v2_hash),
        name: native.has_name.then_some(native.name),
        trackers: native.trackers,
    })
}

impl Session {
    /// Creates a loopback-only session with discovery and peer traffic disabled.
    ///
    /// # Errors
    ///
    /// Returns an error if native session construction or ownership fails.
    pub fn new(config: SessionConfig) -> Result<Self> {
        let native_config = bridge::SessionConfigFfi {
            user_agent: format!(
                "pubky-swarm-libtorrent-engine/{}",
                env!("CARGO_PKG_VERSION")
            ),
            listen_interfaces: format!("127.0.0.1:{}", config.listen_port),
            enable_dht: false,
            enable_lsd: false,
            enable_upnp: false,
            enable_natpmp: false,
            enable_outgoing_tcp: false,
            enable_incoming_tcp: false,
            enable_outgoing_utp: false,
            enable_incoming_utp: false,
            alert_mask: u32::MAX,
        };
        let mut error = String::new();
        let native = bridge::create_session(native_config, &mut error);
        ensure_native_success("creating session", &error)?;
        if native.is_null() {
            return Err(BridgeError::NullSession);
        }
        Ok(Self { native })
    }

    /// Adds a magnet URI with an explicit save path, flags, and initial policy.
    ///
    /// # Errors
    ///
    /// Returns an input error for invalid bounded options or a native add error.
    pub fn add_magnet(&mut self, uri: &str, options: AddTorrentOptions) -> Result<TorrentSnapshot> {
        validate_text("adding magnet URI", "magnet URI", uri, MAX_MAGNET_URI_BYTES)?;
        let options = native_options("adding magnet URI", options)?;
        let native = bridge::add_magnet(self.native_pin(), uri.to_owned(), options);
        mutation_result("adding magnet URI", native)
    }

    /// Validates and adds owned `.torrent` metainfo bytes.
    ///
    /// # Errors
    ///
    /// Returns an input error for invalid bounds or a native metainfo/add error.
    pub fn add_torrent(
        &mut self,
        metainfo: &[u8],
        options: AddTorrentOptions,
    ) -> Result<TorrentSnapshot> {
        validate_bytes(
            "adding torrent metainfo",
            "torrent metainfo",
            metainfo,
            MAX_TORRENT_BYTES,
        )?;
        let options = native_options("adding torrent metainfo", options)?;
        let native = bridge::add_torrent_metainfo(self.native_pin(), metainfo.to_vec(), options);
        mutation_result("adding torrent metainfo", native)
    }

    /// Restores trusted resume data while overriding its save path and flags.
    ///
    /// # Errors
    ///
    /// Returns an input error for invalid bounds or a native restore/add error.
    pub fn add_resume_data(
        &mut self,
        resume_data: &[u8],
        options: AddTorrentOptions,
    ) -> Result<TorrentSnapshot> {
        validate_bytes(
            "restoring resume data",
            "resume data",
            resume_data,
            MAX_RESUME_DATA_BYTES,
        )?;
        let options = native_options("restoring resume data", options)?;
        let native = bridge::add_resume_data(self.native_pin(), resume_data.to_vec(), options);
        mutation_result("restoring resume data", native)
    }

    /// Requests a torrent pause. Poll [`Self::snapshot`] for applied state.
    ///
    /// # Errors
    ///
    /// Returns an error if the ID is unknown or native submission fails.
    pub fn pause(&mut self, torrent_id: TorrentId) -> Result<()> {
        let error = bridge::pause_torrent(self.native_pin(), torrent_id.0);
        ensure_native_success("pausing torrent", &error)
    }

    /// Requests a torrent resume. Poll [`Self::snapshot`] for applied state.
    ///
    /// # Errors
    ///
    /// Returns an error if the ID is unknown or native submission fails.
    pub fn resume(&mut self, torrent_id: TorrentId) -> Result<()> {
        let error = bridge::resume_torrent(self.native_pin(), torrent_id.0);
        ensure_native_success("resuming torrent", &error)
    }

    /// Removes a torrent from the session without deleting payload files.
    ///
    /// # Errors
    ///
    /// Returns an error if the ID is unknown or native removal fails.
    pub fn remove(&mut self, torrent_id: TorrentId) -> Result<()> {
        let error = bridge::remove_torrent(self.native_pin(), torrent_id.0);
        ensure_native_success("removing torrent", &error)
    }

    /// Sets one file priority in the inclusive range `0..=7`.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid priority/index/ID or native submission.
    pub fn set_file_priority(
        &mut self,
        torrent_id: TorrentId,
        file_index: u32,
        priority: u8,
    ) -> Result<()> {
        validate_priority("setting file priority", priority)?;
        let error =
            bridge::set_file_priority(self.native_pin(), torrent_id.0, file_index, priority);
        ensure_native_success("setting file priority", &error)
    }

    /// Selects a file at normal priority or excludes it at priority zero.
    ///
    /// # Errors
    ///
    /// Returns an error for an invalid file/index/ID or native submission.
    pub fn set_file_selected(
        &mut self,
        torrent_id: TorrentId,
        file_index: u32,
        selected: bool,
    ) -> Result<()> {
        self.set_file_priority(torrent_id, file_index, if selected { 4 } else { 0 })
    }

    /// Atomically submits priorities for every file in a torrent.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid priorities/count/ID or native submission.
    pub fn set_file_priorities(&mut self, torrent_id: TorrentId, priorities: &[u8]) -> Result<()> {
        if priorities.len() > MAX_FILE_COUNT {
            return invalid("setting file priorities", "too many priorities");
        }
        for &priority in priorities {
            validate_priority("setting file priorities", priority)?;
        }
        let error =
            bridge::set_file_priorities(self.native_pin(), torrent_id.0, priorities.to_vec());
        ensure_native_success("setting file priorities", &error)
    }

    /// Discards resume assumptions and schedules a full local hash recheck.
    ///
    /// # Errors
    ///
    /// Returns an error if the ID is unknown or native submission fails.
    pub fn force_recheck(&mut self, torrent_id: TorrentId) -> Result<()> {
        let error = bridge::force_recheck(self.native_pin(), torrent_id.0);
        ensure_native_success("forcing torrent recheck", &error)
    }

    /// Schedules an immediate high-priority announce to configured trackers.
    ///
    /// # Errors
    ///
    /// Returns an error if the ID is unknown or native submission fails.
    pub fn force_reannounce(&mut self, torrent_id: TorrentId) -> Result<()> {
        let error = bridge::force_reannounce(self.native_pin(), torrent_id.0);
        ensure_native_success("forcing torrent reannounce", &error)
    }

    /// Sets per-torrent transfer limits.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limits/ID or native submission.
    pub fn set_torrent_limits(&mut self, torrent_id: TorrentId, limits: RateLimits) -> Result<()> {
        let (download, upload) = native_limits("setting torrent rate limits", limits)?;
        let error = bridge::set_torrent_limits(self.native_pin(), torrent_id.0, download, upload);
        ensure_native_success("setting torrent rate limits", &error)
    }

    /// Asynchronously applies session-global transfer limits.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid limits or native submission.
    pub fn set_global_limits(&mut self, limits: RateLimits) -> Result<()> {
        let (download, upload) = native_limits("setting global rate limits", limits)?;
        let error = bridge::set_global_limits(self.native_pin(), download, upload);
        ensure_native_success("setting global rate limits", &error)
    }

    /// Starts asynchronous serialization of resume data including metainfo.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown ID or native request failure.
    pub fn save_resume_data(&mut self, torrent_id: TorrentId) -> Result<ResumeDataRequest> {
        let native = bridge::save_resume_data(self.native_pin(), torrent_id.0);
        ensure_native_success("requesting resume data", &native.error)?;
        Ok(ResumeDataRequest(native.request_id))
    }

    /// Polls one resume-data request. A ready or failed request is consumed.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown/consumed request or native save failure.
    pub fn poll_resume_data(&mut self, request: ResumeDataRequest) -> Result<ResumeDataPoll> {
        let native = bridge::poll_resume_data(self.native_pin(), request.0);
        ensure_native_success("polling resume data", &native.error)?;
        match native.state {
            0 => Ok(ResumeDataPoll::Pending),
            1 => Ok(ResumeDataPoll::Ready(native.bytes)),
            state => Err(BridgeError::Native {
                operation: "polling resume data",
                message: format!("native facade returned unknown state {state}"),
            }),
        }
    }

    /// Drains native alerts and returns fully owned session/torrent snapshots.
    ///
    /// # Errors
    ///
    /// Returns an error if native status or file metadata cannot be copied.
    pub fn snapshot(&mut self) -> Result<SessionSnapshot> {
        let native = bridge::snapshot_session(self.native_pin());
        ensure_native_success("reading session snapshot", &native.error)?;
        Ok(SessionSnapshot {
            is_paused: native.is_paused,
            is_listening: native.is_listening,
            listen_port: native.listen_port,
            torrent_count: native.torrent_count,
            global_limits: limits_from_native(
                native.global_download_limit,
                native.global_upload_limit,
            ),
            torrents: native
                .torrents
                .into_iter()
                .map(torrent_from_native)
                .collect(),
            alerts: native
                .alerts
                .into_iter()
                .map(|alert| AlertSnapshot {
                    type_id: alert.type_id,
                    type_name: alert.type_name,
                    message: alert.message,
                    category: alert.category,
                    torrent_id: alert.has_torrent_id.then_some(TorrentId(alert.torrent_id)),
                })
                .collect(),
        })
    }

    /// Aborts the native session and waits for worker shutdown.
    ///
    /// # Errors
    ///
    /// Returns an error if explicit native shutdown fails.
    pub fn close(mut self) -> Result<()> {
        let error = bridge::shutdown_session(self.native_pin());
        ensure_native_success("shutting down session", &error)
    }

    fn native_pin(&mut self) -> Pin<&mut bridge::SessionHandle> {
        self.native.pin_mut()
    }
}

fn native_options(
    operation: &'static str,
    options: AddTorrentOptions,
) -> Result<bridge::AddTorrentOptionsFfi> {
    validate_text(
        operation,
        "save path",
        &options.save_path,
        MAX_SAVE_PATH_BYTES,
    )?;
    if options.file_priorities.len() > MAX_FILE_COUNT {
        return invalid(operation, "too many initial file priorities");
    }
    for &priority in &options.file_priorities {
        validate_priority(operation, priority)?;
    }
    let (download_limit, upload_limit) = native_limits(operation, options.limits)?;
    Ok(bridge::AddTorrentOptionsFfi {
        save_path: options.save_path,
        flags: bridge::AddTorrentFlagsFfi {
            paused: options.flags.paused,
            auto_managed: options.flags.auto_managed,
            seed_mode: options.flags.seed_mode,
            upload_mode: options.flags.upload_mode,
            share_mode: options.flags.share_mode,
            sequential_download: options.flags.sequential_download,
            stop_when_ready: options.flags.stop_when_ready,
            duplicate_is_error: options.flags.duplicate_is_error,
            default_dont_download: options.flags.default_dont_download,
        },
        download_limit,
        upload_limit,
        file_priorities: options.file_priorities,
    })
}

fn native_limits(operation: &'static str, limits: RateLimits) -> Result<(i32, i32)> {
    fn one(operation: &'static str, name: &str, value: Option<u32>) -> Result<i32> {
        match value {
            None => Ok(-1),
            Some(0) => invalid(operation, format!("{name} must be positive")),
            Some(value) => i32::try_from(value).map_err(|_| BridgeError::InvalidInput {
                operation,
                message: format!("{name} exceeds i32::MAX"),
            }),
        }
    }
    Ok((
        one(
            operation,
            "download rate limit",
            limits.download_bytes_per_second,
        )?,
        one(
            operation,
            "upload rate limit",
            limits.upload_bytes_per_second,
        )?,
    ))
}

fn limits_from_native(download: i32, upload: i32) -> RateLimits {
    RateLimits {
        download_bytes_per_second: u32::try_from(download).ok(),
        upload_bytes_per_second: u32::try_from(upload).ok(),
    }
}

fn mutation_result(
    operation: &'static str,
    native: bridge::TorrentMutationFfi,
) -> Result<TorrentSnapshot> {
    ensure_native_success(operation, &native.error)?;
    Ok(torrent_from_native(native.torrent))
}

fn torrent_from_native(native: bridge::TorrentSnapshotFfi) -> TorrentSnapshot {
    TorrentSnapshot {
        id: TorrentId(native.id),
        v1_hash: native.has_v1.then_some(native.v1_hash),
        v2_hash: native.has_v2.then_some(native.v2_hash),
        name: native.name,
        save_path: native.save_path,
        state: match native.state {
            1 => TorrentState::CheckingFiles,
            2 => TorrentState::DownloadingMetadata,
            3 => TorrentState::Downloading,
            4 => TorrentState::Finished,
            5 => TorrentState::Seeding,
            7 => TorrentState::CheckingResumeData,
            state => TorrentState::Unknown(state),
        },
        progress_ppm: native.progress_ppm,
        has_metadata: native.has_metadata,
        is_paused: native.is_paused,
        is_auto_managed: native.is_auto_managed,
        is_sequential_download: native.is_sequential_download,
        is_seed_mode: native.is_seed_mode,
        is_upload_mode: native.is_upload_mode,
        is_share_mode: native.is_share_mode,
        is_finished: native.is_finished,
        is_seeding: native.is_seeding,
        total_bytes: native.total_bytes,
        wanted_bytes: native.wanted_bytes,
        wanted_done_bytes: native.wanted_done_bytes,
        all_time_download_bytes: native.all_time_download_bytes,
        all_time_upload_bytes: native.all_time_upload_bytes,
        download_rate: native.download_rate,
        upload_rate: native.upload_rate,
        connected_peers: native.connected_peers,
        connected_seeds: native.connected_seeds,
        limits: limits_from_native(native.download_limit, native.upload_limit),
        error_message: native.has_error.then_some(native.error_message),
        files: native
            .files
            .into_iter()
            .map(|file| FileSnapshot {
                index: file.index,
                path: file.path,
                size: file.size,
                priority: file.priority,
                is_selected: file.is_selected,
                is_pad_file: file.is_pad_file,
            })
            .collect(),
    }
}

fn validate_text(operation: &'static str, name: &str, value: &str, maximum: usize) -> Result<()> {
    if value.is_empty() {
        return invalid(operation, format!("{name} must not be empty"));
    }
    if value.len() > maximum {
        return invalid(operation, format!("{name} exceeds {maximum} bytes"));
    }
    if value.as_bytes().contains(&0) {
        return invalid(operation, format!("{name} contains a NUL byte"));
    }
    Ok(())
}

fn validate_bytes(operation: &'static str, name: &str, value: &[u8], maximum: usize) -> Result<()> {
    if value.is_empty() {
        return invalid(operation, format!("{name} must not be empty"));
    }
    if value.len() > maximum {
        return invalid(operation, format!("{name} exceeds {maximum} bytes"));
    }
    Ok(())
}

fn validate_priority(operation: &'static str, priority: u8) -> Result<()> {
    if priority <= 7 {
        Ok(())
    } else {
        invalid(operation, "file priority must be in the range 0..=7")
    }
}

fn invalid<T>(operation: &'static str, message: impl Into<String>) -> Result<T> {
    Err(BridgeError::InvalidInput {
        operation,
        message: message.into(),
    })
}

fn ensure_native_success(operation: &'static str, error: &str) -> Result<()> {
    if error.is_empty() {
        Ok(())
    } else {
        Err(BridgeError::Native {
            operation,
            message: error.to_owned(),
        })
    }
}
