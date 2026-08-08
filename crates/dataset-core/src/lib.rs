//! Transport-neutral authenticated dataset core.
//!
//! A dataset is a sequence of validated [`Snapshot`]s described by
//! [`DatasetManifestV1`] values from `swarm-protocol`. Authority over a
//! dataset comes from the Pubky publisher identity recorded in each manifest;
//! how the current manifest is resolved and how bytes move is entirely a
//! transport concern behind the [`DatasetReader`], [`DatasetPublisher`], and
//! [`ChangeWatcher`] traits.
//!
//! [`MemoryDataset`] is a complete in-memory reference implementation of
//! those traits: it validates every object against the manifest, commits
//! crash-safely, and enforces compare-and-swap and monotonic publication.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use swarm_protocol::{DatasetManifestV1, ManifestDigest, ManifestObjectV1, PublisherId};
use tokio::sync::watch;

/// Dataset core failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Protocol validation failed while building or checking a manifest.
    #[error(transparent)]
    Protocol(#[from] swarm_protocol::Error),
    /// Object bytes did not match the manifest declaration.
    #[error("object {path:?} failed integrity verification: {reason}")]
    Integrity {
        /// Mismatched path.
        path: String,
        /// Specific failed constraint.
        reason: &'static str,
    },
    /// A snapshot was built without a manifest-declared object.
    #[error("snapshot is missing declared object {path:?}")]
    MissingObject {
        /// Absent path.
        path: String,
    },
    /// A snapshot contained an object the manifest does not declare.
    #[error("snapshot contains undeclared object {path:?}")]
    UnexpectedObject {
        /// Extra path.
        path: String,
    },
    /// The same object path was supplied twice for one snapshot.
    #[error("object {path:?} was supplied more than once")]
    DuplicateObject {
        /// Duplicated path.
        path: String,
    },
    /// An authority sequence was not a positive integer.
    #[error("invalid authority sequence {0}: expected a positive integer")]
    InvalidSequence(u64),
    /// Incrementing the current authority sequence would overflow.
    #[error("authority sequence exhausted")]
    SequenceExhausted,
    /// A resolved or published head is older than already accepted state.
    #[error("rollback detected: highest accepted sequence {highest_seen}, resolved {resolved}")]
    Rollback {
        /// Highest accepted authority sequence.
        highest_seen: u64,
        /// Rejected authority sequence.
        resolved: u64,
    },
    /// Two different manifests claim the same publisher and authority sequence.
    #[error("conflicting heads at authority sequence {authority_sequence}")]
    ConflictingHead {
        /// Shared authority sequence.
        authority_sequence: u64,
    },
    /// A head belongs to a different publisher than the tracked dataset.
    #[error("head publisher {head} does not match tracked publisher {tracked}")]
    PublisherMismatch {
        /// Publisher of the tracked dataset.
        tracked: Box<PublisherId>,
        /// Publisher of the compared head.
        head: Box<PublisherId>,
    },
    /// The expected previous head did not match the current head at commit.
    #[error("concurrent publication: expected {expected:?}, found {actual:?}")]
    ConcurrentUpdate {
        /// Head digest the caller expected, or `None` for first publication.
        expected: Option<ManifestDigest>,
        /// Head digest current at commit time.
        actual: Option<ManifestDigest>,
    },
    /// The change notification source was closed while waiting.
    #[error("change notification source closed")]
    WatchClosed,
    /// The underlying transport failed.
    #[error("transport failure: {0}")]
    Transport(String),
    /// The in-memory state lock was poisoned.
    #[error("dataset state lock poisoned")]
    LockPoisoned,
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Typed provenance of a dataset head or snapshot.
///
/// Provenance answers "who published this exact state, and when". It is the
/// unit compared by [`FreshnessTracker`] and returned by head resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    publisher: PublisherId,
    created_at: u64,
    manifest_digest: ManifestDigest,
}

impl Provenance {
    /// Construct from explicit parts.
    #[must_use]
    pub const fn new(
        publisher: PublisherId,
        created_at: u64,
        manifest_digest: ManifestDigest,
    ) -> Self {
        Self {
            publisher,
            created_at,
            manifest_digest,
        }
    }

