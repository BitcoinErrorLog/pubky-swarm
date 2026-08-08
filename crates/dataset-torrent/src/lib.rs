//! BEP 46-authorized `BitTorrent` dataset snapshots.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use dataset_core::{
    ChangeWatcher, DatasetHead, DatasetPublisher, DatasetReader, Error, Provenance, Publication,
    Result, Snapshot,
};
use mainline_discovery::PeerDiscovery;
use swarm_head::{HeadClient, HeadSigner, InfoHashV1, SignedHead};
use swarm_protocol::{DatasetManifestV1, ManifestDigest, ManifestObjectV1, PublisherId};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;
use torrent_engine::{AddOptions, CreateOptions, Torrent, TorrentEngine};

const INTERNAL_ROOT: &str = "__pubky_swarm__";
const MANIFEST_PATH: &str = "__pubky_swarm__/manifest.v1";
const OBJECTS_ROOT: &str = "objects";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OBJECT_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone)]
struct LoadedDataset {
    signed_head: SignedHead,
    head: DatasetHead,
    manifest: DatasetManifestV1,
    torrent: Torrent,
    file_indices: BTreeMap<String, usize>,
}

/// Torrent-backed reader for one publisher.
#[derive(Debug, Clone)]
pub struct TorrentDatasetReader {
    publisher: PublisherId,
    head_client: HeadClient,
    discovery: PeerDiscovery,
    engine: Arc<TorrentEngine>,
    load_timeout: Duration,
    loaded: Arc<Mutex<Option<LoadedDataset>>>,
}

impl TorrentDatasetReader {
    /// Construct with externally configured authority, discovery, and transfer
    /// clients.
    #[must_use]
    pub fn new(
        publisher: PublisherId,
        head_client: HeadClient,
        discovery: PeerDiscovery,
        engine: Arc<TorrentEngine>,
    ) -> Self {
        Self {
            publisher,
            head_client,
            discovery,
            engine,
            load_timeout: Duration::from_secs(60),
            loaded: Arc::new(Mutex::new(None)),
        }
    }

