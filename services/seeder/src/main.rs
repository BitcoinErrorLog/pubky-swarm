//! Bounded non-authoritative dataset seeder.

#![forbid(unsafe_code)]

use std::net::{Ipv4Addr, TcpListener};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dataset_torrent::TorrentDatasetReader;
use mainline::Dht;
use mainline_discovery::PeerDiscovery;
use swarm_head::HeadClient;
use swarm_protocol::PublisherId;
use swarm_store::Store;
use torrent_engine::{DhtMode, EngineConfig, TorrentEngine};

const DEFAULT_MAX_PUBLISHERS: usize = 100;
const DEFAULT_MAX_DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;

struct Target {
    publisher: PublisherId,
    head_client: HeadClient,
    reader: TorrentDatasetReader,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.data_dir)?;
    let mut engine_config = EngineConfig::new(config.data_dir.join("downloads"));
    engine_config.persistence_dir = Some(config.data_dir.join("torrent-state"));
    engine_config.fastresume = true;
    engine_config.dht_mode = DhtMode::Disabled;
    engine_config.listen_port_range = Some(available_port_range());
    engine_config.enable_upnp_port_forwarding = true;
    let engine = Arc::new(TorrentEngine::new(engine_config).await?);
    let store = Store::open(config.data_dir.join("seeder.sqlite3"))?;

    let mut targets = Vec::with_capacity(config.publishers.len());
    for publisher in config.publishers.iter().cloned() {
        let head_client = HeadClient::new(Dht::client()?.as_async());
        let reader = TorrentDatasetReader::new(
            publisher.clone(),
            head_client.clone(),
            PeerDiscovery::new(Dht::client()?.as_async()),
            engine.clone(),
        )
        .with_timeout(config.operation_timeout);
        targets.push(Target {
            publisher,
            head_client,
            reader,
        });
    }

    println!(
        "seeder-started publishers={} disk_limit_bytes={}",
        targets.len(),
        config.max_disk_bytes
    );
    let mut interval = tokio::time::interval(config.refresh_interval);
    loop {
        tokio::select! {
            _ = interval.tick() => {
                seed_cycle(&targets, &config, &store).await;
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    engine.shutdown().await;
    Ok(())
}

async fn seed_cycle(targets: &[Target], config: &Config, store: &Store) {
    let mut used = match directory_size(&config.data_dir) {
        Ok(size) => size,
        Err(error) => {
            eprintln!("seeder-disk-scan-error {error}");
            return;
        }
    };
    for target in targets {
        if used >= config.max_disk_bytes {
            eprintln!(
                "seeder-disk-limit used={used} limit={}",
                config.max_disk_bytes
            );
            break;
        }
        let highest_seen = match store.highest_authority_sequence(&target.publisher) {
            Ok(value) => value.and_then(|value| i64::try_from(value).ok()),
            Err(error) => {
                eprintln!(
                    "sequence-store-error publisher={} {error}",
                    target.publisher
                );
                continue;
            }
        };
        match target
            .head_client
            .resolve(target.publisher.public_key().as_bytes(), highest_seen)
            .await
        {
            Ok(Some(head)) => {
                let sequence = match u64::try_from(head.sequence()) {
                    Ok(sequence) => sequence,
                    Err(error) => {
                        eprintln!("head-sequence-error publisher={} {error}", target.publisher);
                        continue;
                    }
                };
                if let Err(error) = store.record_authority_sequence(&target.publisher, sequence) {
                    eprintln!(
                        "sequence-store-error publisher={} {error}",
                        target.publisher
                    );
                    continue;
                }
                if let Err(error) = target.head_client.reannounce(&head).await {
                    eprintln!(
                        "head-reannounce-error publisher={} {error}",
                        target.publisher
                    );
                }
            }
            Ok(None) => {
                println!("publisher-no-dataset {}", target.publisher);
                continue;
            }
            Err(error) => {
                eprintln!("head-resolve-error publisher={} {error}", target.publisher);
                continue;
            }
        }
        match target.reader.pin_current().await {
            Ok(Some(head)) => println!(
                "dataset-pinned publisher={} sequence={} manifest={}",
                target.publisher,
                head.authority_sequence(),
                head.manifest_digest()
            ),
            Ok(None) => println!("publisher-no-dataset {}", target.publisher),
            Err(error) => eprintln!("dataset-pin-error publisher={} {error}", target.publisher),
        }
        used = directory_size(&config.data_dir).unwrap_or(used);
    }
}

struct Config {
    publishers: Vec<PublisherId>,
    data_dir: PathBuf,
    max_disk_bytes: u64,
    refresh_interval: Duration,
    operation_timeout: Duration,
}

impl Config {
    fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let publishers = std::env::var("PUBKY_SWARM_PUBLISHERS")
            .map_err(|_| "PUBKY_SWARM_PUBLISHERS is required")?
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::parse)
            .collect::<Result<Vec<PublisherId>, _>>()?;
        if publishers.is_empty() {
            return Err("at least one publisher is required".into());
        }
        let max_publishers = env_usize("PUBKY_SWARM_MAX_PUBLISHERS", DEFAULT_MAX_PUBLISHERS)?;
        if publishers.len() > max_publishers {
            return Err(format!(
                "{} publishers exceeds configured maximum {max_publishers}",
                publishers.len()
            )
            .into());
        }
        Ok(Self {
            publishers,
            data_dir: PathBuf::from(
                std::env::var("PUBKY_SWARM_SEEDER_DATA")
                    .unwrap_or_else(|_| "data/seeder".to_owned()),
            ),
            max_disk_bytes: env_u64("PUBKY_SWARM_MAX_DISK_BYTES", DEFAULT_MAX_DISK_BYTES)?,
            refresh_interval: Duration::from_secs(env_u64("PUBKY_SWARM_REFRESH_SECONDS", 300)?),
            operation_timeout: Duration::from_secs(env_u64(
                "PUBKY_SWARM_OPERATION_TIMEOUT_SECONDS",
                120,
            )?),
        })
    }
}

fn env_u64(name: &str, default: u64) -> Result<u64, Box<dyn std::error::Error>> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse()?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    if value == 0 {
        return Err(format!("{name} must be positive").into());
    }
    Ok(value)
}

fn env_usize(name: &str, default: usize) -> Result<usize, Box<dyn std::error::Error>> {
    let value = match std::env::var(name) {
        Ok(value) => value.parse()?,
        Err(std::env::VarError::NotPresent) => default,
        Err(error) => return Err(error.into()),
    };
    if value == 0 {
        return Err(format!("{name} must be positive").into());
    }
    Ok(value)
}

fn directory_size(root: &Path) -> std::io::Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                directories.push(entry.path());
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    Ok(total)
}

fn available_port_range() -> Range<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("operating system must allocate a TCP port");
    let port = u32::from(listener.local_addr().expect("reserved address").port());
    drop(listener);
    let end = (port + 64).min(u32::from(u16::MAX));
    let range = if end > port {
        port..end
    } else {
        (port - 1)..port
    };
    u16::try_from(range.start).expect("valid start port")
        ..u16::try_from(range.end).expect("valid end port")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_size_counts_nested_regular_files() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("a"), vec![0; 10]).unwrap();
        std::fs::write(directory.path().join("nested/b"), vec![0; 20]).unwrap();
        assert_eq!(directory_size(directory.path()).unwrap(), 30);
    }
}