    /// Derive provenance from a validated manifest.
    #[must_use]
    pub fn from_manifest(manifest: &DatasetManifestV1) -> Self {
        Self::new(
            manifest.publisher().clone(),
            manifest.created_at(),
            manifest.digest(),
        )
    }

    /// Publisher identity owning the dataset.
    #[must_use]
    pub const fn publisher(&self) -> &PublisherId {
        &self.publisher
    }

    /// Manifest creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Digest of the manifest's canonical bytes.
    #[must_use]
    pub const fn manifest_digest(&self) -> ManifestDigest {
        self.manifest_digest
    }
}

/// A resolved dataset head: typed provenance plus its authority sequence.
///
/// The authority sequence is the positive, strictly monotonic ordering
/// assigned by the publisher's authority mechanism. Transports map it from
/// their signed primitive: a BEP 44/BEP 46 adapter uses the signed mutable
/// item's `seq` directly, so the sequence is authenticated by the publisher's
/// Ed25519 signature rather than by any wall-clock timestamp. Manifest
/// `created_at` remains skewable metadata and never orders heads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatasetHead {
    provenance: Provenance,
    authority_sequence: u64,
}

impl DatasetHead {
    /// Construct a head, rejecting a non-positive authority sequence.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSequence`] unless `authority_sequence` is
    /// positive.
    pub const fn new(provenance: Provenance, authority_sequence: u64) -> Result<Self> {
        if authority_sequence == 0 {
            return Err(Error::InvalidSequence(0));
        }
        Ok(Self {
            provenance,
            authority_sequence,
        })
    }

    /// Typed provenance of the manifest this head points at.
    #[must_use]
    pub const fn provenance(&self) -> &Provenance {
        &self.provenance
    }

    /// Positive monotonic authority sequence ordering this head.
    #[must_use]
    pub const fn authority_sequence(&self) -> u64 {
        self.authority_sequence
    }

    /// Publisher identity owning the dataset.
    #[must_use]
    pub const fn publisher(&self) -> &PublisherId {
        self.provenance.publisher()
    }

    /// Manifest creation time in Unix milliseconds. Metadata only; never
    /// used to order heads.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.provenance.created_at()
    }

    /// Digest of the manifest's canonical bytes.
    #[must_use]
    pub const fn manifest_digest(&self) -> ManifestDigest {
        self.provenance.manifest_digest()
    }
}

/// Freshness of a resolved head relative to the highest accepted head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Freshness {
    /// First head observed for this dataset; nothing to compare against.
    Initial,
    /// Same creation time and manifest digest as the highest accepted head.
    Unchanged,
    /// Strictly newer than the highest accepted head.
    Advanced,
}

/// Client-side freshness and rollback guard ordered by authority sequence.
///
/// Tracks the highest accepted head and classifies newly resolved heads
/// purely by publisher and authority sequence: a lower sequence is a
/// rollback, the same sequence with a different manifest digest is a
/// conflict, the same sequence and digest is unchanged, and a higher
/// sequence is advanced. Wall-clock `created_at` never participates.
///
/// Like BEP 44 highest-seen persistence, this cannot prevent first-contact
/// rollback: callers must persist the tracker and seed it on restart.
#[derive(Debug, Clone, Default)]
pub struct FreshnessTracker {
    highest: Option<DatasetHead>,
}

impl FreshnessTracker {
    /// Start tracking a dataset with no accepted head.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Resume tracking from a persisted highest accepted head.
    #[must_use]
    pub const fn from_highest(highest: Option<DatasetHead>) -> Self {
        Self { highest }
    }

    /// Highest accepted head, if any.
    #[must_use]
    pub const fn highest_seen(&self) -> Option<&DatasetHead> {
        self.highest.as_ref()
    }

    /// Classify a resolved head without recording it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::PublisherMismatch`] for a foreign publisher,
    /// [`Error::Rollback`] for a lower authority sequence, and
    /// [`Error::ConflictingHead`] for a different manifest digest at the
    /// highest accepted sequence.
    pub fn check(&self, head: &DatasetHead) -> Result<Freshness> {
        let Some(highest) = &self.highest else {
            return Ok(Freshness::Initial);
        };
        if highest.publisher() != head.publisher() {
            return Err(Error::PublisherMismatch {
                tracked: Box::new(highest.publisher().clone()),
                head: Box::new(head.publisher().clone()),
            });
        }
        if head.authority_sequence() > highest.authority_sequence() {
            return Ok(Freshness::Advanced);
        }
        if head.authority_sequence() < highest.authority_sequence() {
            return Err(Error::Rollback {
                highest_seen: highest.authority_sequence(),
                resolved: head.authority_sequence(),
            });
        }
        if head.manifest_digest() == highest.manifest_digest() {
            return Ok(Freshness::Unchanged);
        }
        Err(Error::ConflictingHead {
            authority_sequence: head.authority_sequence(),
        })
    }