    /// Set metadata/object retrieval timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.load_timeout = timeout;
        self
    }

    async fn load(&self) -> Result<Option<LoadedDataset>> {
        let signed = self
            .head_client
            .resolve(self.publisher.public_key().as_bytes(), None)
            .await
            .map_err(transport)?;
        let Some(signed) = signed else {
            return Ok(None);
        };
        if signed.publisher() != self.publisher.public_key().as_bytes() {
            return Err(Error::Transport(
                "BEP 46 head publisher does not match requested Pubky".to_owned(),
            ));
        }
        {
            let loaded = self.loaded.lock().await;
            if let Some(current) = loaded.as_ref()
                && current.signed_head.sequence() == signed.sequence()
                && current.signed_head.info_hash() == signed.info_hash()
            {
                return Ok(Some(current.clone()));
            }
        }

        let peers = self
            .discovery
            .wait_for_peers(signed.info_hash(), self.load_timeout)
            .await
            .map_err(transport)?;
        let magnet = format!("magnet:?xt=urn:btih:{}", signed.info_hash());
        let torrent = tokio::time::timeout(
            self.load_timeout,
            self.engine.add_magnet(
                &magnet,
                AddOptions {
                    paused: true,
                    initial_peers: Some(peers),
                    disable_trackers: true,
                    ..AddOptions::default()
                },
            ),
        )
        .await
        .map_err(|_| Error::Transport("torrent metadata resolution timed out".to_owned()))?
        .map_err(transport)?;
        torrent.wait_until_initialized().await.map_err(transport)?;
        let metadata = torrent.metadata().map_err(transport)?;
        let file_indices: BTreeMap<String, usize> = metadata
            .files
            .iter()
            .map(|file| (file.path.to_string_lossy().replace('\\', "/"), file.index))
            .collect();
        let manifest_index =
            *file_indices
                .get(MANIFEST_PATH)
                .ok_or_else(|| Error::MissingObject {
                    path: MANIFEST_PATH.to_owned(),
                })?;
        torrent
            .update_only_files(&[manifest_index])
            .await
            .map_err(transport)?;
        torrent.resume().await.map_err(transport)?;
        tokio::time::timeout(self.load_timeout, torrent.wait_until_completed())
            .await
            .map_err(|_| Error::Transport("manifest retrieval timed out".to_owned()))?
            .map_err(transport)?;
        let manifest_bytes = read_file(&torrent, manifest_index, MAX_MANIFEST_BYTES).await?;
        let manifest = DatasetManifestV1::from_canonical_bytes(&manifest_bytes)?;
        if manifest.publisher() != &self.publisher {
            return Err(Error::Transport(
                "torrent manifest publisher does not match BEP 46 identity".to_owned(),
            ));
        }
        let sequence = u64::try_from(signed.sequence()).map_err(|_| Error::InvalidSequence(0))?;
        let head = DatasetHead::new(Provenance::from_manifest(&manifest), sequence)?;
        let loaded = LoadedDataset {
            signed_head: signed,
            head,
            manifest,
            torrent,
            file_indices,
        };
        *self.loaded.lock().await = Some(loaded.clone());
        Ok(Some(loaded))
    }

    /// Download every file in the current snapshot and announce this client as
    /// a seeder.
    ///
    /// # Errors
    ///
    /// Returns authority, discovery, transfer, or listener errors.
    pub async fn pin_current(&self) -> Result<Option<DatasetHead>> {
        let Some(loaded) = self.load().await? else {
            return Ok(None);
        };
        let all_indices: Vec<usize> = loaded.file_indices.values().copied().collect();
        loaded
            .torrent
            .update_only_files(&all_indices)
            .await
            .map_err(transport)?;
        loaded.torrent.resume().await.map_err(transport)?;
        tokio::time::timeout(self.load_timeout, loaded.torrent.wait_until_completed())
            .await
            .map_err(|_| Error::Transport("snapshot pin timed out".to_owned()))?
            .map_err(transport)?;
        let port = self
            .engine
            .listen_port()
            .filter(|port| *port != 0)
            .ok_or_else(|| Error::Transport("torrent engine has no listen port".to_owned()))?;
        self.discovery
            .announce(loaded.signed_head.info_hash(), port)
            .await
            .map_err(transport)?;
        Ok(Some(loaded.head))
    }
}

#[async_trait]
impl DatasetReader for TorrentDatasetReader {
    async fn head(&self) -> Result<Option<DatasetHead>> {
        Ok(self.load().await?.map(|loaded| loaded.head))
    }

    async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(loaded) = self.load().await? else {
            return Ok(None);
        };
        let Some(object) = loaded.manifest.object(path) else {
            return Ok(None);
        };
        if object.size > MAX_OBJECT_BYTES {
            return Err(Error::Transport(format!(
                "object {path:?} exceeds in-memory read limit"
            )));
        }
        let torrent_path = format!("{OBJECTS_ROOT}/{path}");
        let index =
            *loaded
                .file_indices
                .get(&torrent_path)
                .ok_or_else(|| Error::MissingObject {
                    path: torrent_path.clone(),
                })?;
        let manifest_index =
            *loaded
                .file_indices
                .get(MANIFEST_PATH)
                .ok_or_else(|| Error::MissingObject {
                    path: MANIFEST_PATH.to_owned(),
                })?;
        loaded
            .torrent
            .update_only_files(&[manifest_index, index])
            .await
            .map_err(transport)?;
        loaded.torrent.resume().await.map_err(transport)?;
        tokio::time::timeout(self.load_timeout, loaded.torrent.wait_until_completed())
            .await
            .map_err(|_| Error::Transport(format!("object {path:?} retrieval timed out")))?
            .map_err(transport)?;
        let bytes = read_file(&loaded.torrent, index, MAX_OBJECT_BYTES).await?;
        loaded.manifest.verify_object(path, &bytes)?;
        Ok(Some(bytes))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ManifestObjectV1>> {
        let Some(loaded) = self.load().await? else {
            return Ok(Vec::new());
        };
        Ok(loaded
            .manifest
            .objects()
            .iter()
            .filter(|object| object.path.starts_with(prefix))
            .cloned()
            .collect())
    }
}

