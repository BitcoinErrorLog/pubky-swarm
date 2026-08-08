//! Pubky Homeserver-backed authenticated dataset baseline.

#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use dataset_core::{
    ChangeWatcher, DatasetHead, DatasetPublisher, DatasetReader, Error, Provenance, Publication,
    Result, Snapshot,
};
use pubky::{EventCursor, PubkySession, PublicKey};
use pubky_adapter::PubkyAdapter;
use serde::{Deserialize, Serialize};
use swarm_protocol::{DatasetManifestV1, ManifestDigest, ManifestObjectV1, PublisherId};
use tokio::sync::Mutex;

/// Mutable Homeserver pointer to the current immutable snapshot.
pub const HEAD_PATH: &str = "/pub/pubky.swarm/v1/dataset-head.json";
/// Immutable snapshot namespace.
pub const SNAPSHOTS_PATH: &str = "/pub/pubky.swarm/v1/datasets/";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HeadWire {
    schema: String,
    version: u16,
    publisher: PublisherId,
    authority_sequence: u64,
    created_at: u64,
    manifest_digest: ManifestDigest,
}

impl HeadWire {
    fn from_head(head: &DatasetHead) -> Self {
        Self {
            schema: "pubky.swarm/dataset-head".to_owned(),
            version: 1,
            publisher: head.publisher().clone(),
            authority_sequence: head.authority_sequence(),
            created_at: head.created_at(),
            manifest_digest: head.manifest_digest(),
        }
    }

    fn into_head(self) -> Result<DatasetHead> {
        if self.schema != "pubky.swarm/dataset-head" || self.version != 1 {
            return Err(Error::Transport(
                "unsupported Homeserver dataset head schema".to_owned(),
            ));
        }
        DatasetHead::new(
            Provenance::new(self.publisher, self.created_at, self.manifest_digest),
            self.authority_sequence,
        )
    }
}

/// Verified read adapter for one Pubky publisher.
#[derive(Debug, Clone)]
pub struct HomeserverReader {
    adapter: PubkyAdapter,
    publisher: PublicKey,
}

impl HomeserverReader {
    /// Construct for a publisher.
    #[must_use]
    pub const fn new(adapter: PubkyAdapter, publisher: PublicKey) -> Self {
        Self { adapter, publisher }
    }

    async fn load_head(&self) -> Result<Option<DatasetHead>> {
        let wire: HeadWire = match self
            .adapter
            .get_public_json(&self.publisher, HEAD_PATH)
            .await
        {
            Ok(wire) => wire,
            Err(error) if is_not_found(&error) => return Ok(None),
            Err(error) => return Err(transport(error)),
        };
        let head = wire.into_head()?;
        if head.publisher().to_bytes() != self.publisher.to_bytes() {
            return Err(Error::Transport(
                "Homeserver head publisher does not match requested Pubky".to_owned(),
            ));
        }
        Ok(Some(head))
    }

    async fn load_manifest(&self, head: &DatasetHead) -> Result<DatasetManifestV1> {
        let path = manifest_path(head.manifest_digest());
        let bytes = self
            .adapter
            .get_public_bytes(&self.publisher, &path)
            .await
            .map_err(transport)?;
        let manifest = DatasetManifestV1::from_canonical_bytes(&bytes)?;
        if manifest.digest() != head.manifest_digest()
            || manifest.publisher() != head.publisher()
            || manifest.created_at() != head.created_at()
        {
            return Err(Error::Integrity {
                path,
                reason: "manifest does not match current head",
            });
        }
        Ok(manifest)
    }
}

#[async_trait]
impl DatasetReader for HomeserverReader {
    async fn head(&self) -> Result<Option<DatasetHead>> {
        self.load_head().await
    }

    async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let Some(head) = self.load_head().await? else {
            return Ok(None);
        };
        let manifest = self.load_manifest(&head).await?;
        if manifest.object(path).is_none() {
            return Ok(None);
        }
        let bytes = self
            .adapter
            .get_public_bytes(&self.publisher, &object_path(head.manifest_digest(), path))
            .await
            .map_err(transport)?;
        manifest.verify_object(path, &bytes)?;
        Ok(Some(bytes.to_vec()))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ManifestObjectV1>> {
        let Some(head) = self.load_head().await? else {
            return Ok(Vec::new());
        };
        let manifest = self.load_manifest(&head).await?;
        Ok(manifest
            .objects()
            .iter()
            .filter(|object| object.path.starts_with(prefix))
            .cloned()
            .collect())
    }
}

/// Crash-ordered Homeserver publisher.
///
/// Writes immutable objects and manifest first, then updates the mutable head.
/// A process-local lock serializes publication. Pubky 0.10 does not expose an
/// HTTP conditional-write API, so cross-process CAS cannot be guaranteed by
/// this control backend; root-authoritative torrent publication uses BEP 44
/// CAS instead.
#[derive(Debug)]
pub struct HomeserverPublisher {
    adapter: PubkyAdapter,
    session: PubkySession,
    identity: PublisherId,
    lock: Mutex<()>,
}

impl HomeserverPublisher {
    /// Construct from a grant-backed session.
    #[must_use]
    pub fn new(adapter: PubkyAdapter, session: PubkySession) -> Self {
        Self {
            identity: PublisherId::new(session.info().public_key().clone()),
            adapter,
            session,
            lock: Mutex::new(()),
        }
    }

    /// Matching read adapter.
    #[must_use]
    pub fn reader(&self) -> HomeserverReader {
        HomeserverReader::new(
            self.adapter.clone(),
            self.session.info().public_key().clone(),
        )
    }
}