    /// Classify a resolved head and record it as the highest accepted head.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::check`] and records nothing.
    pub fn accept(&mut self, head: &DatasetHead) -> Result<Freshness> {
        let freshness = self.check(head)?;
        self.highest = Some(head.clone());
        Ok(freshness)
    }
}

/// Immutable, fully validated dataset state.
///
/// Construction verifies every manifest-declared object against the supplied
/// bytes (presence, length, and BLAKE3 digest) and rejects undeclared or
/// duplicated objects, so a `Snapshot` always authenticates its contents.
#[derive(Debug, Clone)]
pub struct Snapshot {
    manifest: DatasetManifestV1,
    objects: BTreeMap<String, Vec<u8>>,
}

impl Snapshot {
    /// Build and validate a snapshot from a manifest and object bytes.
    ///
    /// `objects` may be in any order; duplicate paths are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`Error::MissingObject`], [`Error::UnexpectedObject`],
    /// [`Error::DuplicateObject`], or [`Error::Integrity`].
    pub fn new(manifest: DatasetManifestV1, objects: Vec<(String, Vec<u8>)>) -> Result<Self> {
        let mut map = BTreeMap::new();
        for (path, bytes) in objects {
            if map.insert(path.clone(), bytes).is_some() {
                return Err(Error::DuplicateObject { path });
            }
        }
        for object in manifest.objects() {
            let bytes = map.get(&object.path).ok_or_else(|| Error::MissingObject {
                path: object.path.clone(),
            })?;
            if object.size != bytes.len() as u64 {
                return Err(Error::Integrity {
                    path: object.path.clone(),
                    reason: "byte length does not match the manifest",
                });
            }
            if !object.digest.verify(bytes) {
                return Err(Error::Integrity {
                    path: object.path.clone(),
                    reason: "BLAKE3 digest does not match the manifest",
                });
            }
        }
        for path in map.keys() {
            if manifest.object(path).is_none() {
                return Err(Error::UnexpectedObject { path: path.clone() });
            }
        }
        Ok(Self {
            manifest,
            objects: map,
        })
    }

    /// Validated manifest describing this state.
    #[must_use]
    pub const fn manifest(&self) -> &DatasetManifestV1 {
        &self.manifest
    }

    /// Manifest digest identifying this state.
    #[must_use]
    pub fn digest(&self) -> ManifestDigest {
        self.manifest.digest()
    }

    /// Typed provenance of this state.
    #[must_use]
    pub fn provenance(&self) -> Provenance {
        Provenance::from_manifest(&self.manifest)
    }

    /// Number of logical objects.
    #[must_use]
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    /// Whether the dataset declares no objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// Verified object bytes by exact path.
    #[must_use]
    pub fn get(&self, path: &str) -> Option<&[u8]> {
        self.objects.get(path).map(Vec::as_slice)
    }

    /// Manifest entries whose path starts with `prefix`, in sorted order.
    ///
    /// The prefix is a literal string prefix: `"docs"` also matches
    /// `"docs-old/a"`. Use a trailing `/` for directory semantics. An empty
    /// prefix lists every object.
    #[must_use]
    pub fn list(&self, prefix: &str) -> Vec<&ManifestObjectV1> {
        self.manifest
            .objects()
            .iter()
            .filter(|object| object.path.starts_with(prefix))
            .collect()
    }
}

/// A proposed dataset publication: new object bytes plus creation time.
///
/// Validation happens in [`DatasetPublisher::publish`]; this type only
/// carries the intent.
#[derive(Debug, Clone, Default)]
pub struct Publication {
    created_at: u64,
    objects: Vec<(String, Vec<u8>)>,
}

impl Publication {
    /// Create a publication for `created_at` (Unix milliseconds) with the
    /// given `(path, bytes)` objects in any order.
    #[must_use]
    pub const fn new(created_at: u64, objects: Vec<(String, Vec<u8>)>) -> Self {
        Self {
            created_at,
            objects,
        }
    }