#[derive(Debug, Clone, Copy)]
struct PublishedState {
    manifest_digest: ManifestDigest,
    authority_sequence: i64,
}

/// Naive snapshot-per-publish torrent backend for an isolated lab identity.
#[derive(Debug)]
pub struct TorrentDatasetPublisher {
    signer: HeadSigner,
    head_client: HeadClient,
    discovery: PeerDiscovery,
    engine: Arc<TorrentEngine>,
    snapshots_dir: PathBuf,
    state: Mutex<Option<PublishedState>>,
}

impl TorrentDatasetPublisher {
    /// Construct a first-publication publisher.
    #[must_use]
    pub fn new(
        signer: HeadSigner,
        head_client: HeadClient,
        discovery: PeerDiscovery,
        engine: Arc<TorrentEngine>,
        snapshots_dir: PathBuf,
    ) -> Self {
        Self {
            signer,
            head_client,
            discovery,
            engine,
            snapshots_dir,
            state: Mutex::new(None),
        }
    }

    /// Restore known local publication state after validating it externally.
    #[must_use]
    pub fn with_current(
        mut self,
        manifest_digest: ManifestDigest,
        authority_sequence: i64,
    ) -> Self {
        self.state = Mutex::new(Some(PublishedState {
            manifest_digest,
            authority_sequence,
        }));
        self
    }
}

#[async_trait]
impl DatasetPublisher for TorrentDatasetPublisher {
    async fn publish(
        &self,
        publication: Publication,
        expected_previous: Option<ManifestDigest>,
    ) -> Result<Snapshot> {
        if publication.objects().iter().any(|(path, _)| {
            path == INTERNAL_ROOT || path.starts_with(&format!("{INTERNAL_ROOT}/"))
        }) {
            return Err(Error::UnexpectedObject {
                path: INTERNAL_ROOT.to_owned(),
            });
        }
        let mut state = self.state.lock().await;
        let network_head = self
            .head_client
            .resolve(&self.signer.public_key(), None)
            .await
            .map_err(transport)?;
        let actual = state.map(|current| current.manifest_digest);
        if actual != expected_previous {
            return Err(Error::ConcurrentUpdate {
                expected: expected_previous,
                actual,
            });
        }
        match (*state, network_head.as_ref()) {
            (None, Some(_)) => {
                return Err(Error::Transport(
                    "existing BEP 46 head requires restored publisher state".to_owned(),
                ));
            }
            (Some(local), Some(network)) if local.authority_sequence != network.sequence() => {
                return Err(Error::Transport(
                    "local publisher sequence differs from BEP 46 head".to_owned(),
                ));
            }
            (Some(_), None) => {
                return Err(Error::Transport(
                    "local publisher state exists but BEP 46 head is unavailable".to_owned(),
                ));
            }
            _ => {}
        }

        let objects = publication.objects().to_vec();
        let identity = PublisherId::from_bytes(self.signer.public_key())?;
        let manifest = DatasetManifestV1::new(
            identity,
            publication.created_at(),
            objects
                .iter()
                .map(|(path, bytes)| ManifestObjectV1::from_bytes(path.clone(), bytes))
                .collect(),
        )?;
        let snapshot = Snapshot::new(manifest.clone(), objects.clone())?;
        let digest = manifest.digest();
        let snapshot_dir = self.snapshots_dir.join(digest.to_string());
        stage_snapshot(&snapshot_dir, &manifest, &objects).await?;

        let created = torrent_engine::create_torrent(
            &snapshot_dir,
            CreateOptions {
                name: Some(digest.to_string()),
                piece_length: Some(16 * 1024),
            },
        )
        .await
        .map_err(transport)?;
        let torrent = self
            .engine
            .add_metainfo(
                created.metainfo_bytes(),
                AddOptions {
                    output_dir: Some(snapshot_dir.clone()),
                    overwrite: true,
                    disable_trackers: true,
                    ..AddOptions::default()
                },
            )
            .await
            .map_err(transport)?;
        torrent.wait_until_completed().await.map_err(transport)?;
        let info_hash: InfoHashV1 = created.info_hash_hex().parse()?;
        let port = self
            .engine
            .listen_port()
            .filter(|port| *port != 0)
            .ok_or_else(|| Error::Transport("torrent engine has no listen port".to_owned()))?;
        self.discovery
            .announce(info_hash, port)
            .await
            .map_err(transport)?;

        let expected_sequence = network_head.as_ref().map(SignedHead::sequence);
        let signed = self
            .head_client
            .publish_next(&self.signer, info_hash, expected_sequence)
            .await
            .map_err(transport)?;
        *state = Some(PublishedState {
            manifest_digest: digest,
            authority_sequence: signed.sequence(),
        });
        Ok(snapshot)
    }
}

