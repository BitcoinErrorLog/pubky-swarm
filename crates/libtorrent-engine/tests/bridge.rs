//! Native acceptance tests for the pinned libtorrent bridge.

use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use libtorrent_engine::{
    AddTorrentFlags, AddTorrentOptions, MAX_MAGNET_URI_BYTES, MAX_TORRENT_BYTES, RateLimits,
    ResumeDataPoll, Session, SessionConfig, TorrentId, build_info, parse_magnet,
};

const OFFICIAL_V1: &str = "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const OFFICIAL_V2: &str = concat!(
    "magnet:?xt=urn:btmh:1220",
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
);
const OFFICIAL_HYBRID: &str = concat!(
    "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
    "&xt=urn:btmh:1220",
    "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
);
static TEMP_ID: AtomicU64 = AtomicU64::new(1);

fn bencode_bytes(value: &[u8]) -> Vec<u8> {
    let mut encoded = value.len().to_string().into_bytes();
    encoded.push(b':');
    encoded.extend_from_slice(value);
    encoded
}

fn bencode_integer(value: i64) -> Vec<u8> {
    format!("i{value}e").into_bytes()
}

fn bencode_list(values: &[Vec<u8>]) -> Vec<u8> {
    let mut encoded = vec![b'l'];
    for value in values {
        encoded.extend_from_slice(value);
    }
    encoded.push(b'e');
    encoded
}

fn bencode_dictionary(mut values: Vec<(&[u8], Vec<u8>)>) -> Vec<u8> {
    values.sort_by(|left, right| left.0.cmp(right.0));
    let mut encoded = vec![b'd'];
    for (key, value) in values {
        encoded.extend_from_slice(&bencode_bytes(key));
        encoded.extend_from_slice(&value);
    }
    encoded.push(b'e');
    encoded
}

fn torrent_file(info: Vec<u8>) -> Vec<u8> {
    bencode_dictionary(vec![(b"info", info)])
}

fn v1_fixture() -> Vec<u8> {
    let first = bencode_dictionary(vec![
        (b"length", bencode_integer(3)),
        (b"path", bencode_list(&[bencode_bytes(b"first.txt")])),
    ]);
    let second = bencode_dictionary(vec![
        (b"length", bencode_integer(3)),
        (b"path", bencode_list(&[bencode_bytes(b"second.txt")])),
    ]);
    torrent_file(bencode_dictionary(vec![
        (b"files", bencode_list(&[first, second])),
        (b"name", bencode_bytes(b"v1-fixture")),
        (b"piece length", bencode_integer(16_384)),
        (
            b"pieces",
            bencode_bytes(&[
                0x1f, 0x8a, 0xc1, 0x0f, 0x23, 0xc5, 0xb5, 0xbc, 0x11, 0x67, 0xbd, 0xa8, 0x4b, 0x83,
                0x3e, 0x5c, 0x05, 0x7a, 0x77, 0xd2,
            ]),
        ),
    ]))
}

fn v2_file_tree(name: &[u8]) -> Vec<u8> {
    let leaf = bencode_dictionary(vec![
        (b"length", bencode_integer(4)),
        (
            b"pieces root",
            bencode_bytes(&[
                0x3a, 0x6e, 0xb0, 0x79, 0x0f, 0x39, 0xac, 0x87, 0xc9, 0x4f, 0x38, 0x56, 0xb2, 0xdd,
                0x2c, 0x5d, 0x11, 0x0e, 0x68, 0x11, 0x60, 0x22, 0x61, 0xa9, 0xa9, 0x23, 0xd3, 0xbb,
                0x23, 0xad, 0xc8, 0xb7,
            ]),
        ),
    ]);
    bencode_dictionary(vec![(name, bencode_dictionary(vec![(b"", leaf)]))])
}

fn v2_fixture() -> Vec<u8> {
    torrent_file(bencode_dictionary(vec![
        (b"file tree", v2_file_tree(b"v2-fixture")),
        (b"meta version", bencode_integer(2)),
        (b"name", bencode_bytes(b"v2-fixture")),
        (b"piece length", bencode_integer(16_384)),
    ]))
}