    /// Add or stage one object. Duplicate paths are rejected at publish time.
    pub fn insert(&mut self, path: String, bytes: Vec<u8>) {
        self.objects.push((path, bytes));
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Staged `(path, bytes)` objects.
    #[must_use]
    pub fn objects(&self) -> &[(String, Vec<u8>)] {
        &self.objects
    }
}

/// Application-facing read access to a publisher's current dataset.
///
/// Implementations resolve authority (which manifest is current) from their
/// transport, but must only return bytes verified against that manifest.
#[async_trait]
pub trait DatasetReader {
    /// Resolve the current head (provenance plus authority sequence), or
    /// `None` if the publisher has never published.
    ///
    /// # Errors
    ///
    /// Returns transport or validation errors.
    async fn head(&self) -> Result<Option<DatasetHead>>;

    /// Verified bytes of one object at the current head, or `None` when the
    /// object or dataset does not exist.
    ///
    /// Implementations must verify returned bytes against the current
    /// manifest's declared length and BLAKE3 digest.
    ///
    /// # Errors
    ///
    /// Returns transport, validation, or integrity errors.
    async fn get(&self, path: &str) -> Result<Option<Vec<u8>>>;

    /// Manifest entries at the current head whose path starts with `prefix`,
    /// in sorted order. See [`Snapshot::list`] for prefix semantics.
    ///
    /// # Errors
    ///
    /// Returns transport or validation errors.
    async fn list(&self, prefix: &str) -> Result<Vec<ManifestObjectV1>>;
}

/// Application-facing crash-safe publication of new dataset states.
///
/// Implementations must stage and fully validate the new manifest and object
/// bytes before an atomic commit: any failure before commit must leave the
/// previous head untouched, and a crash during commit must leave either the
/// complete previous state or the complete new state, never a mixture.
#[async_trait]
pub trait DatasetPublisher {
    /// Validate and atomically publish `publication` as the new head.
    ///
    /// `expected_previous` is a compare-and-swap guard: pass the digest of
    /// the head this publication is based on, or `None` only when no head
    /// exists. Implementations must reject a mismatch with
    /// [`Error::ConcurrentUpdate`].
    ///
    /// # Errors
    ///
    /// Returns validation, integrity, concurrency, rollback, or transport
    /// errors.
    async fn publish(
        &self,
        publication: Publication,
        expected_previous: Option<ManifestDigest>,
    ) -> Result<Snapshot>;
}

/// Best-effort change notification for a dataset head.
///
/// This abstraction deliberately does not promise push semantics:
/// notifications may be delayed, coalesced, duplicated, or missed entirely,
/// depending on the transport. A returned notification only means "the head
/// may have changed"; consumers must re-resolve and re-validate the head
/// through [`DatasetReader`] before trusting anything.
#[async_trait]
pub trait ChangeWatcher {
    /// Resolve once the head may differ from `since` (`None` means "no head
    /// known"). A head differs when its authority sequence or manifest digest
    /// differs. Returns the possibly-changed head as a hint only.
    ///
    /// # Errors
    ///
    /// Returns [`Error::WatchClosed`] when the notification source is gone
    /// and transport errors where applicable.
    async fn wait_for_change(&self, since: Option<&DatasetHead>) -> Result<DatasetHead>;
}

/// Complete in-memory reference implementation of the dataset traits.
///
/// This is a real implementation, not a test double: it runs the same
/// manifest validation, snapshot integrity checks, and compare-and-swap
/// expected of any transport, and it assigns real authority sequences: 1 for
/// the first commit, incremented by 1 per successful commit, advanced
/// atomically with the state swap and left unchanged by any failed
/// publication. A BEP 46 transport adapter maps these sequences to signed
/// BEP 44 `seq` values one-to-one. It is useful for local-first datasets and
/// as the conformance reference for transports.
#[derive(Debug, Clone)]
pub struct MemoryDataset {
    identity: PublisherId,
    state: Arc<Mutex<Option<Committed>>>,
    changes: watch::Sender<Option<DatasetHead>>,
}

/// Atomically committed in-memory state: head plus its validated snapshot.
#[derive(Debug, Clone)]
struct Committed {
    head: DatasetHead,
    snapshot: Snapshot,
}

impl MemoryDataset {
    /// Create an empty dataset owned by `identity`.
    #[must_use]
    pub fn new(identity: PublisherId) -> Self {
        Self {
            identity,
            state: Arc::new(Mutex::new(None)),
            changes: watch::channel(None).0,
        }
    }