/// Conservative BEP 46 polling watcher.
#[derive(Debug, Clone)]
pub struct TorrentDatasetWatcher {
    reader: TorrentDatasetReader,
    interval: Duration,
}

impl TorrentDatasetWatcher {
    /// Construct with polling interval.
    #[must_use]
    pub const fn new(reader: TorrentDatasetReader, interval: Duration) -> Self {
        Self { reader, interval }
    }
}

#[async_trait]
impl ChangeWatcher for TorrentDatasetWatcher {
    async fn wait_for_change(&self, since: Option<&DatasetHead>) -> Result<DatasetHead> {
        loop {
            if let Some(head) = self.reader.head().await?
                && since.is_none_or(|previous| {
                    previous.authority_sequence() != head.authority_sequence()
                        || previous.manifest_digest() != head.manifest_digest()
                })
            {
                return Ok(head);
            }
            tokio::time::sleep(self.interval).await;
        }
    }
}

async fn stage_snapshot(
    final_dir: &Path,
    manifest: &DatasetManifestV1,
    objects: &[(String, Vec<u8>)],
) -> Result<()> {
    if final_dir.exists() {
        return Ok(());
    }
    let parent = final_dir
        .parent()
        .ok_or_else(|| Error::Transport("snapshot directory has no parent".to_owned()))?;
    tokio::fs::create_dir_all(parent).await.map_err(transport)?;
    let staging = parent.join(format!(
        ".{}.staging-{}",
        manifest.digest(),
        std::process::id()
    ));
    if staging.exists() {
        tokio::fs::remove_dir_all(&staging)
            .await
            .map_err(transport)?;
    }
    tokio::fs::create_dir_all(staging.join(INTERNAL_ROOT))
        .await
        .map_err(transport)?;
    tokio::fs::write(staging.join(MANIFEST_PATH), manifest.to_canonical_bytes())
        .await
        .map_err(transport)?;
    for (path, bytes) in objects {
        let destination = staging.join(OBJECTS_ROOT).join(path);
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(transport)?;
        }
        tokio::fs::write(destination, bytes)
            .await
            .map_err(transport)?;
    }
    match tokio::fs::rename(&staging, final_dir).await {
        Ok(()) => Ok(()),
        Err(error) if final_dir.exists() => {
            tokio::fs::remove_dir_all(staging).await.map_err(transport)
        }
        Err(error) => Err(transport(error)),
    }
}

