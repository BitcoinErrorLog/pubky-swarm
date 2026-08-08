//! Machine-readable Pubky Swarm snapshot benchmark harness.

#![forbid(unsafe_code)]

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pubky::Keypair;
use serde::Serialize;
use swarm_protocol::{DatasetManifestV1, ManifestObjectV1, PublisherId};
use torrent_engine::{CreateOptions, create_torrent};

#[derive(Debug, Serialize)]
struct Report {
    schema: &'static str,
    version: u16,
    generated_at: u64,
    runs: Vec<Run>,
}

#[derive(Debug, Serialize)]
struct Run {
    object_count: usize,
    object_size: usize,
    dataset_bytes: u64,
    manifest_build_us: u128,
    manifest_bytes: usize,
    manifest_digest: String,
    torrent_create_ms: u128,
    torrent_metainfo_bytes: usize,
    torrent_piece_length: u32,
    mutation_manifest_us: u128,
    mutation_torrent_ms: u128,
    mutation_metainfo_bytes: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let counts = env_matrix("PUBKY_SWARM_BENCH_COUNTS", &[1, 10, 100], 100_000)?;
    let sizes = env_matrix(
        "PUBKY_SWARM_BENCH_SIZES",
        &[500, 2_048, 10_240],
        1024 * 1024,
    )?;
    let mut runs = Vec::new();
    for count in counts {
        for &size in &sizes {
            let total = count
                .checked_mul(size)
                .ok_or("benchmark dataset size overflow")?;
            if total > 2 * 1024 * 1024 * 1024_usize {
                return Err("one benchmark case exceeds the 2 GiB safety cap".into());
            }
            runs.push(run_case(count, size).await?);
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&Report {
            schema: "pubky.swarm/benchmark-report",
            version: 1,
            generated_at: unix_millis()?,
            runs,
        })?
    );
    Ok(())
}

async fn run_case(count: usize, size: usize) -> Result<Run, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let dataset = directory.path().join("dataset");
    std::fs::create_dir_all(dataset.join("objects"))?;
    let publisher = PublisherId::new(Keypair::from_secret(&[0x71; 32]).public_key());
    let mut entries = Vec::with_capacity(count + 1);
    for index in 0..count {
        let path = format!("objects/{index:08}.bin");
        let bytes = object_bytes(index, size);
        std::fs::write(dataset.join(&path), &bytes)?;
        entries.push(ManifestObjectV1::from_bytes(path, &bytes));
    }

    let started = Instant::now();
    let manifest = DatasetManifestV1::new(publisher.clone(), 1, entries.clone())?;
    let manifest_build_us = started.elapsed().as_micros();
    let canonical = manifest.to_canonical_bytes();
    std::fs::write(dataset.join("manifest.v1"), &canonical)?;

    let started = Instant::now();
    let torrent = create_torrent(
        &dataset,
        CreateOptions {
            name: Some(format!("dataset-{count}-{size}")),
            piece_length: Some(16 * 1024),
        },
    )
    .await?;
    let torrent_create_ms = started.elapsed().as_millis();

    let mutation_path = format!("objects/{count:08}.bin");
    let mutation_bytes = object_bytes(count, 1_024);
    std::fs::write(dataset.join(&mutation_path), &mutation_bytes)?;
    entries.push(ManifestObjectV1::from_bytes(mutation_path, &mutation_bytes));
    let started = Instant::now();
    let mutation_manifest = DatasetManifestV1::new(publisher, 2, entries)?;
    let mutation_manifest_us = started.elapsed().as_micros();
    std::fs::write(
        dataset.join("manifest.v1"),
        mutation_manifest.to_canonical_bytes(),
    )?;
    let started = Instant::now();
    let mutation_torrent = create_torrent(
        &dataset,
        CreateOptions {
            name: Some(format!("dataset-{count}-{size}-mutation")),
            piece_length: Some(16 * 1024),
        },
    )
    .await?;

    Ok(Run {
        object_count: count,
        object_size: size,
        dataset_bytes: u64::try_from(count * size)?,
        manifest_build_us,
        manifest_bytes: canonical.len(),
        manifest_digest: manifest.digest().to_string(),
        torrent_create_ms,
        torrent_metainfo_bytes: torrent.metainfo_bytes().len(),
        torrent_piece_length: torrent.piece_length(),
        mutation_manifest_us,
        mutation_torrent_ms: started.elapsed().as_millis(),
        mutation_metainfo_bytes: mutation_torrent.metainfo_bytes().len(),
    })
}

fn object_bytes(index: usize, size: usize) -> Vec<u8> {
    (0..size)
        .map(|offset| {
            u8::try_from((index.wrapping_mul(31).wrapping_add(offset)) % 251)
                .expect("modulo 251 fits in u8")
        })
        .collect()
}

fn env_matrix(
    name: &str,
    defaults: &[usize],
    maximum: usize,
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let values = match std::env::var(name) {
        Ok(value) => value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::parse)
            .collect::<Result<Vec<usize>, _>>()?,
        Err(std::env::VarError::NotPresent) => defaults.to_vec(),
        Err(error) => return Err(error.into()),
    };
    if values.is_empty() || values.iter().any(|value| *value == 0 || *value > maximum) {
        return Err(format!("{name} values must be in 1..={maximum}").into());
    }
    Ok(values)
}

fn unix_millis() -> Result<u64, Box<dyn std::error::Error>> {
    Ok(u64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_object_fixture() {
        assert_eq!(object_bytes(1, 16), object_bytes(1, 16));
        assert_ne!(object_bytes(1, 16), object_bytes(2, 16));
    }

    #[tokio::test]
    async fn tiny_case_produces_real_manifest_and_torrents() {
        let run = run_case(2, 128).await.unwrap();
        assert_eq!(run.object_count, 2);
        assert!(run.manifest_bytes > 0);
        assert!(run.torrent_metainfo_bytes > 0);
        assert!(run.mutation_metainfo_bytes > run.torrent_metainfo_bytes);
    }
}