    /// Publisher identity owning this dataset.
    #[must_use]
    pub const fn identity(&self) -> &PublisherId {
        &self.identity
    }

    /// Read handle over the shared state.
    #[must_use]
    pub fn reader(&self) -> MemoryReader {
        MemoryReader {
            state: Arc::clone(&self.state),
        }
    }

    /// Publish handle over the shared state.
    #[must_use]
    pub fn publisher(&self) -> MemoryPublisher {
        MemoryPublisher {
            identity: self.identity.clone(),
            state: Arc::clone(&self.state),
            changes: self.changes.clone(),
        }
    }

    /// Best-effort change watcher over the shared state.
    #[must_use]
    pub fn watcher(&self) -> MemoryWatcher {
        MemoryWatcher {
            receiver: self.changes.subscribe(),
        }
    }
}

/// [`DatasetReader`] handle produced by [`MemoryDataset::reader`].
#[derive(Debug, Clone)]
pub struct MemoryReader {
    state: Arc<Mutex<Option<Committed>>>,
}

impl MemoryReader {
    fn state(&self) -> Result<MutexGuard<'_, Option<Committed>>> {
        self.state.lock().map_err(|_| Error::LockPoisoned)
    }

    /// Clone of the current validated snapshot, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LockPoisoned`] if the state lock is poisoned.
    pub fn snapshot(&self) -> Result<Option<Snapshot>> {
        Ok(self
            .state()?
            .as_ref()
            .map(|committed| committed.snapshot.clone()))
    }
}

#[async_trait]
impl DatasetReader for MemoryReader {
    async fn head(&self) -> Result<Option<DatasetHead>> {
        Ok(self
            .state()?
            .as_ref()
            .map(|committed| committed.head.clone()))
    }

    async fn get(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .state()?
            .as_ref()
            .and_then(|committed| committed.snapshot.get(path).map(<[u8]>::to_vec)))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<ManifestObjectV1>> {
        Ok(self.state()?.as_ref().map_or_else(Vec::new, |committed| {
            committed
                .snapshot
                .list(prefix)
                .into_iter()
                .cloned()
                .collect()
        }))
    }
}

/// [`DatasetPublisher`] handle produced by [`MemoryDataset::publisher`].
#[derive(Debug, Clone)]
pub struct MemoryPublisher {
    identity: PublisherId,
    state: Arc<Mutex<Option<Committed>>>,
    changes: watch::Sender<Option<DatasetHead>>,
}

#[async_trait]
impl DatasetPublisher for MemoryPublisher {
    async fn publish(
        &self,
        publication: Publication,
        expected_previous: Option<ManifestDigest>,
    ) -> Result<Snapshot> {
        // Stage and fully validate everything before touching shared state.
        let manifest_objects = publication
            .objects
            .iter()
            .map(|(path, bytes)| ManifestObjectV1::from_bytes(path.clone(), bytes))
            .collect();
        let manifest = DatasetManifestV1::new(
            self.identity.clone(),
            publication.created_at,
            manifest_objects,
        )?;
        let snapshot = Snapshot::new(manifest, publication.objects)?;

        // Atomic commit: one lock-protected compare-and-swap that also
        // advances the authority sequence. Any error above leaves the
        // previous head and sequence untouched; nothing below can fail
        // partially. The sequence derives only from committed state, so a
        // failed publication never consumes a sequence number.
        let mut state = self.state.lock().map_err(|_| Error::LockPoisoned)?;
        let actual = state.as_ref().map(|committed| committed.snapshot.digest());
        if actual != expected_previous {
            return Err(Error::ConcurrentUpdate {
                expected: expected_previous,
                actual,
            });
        }
        let next_sequence = state.as_ref().map_or(Ok(1), |committed| {
            committed
                .head
                .authority_sequence()
                .checked_add(1)
                .ok_or(Error::SequenceExhausted)
        })?;
        let head = DatasetHead::new(snapshot.provenance(), next_sequence)?;
        *state = Some(Committed {
            head: head.clone(),
            snapshot: snapshot.clone(),
        });
        drop(state);
        let _ = self.changes.send_replace(Some(head));
        Ok(snapshot)
    }
}