async fn read_file(torrent: &Torrent, index: usize, maximum: u64) -> Result<Vec<u8>> {
    let metadata = torrent.metadata().map_err(transport)?;
    let file = metadata
        .files
        .get(index)
        .ok_or_else(|| Error::MissingObject {
            path: format!("torrent file index {index}"),
        })?;
    if file.length > maximum {
        return Err(Error::Transport(format!(
            "torrent file {} exceeds {maximum} byte read limit",
            file.path.display()
        )));
    }
    let capacity = usize::try_from(file.length)
        .map_err(|_| Error::Transport("torrent file length exceeds usize".to_owned()))?;
    let mut bytes = Vec::with_capacity(capacity);
    torrent
        .stream_file(index)
        .map_err(transport)?
        .read_to_end(&mut bytes)
        .await
        .map_err(transport)?;
    Ok(bytes)
}

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::ops::Range;

    use mainline::{Dht, Testnet};
    use torrent_engine::{DhtMode, EngineConfig};

    use super::*;

    fn port_range() -> Range<u16> {
        let listener = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = u32::from(listener.local_addr().unwrap().port());
        drop(listener);
        let end = (port + 20).min(u32::from(u16::MAX));
        let range = if end > port {
            port..end
        } else {
            (port - 1)..port
        };
        u16::try_from(range.start).unwrap()..u16::try_from(range.end).unwrap()
    }

    async fn engine(path: &Path) -> Arc<TorrentEngine> {
        let mut config = EngineConfig::new(path);
        config.dht_mode = DhtMode::Disabled;
        config.listen_port_range = Some(port_range());
        Arc::new(TorrentEngine::new(config).await.unwrap())
    }

    fn head_client(bootstrap: &[String]) -> HeadClient {
        HeadClient::new(
            Dht::builder()
                .bootstrap(bootstrap)
                .bind_address(Ipv4Addr::LOCALHOST)
                .build()
                .unwrap()
                .as_async(),
        )
    }

    fn discovery(bootstrap: &[String]) -> PeerDiscovery {
        PeerDiscovery::new(
            Dht::builder()
                .bootstrap(bootstrap)
                .bind_address(Ipv4Addr::LOCALHOST)
                .build()
                .unwrap()
                .as_async(),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 6)]
    async fn fresh_reader_survives_publisher_shutdown_via_seeder() {
        let directory = tempfile::tempdir().unwrap();
        let testnet = Testnet::builder(8).build().unwrap();
        let signer = HeadSigner::from_seed([0x61; 32]);
        let publisher_engine = engine(&directory.path().join("publisher-downloads")).await;
        let publisher = TorrentDatasetPublisher::new(
            signer.clone(),
            head_client(&testnet.bootstrap),
            discovery(&testnet.bootstrap),
            publisher_engine.clone(),
            directory.path().join("published-snapshots"),
        );
        let snapshot = publisher
            .publish(
                Publication::new(
                    1_000,
                    vec![
                        ("profile.json".to_owned(), b"{\"name\":\"Alice\"}".to_vec()),
                        ("releases/a.json".to_owned(), b"{\"title\":\"A\"}".to_vec()),
                    ],
                ),
                None,
            )
            .await
            .unwrap();

        let identity = PublisherId::from_bytes(signer.public_key()).unwrap();
        let seeder_engine = engine(&directory.path().join("seeder-downloads")).await;
        let seeder = TorrentDatasetReader::new(
            identity.clone(),
            head_client(&testnet.bootstrap),
            discovery(&testnet.bootstrap),
            seeder_engine.clone(),
        )
        .with_timeout(Duration::from_secs(20));
        assert_eq!(
            seeder.get("profile.json").await.unwrap().unwrap(),
            b"{\"name\":\"Alice\"}"
        );
        assert_eq!(
            seeder
                .pin_current()
                .await
                .unwrap()
                .unwrap()
                .manifest_digest(),
            snapshot.digest()
        );
        publisher_engine.shutdown().await;

        let fresh_engine = engine(&directory.path().join("fresh-downloads")).await;
        let fresh = TorrentDatasetReader::new(
            identity,
            head_client(&testnet.bootstrap),
            discovery(&testnet.bootstrap),
            fresh_engine.clone(),
        )
        .with_timeout(Duration::from_secs(20));
        assert_eq!(
            fresh.get("releases/a.json").await.unwrap().unwrap(),
            b"{\"title\":\"A\"}"
        );
        assert_eq!(fresh.list("").await.unwrap().len(), 2);

        fresh_engine.shutdown().await;
        seeder_engine.shutdown().await;
    }
}
