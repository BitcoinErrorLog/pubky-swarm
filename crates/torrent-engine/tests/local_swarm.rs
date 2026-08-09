//! Local-only acceptance probe: exercises torrent creation, seeding, adding by
//! magnet, selective file retrieval, streaming, pause/resume, forget, and
//! session persistence — all over loopback between two in-process sessions,
//! with no public content, trackers, or DHT bootstrap nodes.
//!
//! Peer discovery here uses explicit `initial_peers` because librqbit 8.1.1's
//! public session API cannot run an isolated local DHT: `SessionOptions`
//! exposes no way to set DHT bootstrap addresses, and with the DHT enabled a
//! session always falls back to the hardcoded public bootstrap nodes
//! (`dht.transmissionbt.com:6881`, `dht.libtorrent.org:25401`), which tests
//! must not contact. See the crate report for the full API analysis.

use std::io::SeekFrom;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use catalog_client::{SourceKind, parse_catalog};
use mainline::{Dht, Testnet};
use mainline_discovery::PeerDiscovery;
use swarm_protocol::InfoHashV1;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use torrent_engine::{
    AddOptions, CreateOptions, DhtMode, EngineConfig, Error, TorrentEngine, create_torrent,
    magnet_v1_info_hash,
};

/// Deterministic pseudo-random-but-reproducible file contents.
fn pattern(seed: u8, len: usize) -> Vec<u8> {
    (0..u32::try_from(len).unwrap())
        .map(|i| ((i % 251) as u8) ^ seed)
        .collect()
}

/// Ask the OS for a free TCP port, then offer librqbit a small range starting
/// there. Plain `0..1` would bind an ephemeral port, but librqbit 8.1.1
/// reports the *requested* port (0) from `tcp_listen_port()` rather than the
/// bound `local_addr`, making the ephemeral port undiscoverable through its
/// public API — so pre-allocating is the reliable way to avoid fixed-port
/// collisions here.
fn free_tcp_port_range() -> std::ops::Range<u16> {
    let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    // Widen to u32 for arithmetic so the u16 range can never overflow.
    let port = u32::from(listener.local_addr().unwrap().port());
    drop(listener);
    let end = (port + 20).min(u32::from(u16::MAX));
    let range = if end > port {
        port..end
    } else {
        // bind(0) never returns u16::MAX, but never produce an empty range.
        (port - 1)..port
    };
    u16::try_from(range.start).unwrap()..u16::try_from(range.end).unwrap()
}

/// Engine config for local tests: OS-assigned listen port range, no DHT.
fn local_config(download_dir: std::path::PathBuf) -> EngineConfig {
    let mut config = EngineConfig::new(download_dir);
    config.listen_port_range = Some(free_tcp_port_range());
    config.dht_mode = DhtMode::Disabled;
    config
}