/// [`ChangeWatcher`] handle produced by [`MemoryDataset::watcher`].
#[derive(Debug)]
pub struct MemoryWatcher {
    receiver: watch::Receiver<Option<DatasetHead>>,
}

#[async_trait]
impl ChangeWatcher for MemoryWatcher {
    async fn wait_for_change(&self, since: Option<&DatasetHead>) -> Result<DatasetHead> {
        let mut receiver = self.receiver.clone();
        loop {
            let current = receiver.borrow_and_update().clone();
            let up_to_date = match (&current, since) {
                (None, None) => true,
                (Some(current), Some(since)) => {
                    current.authority_sequence() == since.authority_sequence()
                        && current.manifest_digest() == since.manifest_digest()
                }
                _ => false,
            };
            if !up_to_date && let Some(head) = current {
                return Ok(head);
            }
            receiver.changed().await.map_err(|_| Error::WatchClosed)?;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pubky::Keypair;

    use super::*;

    fn identity(seed: u8) -> PublisherId {
        PublisherId::new(Keypair::from_secret(&[seed; 32]).public_key())
    }

    fn head(seed: u8, authority_sequence: u64, created_at: u64, fill: u8) -> DatasetHead {
        DatasetHead::new(
            Provenance::new(
                identity(seed),
                created_at,
                ManifestDigest::from_bytes([fill; 32]),
            ),
            authority_sequence,
        )
        .unwrap()
    }

    fn objects() -> Vec<(String, Vec<u8>)> {
        vec![
            ("docs/a.txt".to_owned(), b"alpha".to_vec()),
            ("docs/sub/b.txt".to_owned(), b"beta".to_vec()),
            ("docsify.txt".to_owned(), b"gamma".to_vec()),
            ("readme.md".to_owned(), b"readme".to_vec()),
        ]
    }

    #[tokio::test]
    async fn publish_and_read_round_trip() {
        let dataset = MemoryDataset::new(identity(1));
        let publisher = dataset.publisher();
        let reader = dataset.reader();

        assert!(reader.head().await.unwrap().is_none());
        assert!(reader.get("readme.md").await.unwrap().is_none());

        let snapshot = publisher
            .publish(Publication::new(1_000, objects()), None)
            .await
            .unwrap();
        assert_eq!(snapshot.len(), 4);
        assert_eq!(snapshot.manifest().publisher(), &identity(1));

        let head = reader.head().await.unwrap().unwrap();
        assert_eq!(head.provenance(), &snapshot.provenance());
        assert_eq!(head.authority_sequence(), 1);
        assert_eq!(head.manifest_digest(), snapshot.digest());

        assert_eq!(
            reader.get("docs/sub/b.txt").await.unwrap().unwrap(),
            b"beta"
        );
        assert!(reader.get("missing.txt").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn publish_enforces_cas_and_advances_sequence_atomically() {
        let dataset = MemoryDataset::new(identity(2));
        let publisher = dataset.publisher();
        let reader = dataset.reader();

        let wrong = ManifestDigest::from_bytes([9; 32]);
        assert!(matches!(
            publisher
                .publish(Publication::new(1_000, objects()), Some(wrong))
                .await,
            Err(Error::ConcurrentUpdate {
                expected: Some(_),
                actual: None
            })
        ));
        // The failed publication consumed no sequence.
        assert!(reader.head().await.unwrap().is_none());

        let first = publisher
            .publish(Publication::new(1_000, objects()), None)
            .await
            .unwrap();
        assert_eq!(
            reader.head().await.unwrap().unwrap().authority_sequence(),
            1
        );

        for expected in [None, Some(wrong)] {
            assert!(matches!(
                publisher
                    .publish(Publication::new(2_000, objects()), expected)
                    .await,
                Err(Error::ConcurrentUpdate { .. })
            ));
            // CAS failures leave head and sequence unchanged.
            let head = reader.head().await.unwrap().unwrap();
            assert_eq!(head.authority_sequence(), 1);
            assert_eq!(head.manifest_digest(), first.digest());
        }

        // An older (skewed) timestamp with a valid CAS base is accepted:
        // ordering comes from the sequence, never from created_at.
        let second = publisher
            .publish(
                Publication::new(500, vec![("only.txt".to_owned(), b"new".to_vec())]),
                Some(first.digest()),
            )
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second.manifest().created_at(), 500);

        let head = reader.head().await.unwrap().unwrap();
        assert_eq!(head.authority_sequence(), 2);
        assert_eq!(head.manifest_digest(), second.digest());
        assert!(reader.get("readme.md").await.unwrap().is_none());
        assert_eq!(reader.get("only.txt").await.unwrap().unwrap(), b"new");
    }

    #[tokio::test]
    async fn failed_publication_leaves_previous_head_untouched() {
        let dataset = MemoryDataset::new(identity(3));
        let publisher = dataset.publisher();
        let first = publisher
            .publish(Publication::new(1_000, objects()), None)
            .await
            .unwrap();

        let invalid = Publication::new(
            2_000,
            vec![
                ("ok.txt".to_owned(), b"ok".to_vec()),
                ("../escape".to_owned(), b"bad".to_vec()),
            ],
        );
        assert!(matches!(
            publisher.publish(invalid, Some(first.digest())).await,
            Err(Error::Protocol(_))
        ));

        let head = dataset.reader().head().await.unwrap().unwrap();
        assert_eq!(head.manifest_digest(), first.digest());
        assert_eq!(head.authority_sequence(), 1);
        assert_eq!(
            dataset.reader().get("readme.md").await.unwrap().unwrap(),
            b"readme"
        );
    }

    #[tokio::test]
    async fn publish_is_deterministic_across_object_orderings() {
        let count = objects().len();
        let mut digests = Vec::new();
        for rotation in 0..count {
            let mut shuffled = objects();
            shuffled.rotate_left(rotation);
            if rotation % 2 == 1 {
                shuffled.reverse();
            }
            let dataset = MemoryDataset::new(identity(4));
            let snapshot = dataset
                .publisher()
                .publish(Publication::new(1_000, shuffled), None)
                .await
                .unwrap();
            digests.push(snapshot.digest());
        }
        assert!(digests.windows(2).all(|pair| pair[0] == pair[1]));
    }

    #[tokio::test]
    async fn list_uses_literal_prefix_semantics() {
        let dataset = MemoryDataset::new(identity(5));
        dataset
            .publisher()
            .publish(Publication::new(1_000, objects()), None)
            .await
            .unwrap();
        let reader = dataset.reader();

        let paths = |entries: Vec<ManifestObjectV1>| {
            entries
                .into_iter()
                .map(|entry| entry.path)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            paths(reader.list("").await.unwrap()),
            vec!["docs/a.txt", "docs/sub/b.txt", "docsify.txt", "readme.md"]
        );
        assert_eq!(
            paths(reader.list("docs/").await.unwrap()),
            vec!["docs/a.txt", "docs/sub/b.txt"]
        );
        assert_eq!(
            paths(reader.list("docs").await.unwrap()),
            vec!["docs/a.txt", "docs/sub/b.txt", "docsify.txt"]
        );
        assert!(reader.list("absent").await.unwrap().is_empty());
        assert_eq!(reader.list("docs/a.txt").await.unwrap()[0].size, 5);
    }

    #[test]
    fn snapshot_rejects_integrity_violations() {
        let manifest = DatasetManifestV1::new(
            identity(6),
            1_000,
            objects()
                .iter()
                .map(|(path, bytes)| ManifestObjectV1::from_bytes(path.clone(), bytes))
                .collect(),
        )
        .unwrap();

        let mut tampered = objects();
        tampered[0].1[0] ^= 0x01;
        assert!(matches!(
            Snapshot::new(manifest.clone(), tampered),
            Err(Error::Integrity { .. })
        ));

        let mut wrong_size = objects();
        wrong_size[0].1.push(b'!');
        assert!(matches!(
            Snapshot::new(manifest.clone(), wrong_size),
            Err(Error::Integrity { .. })
        ));

        let mut missing = objects();
        missing.remove(1);
        assert!(matches!(
            Snapshot::new(manifest.clone(), missing),
            Err(Error::MissingObject { .. })
        ));

        let mut extra = objects();
        extra.push(("extra.txt".to_owned(), b"x".to_vec()));
        assert!(matches!(
            Snapshot::new(manifest.clone(), extra),
            Err(Error::UnexpectedObject { .. })
        ));

        let mut duplicate = objects();
        duplicate.push(("readme.md".to_owned(), b"again".to_vec()));
        assert!(matches!(
            Snapshot::new(manifest.clone(), duplicate),
            Err(Error::DuplicateObject { .. })
        ));

        assert!(Snapshot::new(manifest, objects()).is_ok());
    }

    #[test]
    fn dataset_head_requires_positive_sequence() {
        let provenance = Provenance::new(identity(7), 1_000, ManifestDigest::from_bytes([1; 32]));
        assert!(matches!(
            DatasetHead::new(provenance.clone(), 0),
            Err(Error::InvalidSequence(0))
        ));
        assert_eq!(
            DatasetHead::new(provenance, 1)
                .unwrap()
                .authority_sequence(),
            1
        );
    }

    #[test]
    fn freshness_tracker_orders_by_sequence_not_timestamp() {
        let mut tracker = FreshnessTracker::new();
        assert!(tracker.highest_seen().is_none());

        let first = head(7, 1, 1_000, 1);
        // Higher sequence but an older (skewed) timestamp: still an advance.
        let second = head(7, 2, 500, 2);
        // Same sequence, different digest: conflict, regardless of timestamp.
        let conflict = head(7, 2, 9_999, 3);
        // Lower sequence but a newer timestamp: still a rollback.
        let stale = head(7, 1, 9_999, 1);
        let foreign = head(8, 3, 3_000, 4);

        assert_eq!(tracker.check(&first).unwrap(), Freshness::Initial);
        assert_eq!(tracker.accept(&first).unwrap(), Freshness::Initial);
        assert_eq!(tracker.highest_seen(), Some(&first));

        assert_eq!(tracker.accept(&second).unwrap(), Freshness::Advanced);
        assert_eq!(tracker.check(&second).unwrap(), Freshness::Unchanged);
        assert_eq!(tracker.accept(&second).unwrap(), Freshness::Unchanged);

        assert!(matches!(
            tracker.check(&stale),
            Err(Error::Rollback {
                highest_seen: 2,
                resolved: 1
            })
        ));
        assert!(matches!(
            tracker.check(&conflict),
            Err(Error::ConflictingHead {
                authority_sequence: 2
            })
        ));
        assert!(matches!(
            tracker.check(&foreign),
            Err(Error::PublisherMismatch { .. })
        ));

        // Failed checks record nothing.
        assert_eq!(tracker.highest_seen(), Some(&second));

        // A persisted tracker resumes rollback protection across restarts.
        let resumed = FreshnessTracker::from_highest(Some(second.clone()));
        assert!(matches!(
            resumed.check(&stale),
            Err(Error::Rollback {
                highest_seen: 2,
                resolved: 1
            })
        ));
    }

    #[tokio::test]
    async fn watcher_signals_changes_best_effort() {
        let dataset = MemoryDataset::new(identity(9));
        let watcher = dataset.watcher();

        let first = dataset
            .publisher()
            .publish(Publication::new(1_000, objects()), None)
            .await
            .unwrap();

        // A head already differs from "nothing known".
        let noticed = watcher.wait_for_change(None).await.unwrap();
        assert_eq!(noticed.manifest_digest(), first.digest());
        assert_eq!(noticed.authority_sequence(), 1);

        // An unchanged head does not signal; a later publish does.
        let pending = tokio::spawn({
            let watcher = dataset.watcher();
            let current = noticed.clone();
            async move { watcher.wait_for_change(Some(&current)).await }
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());

        let second = dataset
            .publisher()
            .publish(
                Publication::new(2_000, vec![("only.txt".to_owned(), b"new".to_vec())]),
                Some(first.digest()),
            )
            .await
            .unwrap();
        let noticed = tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(noticed.manifest_digest(), second.digest());
        assert_eq!(noticed.authority_sequence(), 2);
    }

    #[tokio::test]
    async fn watcher_reports_closed_source() {
        let watcher = {
            let dataset = MemoryDataset::new(identity(10));
            dataset.watcher()
        };
        let result = tokio::time::timeout(Duration::from_secs(5), watcher.wait_for_change(None))
            .await
            .unwrap();
        assert!(matches!(result, Err(Error::WatchClosed)));
    }
}