fn hybrid_fixture() -> Vec<u8> {
    torrent_file(bencode_dictionary(vec![
        (b"file tree", v2_file_tree(b"hybrid-file")),
        (b"length", bencode_integer(4)),
        (b"meta version", bencode_integer(2)),
        (b"name", bencode_bytes(b"hybrid-file")),
        (b"piece length", bencode_integer(16_384)),
        (
            b"pieces",
            bencode_bytes(&[
                0xa1, 0x7c, 0x9a, 0xaa, 0x61, 0xe8, 0x0a, 0x1b, 0xf7, 0x1d, 0x0d, 0x85, 0x0a, 0xf4,
                0xe5, 0xba, 0xa9, 0x80, 0x0b, 0xbd,
            ]),
        ),
    ]))
}

fn temp_directory(label: &str) -> std::path::PathBuf {
    let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pubky-swarm-libtorrent-{label}-{}-{id}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temporary save path must be created");
    path
}

fn options(path: &std::path::Path) -> AddTorrentOptions {
    AddTorrentOptions::new(
        path.to_str()
            .expect("temporary test path must be valid UTF-8"),
    )
}

fn wait_for_torrent(
    session: &mut Session,
    torrent_id: TorrentId,
    predicate: impl Fn(&libtorrent_engine::TorrentSnapshot) -> bool,
) -> libtorrent_engine::TorrentSnapshot {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot().expect("snapshot must succeed");
        let torrent = snapshot
            .torrents
            .into_iter()
            .find(|torrent| torrent.id == torrent_id)
            .expect("torrent must remain in the session");
        if predicate(&torrent) {
            return torrent;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for torrent state: {torrent:?}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn reports_the_linked_libtorrent_build() {
    let info = build_info().expect("linked libtorrent must report build information");

    assert_eq!(info.version, "2.0.13.0");
    assert_eq!(info.revision, "4a0dcf5cf");
    assert_eq!(info.abi_version, 3);
    for expected in [
        "static-link=1",
        "openssl=1",
        "dht=1",
        "extensions=1",
        "logging=0",
        "i2p=0",
        "deprecated-functions=0",
        "exceptions=1",
    ] {
        assert!(
            info.flags.iter().any(|flag| flag == expected),
            "missing native build flag {expected:?}: {:?}",
            info.flags
        );
    }
}

#[test]
fn parses_official_v1_vector() {
    let magnet = parse_magnet(OFFICIAL_V1).expect("official v1 vector must parse");

    assert_eq!(
        magnet.v1_hash.as_deref(),
        Some("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd")
    );
    assert_eq!(magnet.v2_hash, None);
}

#[test]
fn parses_official_v2_vector() {
    let magnet = parse_magnet(OFFICIAL_V2).expect("official v2 vector must parse");

    assert_eq!(magnet.v1_hash, None);
    assert_eq!(
        magnet.v2_hash.as_deref(),
        Some("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd")
    );
}

#[test]
fn parses_official_hybrid_vector() {
    let magnet = parse_magnet(OFFICIAL_HYBRID).expect("official hybrid vector must parse");

    assert_eq!(
        magnet.v1_hash.as_deref(),
        Some("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd")
    );
    assert_eq!(
        magnet.v2_hash.as_deref(),
        Some("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd")
    );
}

#[test]
fn copies_name_and_trackers_into_owned_rust_values() {
    let magnet = parse_magnet(concat!(
        "magnet:?xt=urn:btih:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
        "&tr=http://1&tr=http://2&tr=http://3&tr=http://3&dn=foo"
    ))
    .expect("official field vector must parse");

    assert_eq!(magnet.name.as_deref(), Some("foo"));
    assert_eq!(
        magnet.trackers,
        ["http://1", "http://2", "http://3", "http://3"]
    );
}

#[test]
fn rejects_an_invalid_magnet_with_an_owned_error() {
    let error =
        parse_magnet("magnet:?xt=urn:btih:abababab").expect_err("truncated hash must be rejected");

    assert!(!error.to_string().is_empty());
}

#[test]
fn creates_snapshots_and_explicitly_destroys_an_isolated_session() {
    let mut session =
        Session::new(SessionConfig::default()).expect("native session must be constructed");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut alerts = Vec::new();

    let snapshot = loop {
        let mut snapshot = session
            .snapshot()
            .expect("snapshot must be owned and valid");
        alerts.append(&mut snapshot.alerts);
        let has_listen_alert = alerts
            .iter()
            .any(|alert| alert.type_name == "listen_succeeded");
        if (snapshot.is_listening && has_listen_alert) || Instant::now() >= deadline {
            break snapshot;
        }
        thread::sleep(Duration::from_millis(10));
    };

    assert!(!snapshot.is_paused);
    assert!(snapshot.is_listening, "loopback listener did not start");
    assert_ne!(snapshot.listen_port, 0);
    assert_eq!(snapshot.torrent_count, 0);
    assert!(
        alerts
            .iter()
            .any(|alert| alert.type_name == "listen_succeeded"),
        "expected a copied listen_succeeded alert, got {alerts:?}"
    );

    session
        .close()
        .expect("explicit native shutdown and destruction must succeed");
}

#[test]
fn adds_owned_v1_v2_and_hybrid_metainfo_snapshots() {
    let save_path = temp_directory("metainfo");
    let mut session = Session::new(SessionConfig::default()).expect("session must start");

    let v1 = session
        .add_torrent(&v1_fixture(), options(&save_path))
        .expect("generated v1 fixture must be accepted");
    let v2 = session
        .add_torrent(&v2_fixture(), options(&save_path))
        .expect("generated v2 fixture must be accepted");
    let hybrid = session
        .add_torrent(&hybrid_fixture(), options(&save_path))
        .expect("generated hybrid fixture must be accepted");

    assert!(v1.v1_hash.is_some());
    assert_eq!(v1.v2_hash, None);
    assert_eq!(v1.name, "v1-fixture");
    assert_eq!(v1.files.len(), 2);
    assert!(v1.files.iter().all(|file| file.is_selected));

    assert_eq!(v2.v1_hash, None);
    assert!(v2.v2_hash.is_some());
    assert_eq!(v2.name, "v2-fixture");
    assert_eq!(v2.files.iter().filter(|file| !file.is_pad_file).count(), 1);

    assert!(hybrid.v1_hash.is_some());
    assert!(hybrid.v2_hash.is_some());
    assert_eq!(
        hybrid.files.iter().filter(|file| !file.is_pad_file).count(),
        1
    );

    let snapshot = session.snapshot().expect("snapshot must succeed");
    assert_eq!(snapshot.torrent_count, 3);
    assert_eq!(snapshot.torrents.len(), 3);

    session.close().expect("session must close");
    fs::remove_dir_all(save_path).expect("temporary directory must be removed");
}

#[test]
fn adds_and_removes_a_magnet_without_exposing_a_native_handle() {
    let save_path = temp_directory("magnet");
    let mut session = Session::new(SessionConfig::default()).expect("session must start");
    let torrent = session
        .add_magnet(OFFICIAL_HYBRID, options(&save_path))
        .expect("hybrid magnet must be added");

    assert!(torrent.v1_hash.is_some());
    assert!(torrent.v2_hash.is_some());
    assert!(!torrent.has_metadata);
    assert!(torrent.files.is_empty());

    session
        .remove(torrent.id)
        .expect("removal without delete flags must succeed");
    assert!(
        session
            .snapshot()
            .expect("snapshot must succeed")
            .torrents
            .is_empty()
    );

    session.close().expect("session must close");
    fs::remove_dir_all(save_path).expect("temporary directory must be removed");
}

#[test]
fn controls_pause_selection_recheck_reannounce_and_rate_limits_locally() {
    let save_path = temp_directory("controls");
    let mut session = Session::new(SessionConfig::default()).expect("session must start");
    let torrent = session
        .add_torrent(&v1_fixture(), options(&save_path))
        .expect("v1 fixture must be added");

    session
        .pause(torrent.id)
        .expect("pause request must succeed");
    let paused = wait_for_torrent(&mut session, torrent.id, |value| value.is_paused);
    assert!(paused.is_paused);

    session
        .set_file_priorities(torrent.id, &[0, 7])
        .expect("file priorities must be accepted");
    let prioritized = wait_for_torrent(&mut session, torrent.id, |value| {
        value.files[0].priority == 0 && value.files[1].priority == 7
    });
    assert!(!prioritized.files[0].is_selected);
    assert!(prioritized.files[1].is_selected);

    session
        .set_file_selected(torrent.id, 0, true)
        .expect("file selection must be accepted");
    wait_for_torrent(&mut session, torrent.id, |value| {
        value.files[0].priority == 4
    });

    let torrent_limits = RateLimits {
        download_bytes_per_second: Some(32_000),
        upload_bytes_per_second: Some(16_000),
    };
    session
        .set_torrent_limits(torrent.id, torrent_limits)
        .expect("torrent limits must be accepted");
    wait_for_torrent(&mut session, torrent.id, |value| {
        value.limits == torrent_limits
    });

    let global_limits = RateLimits {
        download_bytes_per_second: Some(64_000),
        upload_bytes_per_second: Some(48_000),
    };
    session
        .set_global_limits(global_limits)
        .expect("global limits must be accepted");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let snapshot = session.snapshot().expect("snapshot must succeed");
        if snapshot.global_limits == global_limits {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for global limits: {:?}",
            snapshot.global_limits
        );
        thread::sleep(Duration::from_millis(10));
    }

    session
        .force_reannounce(torrent.id)
        .expect("reannounce command must be accepted without trackers");
    session
        .force_recheck(torrent.id)
        .expect("local recheck command must be accepted");
    session
        .resume(torrent.id)
        .expect("resume request must succeed");
    wait_for_torrent(&mut session, torrent.id, |value| !value.is_paused);

    session.close().expect("session must close");
    fs::remove_dir_all(save_path).expect("temporary directory must be removed");
}

#[test]
fn saves_restores_and_removes_without_deleting_payload_files() {
    let save_path = temp_directory("resume");
    let payload = save_path.join("kept-after-remove.txt");
    fs::write(&payload, b"must remain").expect("sentinel payload must be written");
    let fixture = hybrid_fixture();
    let mut session = Session::new(SessionConfig::default()).expect("session must start");
    let torrent = session
        .add_torrent(&fixture, options(&save_path))
        .expect("hybrid fixture must be added");

    let request = session
        .save_resume_data(torrent.id)
        .expect("resume-data request must start");
    let deadline = Instant::now() + Duration::from_secs(5);
    let resume_data = loop {
        match session
            .poll_resume_data(request)
            .expect("resume-data polling must succeed")
        {
            ResumeDataPoll::Pending => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for resume data"
                );
                thread::sleep(Duration::from_millis(10));
            }
            ResumeDataPoll::Ready(bytes) => break bytes,
        }
    };
    assert!(!resume_data.is_empty());

    session
        .remove(torrent.id)
        .expect("removal must not request payload deletion");
    assert_eq!(
        fs::read(&payload).expect("payload must survive removal"),
        b"must remain"
    );
    session.close().expect("first session must close");

    let mut restored = Session::new(SessionConfig::default()).expect("second session must start");
    let mut restored_options = options(&save_path);
    restored_options.flags = AddTorrentFlags {
        paused: true,
        ..AddTorrentFlags::default()
    };
    let restored_torrent = restored
        .add_resume_data(&resume_data, restored_options)
        .expect("owned resume bytes must restore the torrent");
    assert!(restored_torrent.v1_hash.is_some());
    assert!(restored_torrent.v2_hash.is_some());
    assert!(restored_torrent.is_paused);

    restored.close().expect("second session must close");
    fs::remove_dir_all(save_path).expect("temporary directory must be removed");
}

#[test]
fn rejects_oversized_and_invalid_inputs_before_native_calls() {
    let oversized_magnet = "x".repeat(MAX_MAGNET_URI_BYTES + 1);
    assert!(parse_magnet(&oversized_magnet).is_err());

    let save_path = temp_directory("limits");
    let mut session = Session::new(SessionConfig::default()).expect("session must start");
    assert!(
        session
            .add_torrent(&vec![b'x'; MAX_TORRENT_BYTES + 1], options(&save_path))
            .is_err()
    );
    assert!(
        session
            .add_torrent(b"not-bencoded-metainfo", options(&save_path))
            .is_err()
    );

    let torrent = session
        .add_torrent(&v1_fixture(), options(&save_path))
        .expect("valid fixture must be added");
    assert!(session.set_file_priority(torrent.id, 0, 8).is_err());
    assert!(
        session
            .set_global_limits(RateLimits {
                download_bytes_per_second: Some(0),
                upload_bytes_per_second: None,
            })
            .is_err()
    );

    session.close().expect("session must close");
    fs::remove_dir_all(save_path).expect("temporary directory must be removed");
}