/// Full create → seed → leech → selective-download → stream cycle.
///
/// Discovery is via an explicit loopback peer address (`initial_peers`); this
/// is deliberately *not* named a DHT test, see the module docs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)] // sequential acceptance probe; splitting hurts readability
async fn explicit_peer_seed_leech_selective_stream() {
    let tmp = TempDir::new().unwrap();

    // --- Seeder side: create content and a v1 torrent from it. ---
    let seed_root = tmp.path().join("seed-root");
    let data_dir = seed_root.join("data");
    std::fs::create_dir_all(data_dir.join("sub")).unwrap();
    let file_a = pattern(0x11, 150_000);
    let file_b = pattern(0x42, 90_000);
    std::fs::write(data_dir.join("a.bin"), &file_a).unwrap();
    std::fs::write(data_dir.join("sub").join("b.bin"), &file_b).unwrap();

    let created = create_torrent(
        &data_dir,
        CreateOptions {
            name: None,
            piece_length: Some(16 * 1024),
        },
    )
    .await
    .unwrap();
    assert_eq!(created.file_count(), 2);
    assert_eq!(created.total_length(), (file_a.len() + file_b.len()) as u64);
    let catalog_xml = format!(
        "<rss><channel><item><title>Local shared data</title>\
         <infohash>{}</infohash><size>{}</size></item></channel></rss>",
        created.info_hash_hex(),
        created.total_length()
    );
    let catalog_item = parse_catalog(
        1,
        "Local acceptance catalog",
        SourceKind::Rss,
        catalog_xml.as_bytes(),
        1,
    )
    .unwrap()
    .into_iter()
    .next()
    .expect("catalog must expose the shared torrent");
    assert_eq!(
        magnet_v1_info_hash(&catalog_item.magnet)
            .unwrap()
            .as_deref(),
        Some(created.info_hash_hex())
    );

    let seeder = TorrentEngine::new(local_config(seed_root.clone()))
        .await
        .unwrap();
    let seeder_torrent = seeder
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                // The content is already in place; overwrite lets librqbit
                // verify and seed it instead of refusing to write.
                overwrite: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    tokio::time::timeout(
        Duration::from_secs(30),
        seeder_torrent.wait_until_completed(),
    )
    .await
    .expect("seeder initial check timed out")
    .unwrap();
    let seeder_progress = seeder_torrent.progress();
    assert!(seeder_progress.finished);
    assert_eq!(seeder_progress.progress_bytes, created.total_length());

    // Find the index of sub/b.bin via typed metadata (walk order is not
    // guaranteed to match creation order assumptions).
    let seeder_meta = seeder_torrent.metadata().unwrap();
    let b_index = seeder_meta
        .files
        .iter()
        .find(|f| f.path.ends_with("b.bin"))
        .expect("b.bin must be in metadata")
        .index;
    let a_index = seeder_meta
        .files
        .iter()
        .find(|f| f.path.ends_with("a.bin"))
        .expect("a.bin must be in metadata")
        .index;
    assert!(seeder_meta.files.iter().all(|f| f.included));

    // --- Leecher side: add by magnet, selective download of b.bin only. ---
    let leech_dir = tmp.path().join("leech");
    let leecher = TorrentEngine::new(local_config(leech_dir.clone()))
        .await
        .unwrap();
    let seeder_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        seeder.listen_port().expect("seeder must listen"),
    );

    let leech_torrent = leecher
        .add_magnet(
            &catalog_item.magnet,
            AddOptions {
                only_files: Some(vec![b_index]),
                initial_peers: Some(vec![seeder_addr]),
                disable_trackers: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(leech_torrent.info_hash(), created.info_hash_hex());

    tokio::time::timeout(
        Duration::from_secs(60),
        leech_torrent.wait_until_completed(),
    )
    .await
    .expect("leecher download timed out")
    .unwrap();

    // Selective retrieval: b.bin fully downloaded, a.bin untouched.
    let progress = leech_torrent.progress();
    assert!(progress.finished);
    let b_have = progress
        .files
        .iter()
        .find(|f| f.index == b_index)
        .map_or(0, |f| f.have_bytes);
    let a_have = progress
        .files
        .iter()
        .find(|f| f.index == a_index)
        .map_or(0, |f| f.have_bytes);
    assert_eq!(b_have, file_b.len() as u64);
    // BitTorrent pieces span file boundaries: the piece covering the end of
    // b.bin overlaps the start of a.bin, so librqbit writes that overlap to
    // disk. With two files there is exactly one boundary, so the unselected
    // file can hold at most one piece of spill.
    assert!(
        a_have < 16 * 1024,
        "unselected file must only contain piece-boundary spill, got {a_have} bytes"
    );

    let leech_meta = leech_torrent.metadata().unwrap();
    assert!(leech_meta.files[b_index].included);
    assert!(!leech_meta.files[a_index].included);
    let b_rel = leech_meta.files[b_index].path.clone();
    let b_disk = std::fs::read(leech_dir.join("data").join(&b_rel)).unwrap();
    assert_eq!(b_disk, file_b, "downloaded b.bin content must match");

    // --- Streaming: seek + partial read must return exact bytes. ---
    let mut stream = leech_torrent.stream_file(b_index).unwrap();
    let offset = 1000u64;
    let take = 5000usize;
    stream.seek(SeekFrom::Start(offset)).await.unwrap();
    let mut buf = vec![0u8; take];
    stream.read_exact(&mut buf).await.unwrap();
    let start = usize::try_from(offset).unwrap();
    assert_eq!(buf, file_b[start..start + take]);

    // Seek backwards and read to end: full-file equality through the stream.
    stream.seek(SeekFrom::Start(0)).await.unwrap();
    let mut whole = Vec::new();
    stream.read_to_end(&mut whole).await.unwrap();
    assert_eq!(whole, file_b);

    // Out-of-range file access is a typed error.
    assert!(matches!(
        leech_torrent.stream_file(999),
        Err(Error::FileIndexOutOfRange {
            index: 999,
            file_count: 2
        })
    ));
    assert!(matches!(
        leech_torrent.update_only_files(&[999]).await,
        Err(Error::FileIndexOutOfRange {
            index: 999,
            file_count: 2
        })
    ));
    assert!(matches!(
        leech_torrent.update_only_files(&[]).await,
        Err(Error::EmptyFileSelection)
    ));

    // --- Pause/resume are clean and idempotent. ---
    leech_torrent.pause().await.unwrap();
    assert!(leech_torrent.is_paused());
    leech_torrent.pause().await.unwrap(); // no-op
    leech_torrent.resume().await.unwrap();
    assert!(!leech_torrent.is_paused());
    leech_torrent.resume().await.unwrap(); // no-op

    // --- Forget removes the torrent from the session (files kept). ---
    let leech_id = leech_torrent.id();
    leech_torrent.forget(false).await.unwrap();
    assert!(leecher.get(leech_id).is_none());
    assert!(leecher.list().is_empty());
    assert!(leech_dir.join("data").join(&b_rel).exists());

    // --- Seeder saw upload traffic. ---
    assert!(seeder_torrent.progress().uploaded_bytes > 0);

    seeder.shutdown().await;
    leecher.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mainline_discovery_feeds_verified_torrent_transfer() {
    let tmp = TempDir::new().unwrap();
    let seed_root = tmp.path().join("seed-mainline");
    let leech_root = tmp.path().join("leech-mainline");
    std::fs::create_dir_all(&seed_root).unwrap();
    let content = pattern(0x29, 128_000);
    let payload_path = seed_root.join("mainline.bin");
    std::fs::write(&payload_path, &content).unwrap();
    let created = create_torrent(
        &payload_path,
        CreateOptions {
            name: None,
            piece_length: Some(16 * 1024),
        },
    )
    .await
    .unwrap();

    let seeder = TorrentEngine::new(local_config(seed_root.clone()))
        .await
        .unwrap();
    let seed_torrent = seeder
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                overwrite: true,
                disable_trackers: true,
                ..AddOptions::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), seed_torrent.wait_until_completed())
        .await
        .expect("seeder verification timed out")
        .unwrap();

    let testnet = Testnet::builder(5).build().unwrap();
    let build_discovery = || {
        PeerDiscovery::new(
            Dht::builder()
                .bootstrap(&testnet.bootstrap)
                .bind_address(Ipv4Addr::LOCALHOST)
                .build()
                .unwrap()
                .as_async(),
        )
    };
    let announcer = build_discovery();
    let resolver = build_discovery();
    let info_hash: InfoHashV1 = created.info_hash_hex().parse().unwrap();
    announcer
        .announce(info_hash, seeder.listen_port().unwrap())
        .await
        .unwrap();
    let peers = resolver
        .wait_for_peers(info_hash, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(!peers.is_empty());

    let leecher = TorrentEngine::new(local_config(leech_root.clone()))
        .await
        .unwrap();
    let downloaded = leecher
        .add_magnet(
            &created.magnet(),
            AddOptions {
                initial_peers: Some(peers),
                disable_trackers: true,
                ..AddOptions::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), downloaded.wait_until_completed())
        .await
        .expect("Mainline-discovered transfer timed out")
        .unwrap();
    assert_eq!(
        std::fs::read(leech_root.join("mainline.bin")).unwrap(),
        content
    );

    leecher.shutdown().await;
    seeder.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_persistence_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let download_dir = tmp.path().join("dl");
    let persistence_dir = tmp.path().join("persist");

    // Content in place so the torrent seeds immediately.
    std::fs::create_dir_all(&download_dir).unwrap();
    let content = pattern(0x77, 40_000);
    std::fs::write(download_dir.join("single.bin"), &content).unwrap();

    let created = create_torrent(
        &download_dir.join("single.bin"),
        CreateOptions {
            name: None,
            piece_length: Some(16 * 1024),
        },
    )
    .await
    .unwrap();

    let mut config = local_config(download_dir.clone());
    config.persistence_dir = Some(persistence_dir.clone());
    config.fastresume = true;

    let engine = TorrentEngine::new(config).await.unwrap();
    let torrent = engine
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), torrent.wait_until_completed())
        .await
        .expect("initial check timed out")
        .unwrap();
    assert_eq!(engine.list().len(), 1);
    engine.shutdown().await;

    // Session state must have been written to the persistence dir.
    assert!(
        std::fs::read_dir(&persistence_dir).unwrap().count() > 0,
        "persistence directory must not be empty"
    );

    // A new session with the same config restores the torrent.
    let mut config = local_config(download_dir);
    config.persistence_dir = Some(persistence_dir);
    config.fastresume = true;
    let engine2 = TorrentEngine::new(config).await.unwrap();

    let restored = engine2.list();
    assert_eq!(
        restored.len(),
        1,
        "torrent must be restored from persistence"
    );
    assert_eq!(restored[0].info_hash(), created.info_hash_hex());
    let meta = restored[0].metadata().unwrap();
    assert_eq!(meta.total_length, content.len() as u64);
    assert_eq!(meta.files.len(), 1);

    engine2.shutdown().await;
}