#[async_trait]
impl DatasetPublisher for HomeserverPublisher {
    async fn publish(
        &self,
        publication: Publication,
        expected_previous: Option<ManifestDigest>,
    ) -> Result<Snapshot> {
        let _guard = self.lock.lock().await;
        let reader = self.reader();
        let current = reader.load_head().await?;
        let actual = current.as_ref().map(DatasetHead::manifest_digest);
        if actual != expected_previous {
            return Err(Error::ConcurrentUpdate {
                expected: expected_previous,
                actual,
            });
        }
        let next_sequence = current
            .as_ref()
            .map_or(1, DatasetHead::authority_sequence)
            .checked_add(u64::from(current.is_some()))
            .ok_or(Error::SequenceExhausted)?;
        let objects = publication.objects().to_vec();
        let manifest = DatasetManifestV1::new(
            self.identity.clone(),
            publication.created_at(),
            objects
                .iter()
                .map(|(path, bytes)| ManifestObjectV1::from_bytes(path.clone(), bytes))
                .collect(),
        )?;
        let snapshot = Snapshot::new(manifest.clone(), objects.clone())?;
        let digest = manifest.digest();

        for (path, bytes) in objects {
            self.adapter
                .put_bytes(&self.session, &object_path(digest, &path), bytes)
                .await
                .map_err(transport)?;
        }
        self.adapter
            .put_bytes(
                &self.session,
                &manifest_path(digest),
                manifest.to_canonical_bytes(),
            )
            .await
            .map_err(transport)?;
        let head = DatasetHead::new(snapshot.provenance(), next_sequence)?;
        self.adapter
            .put_json(&self.session, HEAD_PATH, &HeadWire::from_head(&head))
            .await
            .map_err(transport)?;
        Ok(snapshot)
    }
}

/// Conservative polling watcher for Homeserver heads.
#[derive(Debug, Clone)]
pub struct HomeserverWatcher {
    reader: HomeserverReader,
    interval: Duration,
}

impl HomeserverWatcher {
    /// Construct with a polling interval.
    #[must_use]
    pub const fn new(reader: HomeserverReader, interval: Duration) -> Self {
        Self { reader, interval }
    }
}

#[async_trait]
impl ChangeWatcher for HomeserverWatcher {
    async fn wait_for_change(&self, since: Option<&DatasetHead>) -> Result<DatasetHead> {
        loop {
            if let Some(head) = self.reader.load_head().await?
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

/// Create an event cursor for callers bridging Homeserver SSE into their own
/// watcher loop.
#[must_use]
pub fn event_cursor(value: u64) -> EventCursor {
    EventCursor::new(value)
}

fn manifest_path(digest: ManifestDigest) -> String {
    format!("{SNAPSHOTS_PATH}{digest}/manifest.bin")
}

fn object_path(digest: ManifestDigest, logical_path: &str) -> String {
    format!("{SNAPSHOTS_PATH}{digest}/objects/{logical_path}")
}

fn is_not_found(error: &pubky::Error) -> bool {
    matches!(
        error,
        pubky::Error::Request(pubky::errors::RequestError::Server { status, .. })
            if status.as_u16() == 404
    )
}

fn transport(error: impl std::fmt::Display) -> Error {
    Error::Transport(error.to_string())
}

#[cfg(test)]
mod tests {
    use pubky::{ClientId, Keypair};
    use pubky_testnet::{EphemeralTestnet, pubky_homeserver::ConnectionString};

    use super::*;

    fn postgres_connection() -> ConnectionString {
        let value = std::env::var("TEST_PUBKY_CONNECTION_STRING").unwrap_or_else(|_| {
            let user = std::env::var("USER").expect("USER must identify the PostgreSQL role");
            format!("postgres://{user}@127.0.0.1:5432/postgres?pubky-test=true")
        });
        ConnectionString::new(&value).expect("valid PostgreSQL connection string")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[pubky_testnet::test]
    async fn real_homeserver_snapshot_publication_and_verified_read() {
        let testnet = EphemeralTestnet::builder()
            .postgres(postgres_connection())
            .build()
            .await
            .unwrap();
        let sdk = testnet.sdk().unwrap();
        let signer = sdk.signer(Keypair::random());
        signer
            .signup(&testnet.homeserver_app().public_key(), None)
            .await
            .unwrap();
        let session = signer
            .signin_blocking(ClientId::new("pubky.swarm").unwrap())
            .await
            .unwrap();
        let publisher = HomeserverPublisher::new(PubkyAdapter::with_sdk(sdk.clone()), session);
        let reader = publisher.reader();
        let first = publisher
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
        assert_eq!(
            reader.head().await.unwrap().unwrap().authority_sequence(),
            1
        );
        assert_eq!(
            reader.get("profile.json").await.unwrap().unwrap(),
            b"{\"name\":\"Alice\"}"
        );
        assert_eq!(reader.list("releases/").await.unwrap().len(), 1);

        let second = publisher
            .publish(
                Publication::new(
                    500,
                    vec![(
                        "profile.json".to_owned(),
                        b"{\"name\":\"Alice 2\"}".to_vec(),
                    )],
                ),
                Some(first.digest()),
            )
            .await
            .unwrap();
        assert_eq!(
            reader.head().await.unwrap().unwrap().authority_sequence(),
            2
        );
        assert_eq!(
            reader.get("profile.json").await.unwrap().unwrap(),
            b"{\"name\":\"Alice 2\"}"
        );
        assert!(matches!(
            publisher
                .publish(Publication::new(2_000, Vec::new()), Some(first.digest()))
                .await,
            Err(Error::ConcurrentUpdate {
                actual: Some(actual),
                ..
            }) if actual == second.digest()
        ));
    }
}
