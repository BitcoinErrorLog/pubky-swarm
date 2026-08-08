#[cxx::bridge(namespace = "pubky_swarm::libtorrent")]
pub mod bridge {
    #[derive(Debug)]
    struct BuildInfoFfi {
        version: String,
        revision: String,
        abi_version: u32,
        flags: Vec<String>,
        error: String,
    }

    #[derive(Debug)]
    struct MagnetInfoFfi {
        v1_hash: String,
        has_v1: bool,
        v2_hash: String,
        has_v2: bool,
        name: String,
        has_name: bool,
        trackers: Vec<String>,
        error: String,
    }

    struct AddTorrentFlagsFfi {
        paused: bool,
        auto_managed: bool,
        seed_mode: bool,
        upload_mode: bool,
        share_mode: bool,
        sequential_download: bool,
        stop_when_ready: bool,
        duplicate_is_error: bool,
        default_dont_download: bool,
    }

    struct AddTorrentOptionsFfi {
        save_path: String,
        flags: AddTorrentFlagsFfi,
        download_limit: i32,
        upload_limit: i32,
        file_priorities: Vec<u8>,
    }

    struct SessionConfigFfi {
        user_agent: String,
        listen_interfaces: String,
        enable_dht: bool,
        enable_lsd: bool,
        enable_upnp: bool,
        enable_natpmp: bool,
        enable_outgoing_tcp: bool,
        enable_incoming_tcp: bool,
        enable_outgoing_utp: bool,
        enable_incoming_utp: bool,
        alert_mask: u32,
    }

    #[derive(Debug)]
    struct AlertSnapshotFfi {
        type_id: i32,
        type_name: String,
        message: String,
        category: u32,
        torrent_id: u64,
        has_torrent_id: bool,
    }

    #[derive(Debug)]
    struct FileSnapshotFfi {
        index: u32,
        path: String,
        size: i64,
        priority: u8,
        is_selected: bool,
        is_pad_file: bool,
    }

    #[derive(Debug)]
    struct TorrentSnapshotFfi {
        id: u64,
        v1_hash: String,
        has_v1: bool,
        v2_hash: String,
        has_v2: bool,
        name: String,
        save_path: String,
        state: u8,
        progress_ppm: u32,
        has_metadata: bool,
        is_paused: bool,
        is_auto_managed: bool,
        is_sequential_download: bool,
        is_seed_mode: bool,
        is_upload_mode: bool,
        is_share_mode: bool,
        is_finished: bool,
        is_seeding: bool,
        total_bytes: i64,
        wanted_bytes: i64,
        wanted_done_bytes: i64,
        all_time_download_bytes: i64,
        all_time_upload_bytes: i64,
        download_rate: i32,
        upload_rate: i32,
        connected_peers: i32,
        connected_seeds: i32,
        download_limit: i32,
        upload_limit: i32,
        error_message: String,
        has_error: bool,
        files: Vec<FileSnapshotFfi>,
    }

    #[derive(Debug)]
    struct TorrentMutationFfi {
        torrent: TorrentSnapshotFfi,
        error: String,
    }

    #[derive(Debug)]
    struct ResumeRequestFfi {
        request_id: u64,
        error: String,
    }

    #[derive(Debug)]
    struct ResumeDataFfi {
        state: u8,
        bytes: Vec<u8>,
        error: String,
    }

    #[derive(Debug)]
    struct SessionSnapshotFfi {
        is_paused: bool,
        is_listening: bool,
        listen_port: u16,
        torrent_count: u64,
        global_download_limit: i32,
        global_upload_limit: i32,
        torrents: Vec<TorrentSnapshotFfi>,
        alerts: Vec<AlertSnapshotFfi>,
        error: String,
    }

    unsafe extern "C++" {
        include!("facade.hpp");

        type SessionHandle;

        fn build_info() -> BuildInfoFfi;
        fn parse_magnet(uri: String) -> MagnetInfoFfi;
        fn create_session(config: SessionConfigFfi, error: &mut String)
        -> UniquePtr<SessionHandle>;
        fn add_magnet(
            session: Pin<&mut SessionHandle>,
            uri: String,
            options: AddTorrentOptionsFfi,
        ) -> TorrentMutationFfi;
        fn add_torrent_metainfo(
            session: Pin<&mut SessionHandle>,
            metainfo: Vec<u8>,
            options: AddTorrentOptionsFfi,
        ) -> TorrentMutationFfi;
        fn add_resume_data(
            session: Pin<&mut SessionHandle>,
            resume_data: Vec<u8>,
            options: AddTorrentOptionsFfi,
        ) -> TorrentMutationFfi;
        fn pause_torrent(session: Pin<&mut SessionHandle>, torrent_id: u64) -> String;
        fn resume_torrent(session: Pin<&mut SessionHandle>, torrent_id: u64) -> String;
        fn remove_torrent(session: Pin<&mut SessionHandle>, torrent_id: u64) -> String;
        fn set_file_priority(
            session: Pin<&mut SessionHandle>,
            torrent_id: u64,
            file_index: u32,
            priority: u8,
        ) -> String;
        fn set_file_priorities(
            session: Pin<&mut SessionHandle>,
            torrent_id: u64,
            priorities: Vec<u8>,
        ) -> String;
        fn force_recheck(session: Pin<&mut SessionHandle>, torrent_id: u64) -> String;
        fn force_reannounce(session: Pin<&mut SessionHandle>, torrent_id: u64) -> String;
        fn set_torrent_limits(
            session: Pin<&mut SessionHandle>,
            torrent_id: u64,
            download_limit: i32,
            upload_limit: i32,
        ) -> String;
        fn set_global_limits(
            session: Pin<&mut SessionHandle>,
            download_limit: i32,
            upload_limit: i32,
        ) -> String;
        fn save_resume_data(session: Pin<&mut SessionHandle>, torrent_id: u64) -> ResumeRequestFfi;
        fn poll_resume_data(session: Pin<&mut SessionHandle>, request_id: u64) -> ResumeDataFfi;
        fn snapshot_session(session: Pin<&mut SessionHandle>) -> SessionSnapshotFfi;
        fn shutdown_session(session: Pin<&mut SessionHandle>) -> String;
    }
}