/// Two-stage magnet flow over loopback: the leecher resolves metadata from
/// the seeder via librqbit's `list_only` path, the engine validates the
/// resolved metainfo, re-adds it as metainfo (preserving the magnet's
/// trackers and merging seen peers), and downloads. The `list_only` stage
/// must not leave a managed torrent behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn magnet_two_stage_resolve_then_download() {
    let tmp = TempDir::new().unwrap();

    let seed_root = tmp.path().join("seed-root");
    std::fs::create_dir_all(&seed_root).unwrap();
    let content = pattern(0x5A, 80_000);
    std::fs::write(seed_root.join("payload.bin"), &content).unwrap();

    let created = create_torrent(
        &seed_root.join("payload.bin"),
        CreateOptions {
            name: None,
            piece_length: Some(16 * 1024),
        },
    )
    .await
    .unwrap();

    let seeder = TorrentEngine::new(local_config(seed_root.clone()))
        .await
        .unwrap();
    let seed_torrent = seeder
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                overwrite: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), seed_torrent.wait_until_completed())
        .await
        .expect("seeder initial check timed out")
        .unwrap();

    let leech_dir = tmp.path().join("leech");
    let leecher = TorrentEngine::new(local_config(leech_dir.clone()))
        .await
        .unwrap();
    let seeder_addr = SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        seeder.listen_port().expect("seeder must listen"),
    );

    let torrent = leecher
        .add_magnet(
            &created.magnet(),
            AddOptions {
                initial_peers: Some(vec![seeder_addr]),
                disable_trackers: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // The list_only resolution stage must not have left a managed torrent:
    // exactly one torrent exists, and it is the one we got back.
    let managed = leecher.list();
    assert_eq!(managed.len(), 1);
    assert_eq!(managed[0].id(), torrent.id());
    assert_eq!(torrent.info_hash(), created.info_hash_hex());

    tokio::time::timeout(Duration::from_secs(60), torrent.wait_until_completed())
        .await
        .expect("download timed out")
        .unwrap();

    let on_disk = std::fs::read(leech_dir.join("payload.bin")).unwrap();
    assert_eq!(on_disk, content);

    seeder.shutdown().await;
    leecher.shutdown().await;
}
