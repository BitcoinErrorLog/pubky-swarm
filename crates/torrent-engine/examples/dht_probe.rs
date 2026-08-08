//! Live Mainline DHT acceptance probe using generated content.
//!
//! This is intentionally an executable rather than an always-on test: it
//! contacts public `BitTorrent` bootstrap nodes and depends on NAT/firewall
//! conditions. It never downloads third-party content.

use std::net::{Ipv4Addr, TcpListener};
use std::ops::Range;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use torrent_engine::{
    AddOptions, CreateOptions, DhtMode, EngineConfig, TorrentEngine, create_torrent,
};

fn available_port_range() -> Range<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("reserve local TCP port");
    let start = u32::from(listener.local_addr().expect("reserved TCP address").port());
    drop(listener);
    let end = (start + 32).min(u32::from(u16::MAX));
    let (start, end) = if end > start {
        (start, end)
    } else {
        (start - 1, start)
    };
    u16::try_from(start).expect("port range start")..u16::try_from(end).expect("port range end")
}

fn live_config(download_dir: std::path::PathBuf) -> EngineConfig {
    let mut config = EngineConfig::new(download_dir);
    config.listen_port_range = Some(available_port_range());
    config.dht_mode = DhtMode::Ephemeral;
    config.enable_upnp_port_forwarding = true;
    config
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let workspace = TempDir::new()?;
    let seed_root = workspace.path().join("seed");
    let leech_root = workspace.path().join("leech");
    std::fs::create_dir_all(&seed_root)?;

    let payload: Vec<u8> = (0..512_000_u32)
        .map(|index| ((index.wrapping_mul(31)) % 251) as u8)
        .collect();
    let payload_path = seed_root.join("pubky-swarm-dht-probe.bin");
    std::fs::write(&payload_path, &payload)?;
    let created = create_torrent(
        &payload_path,
        CreateOptions {
            name: None,
            piece_length: Some(64 * 1024),
        },
    )
    .await?;

    let seeder = TorrentEngine::new(live_config(seed_root.clone())).await?;
    let seed_torrent = seeder
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                overwrite: true,
                disable_trackers: true,
                ..AddOptions::default()
            },
        )
        .await?;
    tokio::time::timeout(Duration::from_secs(30), seed_torrent.wait_until_completed()).await??;

    println!(
        "seed-ready info_hash={} port={:?}",
        created.info_hash_hex(),
        seeder.listen_port()
    );
    tokio::time::sleep(Duration::from_secs(15)).await;

    let started = Instant::now();
    let leecher = TorrentEngine::new(live_config(leech_root.clone())).await?;
    let downloaded = tokio::time::timeout(
        Duration::from_secs(180),
        leecher.add_magnet(
            &created.magnet(),
            AddOptions {
                disable_trackers: true,
                ..AddOptions::default()
            },
        ),
    )
    .await??;
    let metadata_elapsed = started.elapsed();
    tokio::time::timeout(Duration::from_secs(180), downloaded.wait_until_completed()).await??;
    let total_elapsed = started.elapsed();

    let received = std::fs::read(leech_root.join("pubky-swarm-dht-probe.bin"))?;
    if received != payload {
        return Err("downloaded payload mismatch".into());
    }

    println!(
        "dht-probe-ok metadata_ms={} total_ms={} uploaded_bytes={}",
        metadata_elapsed.as_millis(),
        total_elapsed.as_millis(),
        seed_torrent.progress().uploaded_bytes
    );
    leecher.shutdown().await;
    seeder.shutdown().await;
    Ok(())
}
