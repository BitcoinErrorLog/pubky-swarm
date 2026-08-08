//! Transport-neutral wire types shared by Pubky Swarm components.

#![forbid(unsafe_code)]

use std::fmt::{Display, Formatter};
use std::path::{Component, Path};
use std::str::FromStr;

use pubky::PublicKey;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod catalog;

pub use catalog::{
    ArtifactId, BlocklistV1, Btmh, CanonicalUri, CollectionV1, DerivedContentType,
    ImportObservation, ImportProvenance, MAX_ARTIFACT_SUBJECTS, MAX_IMPORT_OBSERVATIONS,
    ModerationAction, ModerationDecisionV1, ObservationValue, PublicCatalogImport, RelationKind,
    SourceAttribution, SubjectRef, SuggestionNamespace, TagClaimV1, TagOperation, TombstoneV1,
    TorrentRef, TrackerSource, derive_file_observations, ensure_public_catalog_eligible,
    map_bep38_relation, map_info_name, map_issuer_suggestion, map_magnet_dn, map_private,
    map_top_level_comment, map_top_level_creation_date, map_top_level_creator, map_tracker,
};

/// Release object schema name.
pub const RELEASE_SCHEMA: &str = "pubky.swarm/release";
/// Current release object version.
pub const RELEASE_VERSION: u16 = 1;
/// Root Pubky namespace for this protocol version.
pub const RELEASES_PATH: &str = "/pub/pubky.swarm/v1/releases/";

/// Protocol type validation failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A `BitTorrent` v1 info hash was not 40 hexadecimal characters.
    #[error("invalid v1 info hash: {0}")]
    InvalidInfoHash(String),
    /// A publisher was not a valid Pubky identity.
    #[error("invalid publisher identity: {0}")]
    InvalidPublisher(String),
    /// A release identifier was malformed or did not match its content.
    #[error("invalid release id: {0}")]
    InvalidReleaseId(String),
    /// A release field violated its protocol bound.
    #[error("invalid release field {field}: {reason}")]
    InvalidField {
        /// Field containing the invalid value.
        field: &'static str,
        /// Specific failed constraint.
        reason: String,
    },
    /// A release file path was unsafe or non-canonical.
    #[error("invalid release file path {path:?}: {reason}")]
    InvalidPath {
        /// Rejected path.
        path: String,
        /// Specific failed constraint.
        reason: &'static str,
    },
    /// Declared file sizes did not match the torrent total.
    #[error("release file sizes total {files_total}, torrent declares {torrent_total}")]
    SizeMismatch {
        /// Sum of release file sizes.
        files_total: u64,
        /// Declared torrent size.
        torrent_total: u64,
    },
    /// A BLAKE3 object digest was not 64 hexadecimal characters.
    #[error("invalid object digest: {0}")]
    InvalidObjectDigest(String),
    /// A manifest digest was not 64 hexadecimal characters.
    #[error("invalid manifest digest: {0}")]
    InvalidManifestDigest(String),
    /// Canonical manifest bytes were malformed or non-canonical.
    #[error("invalid canonical manifest bytes: {0}")]
    InvalidCanonicalBytes(&'static str),
    /// A verified read referenced an object the manifest does not declare.
    #[error("unknown manifest object {path:?}")]
    UnknownObject {
        /// Requested path.
        path: String,
    },
    /// Object bytes did not match the manifest declaration.
    #[error("manifest object {path:?} failed verification: {reason}")]
    ObjectMismatch {
        /// Mismatched path.
        path: String,
        /// Specific failed constraint.
        reason: &'static str,
    },
}

/// `BitTorrent` v1 SHA-1 info hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InfoHashV1([u8; 20]);

impl InfoHashV1 {
    /// Construct from the canonical 20 raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// Return the raw hash bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }

    /// Return lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl FromStr for InfoHashV1 {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 40 {
            return Err(Error::InvalidInfoHash(format!(
                "expected 40 hexadecimal characters, got {}",
                value.len()
            )));
        }
        let bytes =
            hex::decode(value).map_err(|error| Error::InvalidInfoHash(error.to_string()))?;
        let bytes: [u8; 20] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            Error::InvalidInfoHash(format!("expected 20 bytes, got {}", bytes.len()))
        })?;
        Ok(Self(bytes))
    }
}

impl Display for InfoHashV1 {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl Serialize for InfoHashV1 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for InfoHashV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

/// Pubky publisher identity serialized as canonical z-base32.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublisherId(PublicKey);

impl PublisherId {
    /// Construct from an official Pubky public key.
    #[must_use]
    pub const fn new(public_key: PublicKey) -> Self {
        Self(public_key)
    }

    /// Parse and validate raw Ed25519 public-key bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidPublisher`] if the bytes are not a valid
    /// Ed25519 point.
    pub fn from_bytes(bytes: [u8; 32]) -> Result<Self, Error> {
        pubky::pkarr::PublicKey::try_from(&bytes)
            .map(PublicKey::from)
            .map(Self)
            .map_err(|error| Error::InvalidPublisher(error.to_string()))
    }

    /// Borrow the official Pubky key.
    #[must_use]
    pub const fn public_key(&self) -> &PublicKey {
        &self.0
    }

    /// Return raw Ed25519 bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }
}

impl Display for PublisherId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

impl FromStr for PublisherId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        PublicKey::try_from(value)
            .map(Self)
            .map_err(|error| Error::InvalidPublisher(error.to_string()))
    }
}

impl Serialize for PublisherId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PublisherId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Deterministic 128-bit identifier for a release record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReleaseId([u8; 16]);

impl ReleaseId {
    fn derive(publisher: &PublisherId, created_at: u64, info_hash: InfoHashV1) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"pubky.swarm/release/v1\0");
        hasher.update(&publisher.to_bytes());
        hasher.update(&created_at.to_be_bytes());
        hasher.update(info_hash.as_bytes());
        let mut id = [0_u8; 16];
        id.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
        Self(id)
    }

    /// Lowercase hexadecimal representation used as the storage filename.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl Display for ReleaseId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl FromStr for ReleaseId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 32 {
            return Err(Error::InvalidReleaseId(format!(
                "expected 32 hexadecimal characters, got {}",
                value.len()
            )));
        }
        let bytes =
            hex::decode(value).map_err(|error| Error::InvalidReleaseId(error.to_string()))?;
        let id: [u8; 16] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            Error::InvalidReleaseId(format!("expected 16 bytes, got {}", bytes.len()))
        })?;
        Ok(Self(id))
    }
}

impl Serialize for ReleaseId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ReleaseId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One file advertised by a payload torrent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseFile {
    /// Canonical `/`-separated relative path.
    pub path: String,
    /// File size in bytes.
    pub size: u64,
}

/// Payload torrent descriptor for release schema v1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TorrentV1 {
    /// v1 SHA-1 info hash.
    pub info_hash: InfoHashV1,
    /// Total payload bytes.
    pub size: u64,
    /// Files in torrent order.
    pub files: Vec<ReleaseFile>,
    /// Optional HTTP(S) or UDP tracker hints.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trackers: Vec<String>,
}

/// Validated Pubky Swarm torrent release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseV1 {
    schema: String,
    version: u16,
    id: ReleaseId,
    publisher: PublisherId,
    created_at: u64,
    title: String,
    description: String,
    torrent: TorrentV1,
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct ReleaseWire {
    schema: String,
    version: u16,
    id: ReleaseId,
    publisher: PublisherId,
    created_at: u64,
    title: String,
    #[serde(default)]
    description: String,
    torrent: TorrentV1,
    #[serde(default)]
    tags: Vec<String>,
}

impl ReleaseV1 {
    /// Construct and validate a release.
    ///
    /// # Errors
    ///
    /// Returns a field, path, tracker, size, or protocol validation error.
    pub fn new(
        publisher: PublisherId,
        created_at: u64,
        title: String,
        description: String,
        torrent: TorrentV1,
        tags: Vec<String>,
    ) -> Result<Self, Error> {
        let id = ReleaseId::derive(&publisher, created_at, torrent.info_hash);
        let release = Self {
            schema: RELEASE_SCHEMA.to_owned(),
            version: RELEASE_VERSION,
            id,
            publisher,
            created_at,
            title,
            description,
            torrent,
            tags,
        };
        release.validate()?;
        Ok(release)
    }

    fn try_from_wire(wire: ReleaseWire) -> Result<Self, Error> {
        let release = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            publisher: wire.publisher,
            created_at: wire.created_at,
            title: wire.title,
            description: wire.description,
            torrent: wire.torrent,
            tags: wire.tags,
        };
        release.validate()?;
        let expected = ReleaseId::derive(
            &release.publisher,
            release.created_at,
            release.torrent.info_hash,
        );
        if release.id != expected {
            return Err(Error::InvalidReleaseId(
                "id does not match publisher, timestamp, and infohash".to_owned(),
            ));
        }
        Ok(release)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.schema != RELEASE_SCHEMA {
            return Err(invalid_field("schema", "unsupported schema"));
        }
        if self.version != RELEASE_VERSION {
            return Err(invalid_field("version", "unsupported version"));
        }
        validate_text("title", &self.title, 1, 200)?;
        validate_text("description", &self.description, 0, 4_000)?;
        if self.created_at == 0 {
            return Err(invalid_field("created_at", "must be positive"));
        }
        validate_torrent(&self.torrent)?;
        validate_tags(&self.tags)
    }

    /// Deterministic release identifier.
    #[must_use]
    pub const fn id(&self) -> ReleaseId {
        self.id
    }

    /// Publisher identity.
    #[must_use]
    pub const fn publisher(&self) -> &PublisherId {
        &self.publisher
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Display title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Optional description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Payload torrent.
    #[must_use]
    pub const fn torrent(&self) -> &TorrentV1 {
        &self.torrent
    }

    /// Portable catalog reference for this v1 torrent.
    ///
    /// This is an additive projection and does not change the release v1 wire
    /// format or identifier derivation.
    #[must_use]
    pub const fn torrent_ref(&self) -> TorrentRef {
        TorrentRef::btih(self.torrent.info_hash)
    }

    /// Normalized discovery tags.
    #[must_use]
    pub fn tags(&self) -> &[String] {
        &self.tags
    }

    /// Canonical Pubky storage path.
    #[must_use]
    pub fn storage_path(&self) -> String {
        format!("{RELEASES_PATH}{}.json", self.id)
    }
}

impl<'de> Deserialize<'de> for ReleaseV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from_wire(ReleaseWire::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

fn invalid_field(field: &'static str, reason: impl Into<String>) -> Error {
    Error::InvalidField {
        field,
        reason: reason.into(),
    }
}

fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
) -> Result<(), Error> {
    let length = value.chars().count();
    if length < minimum || length > maximum || (minimum > 0 && value.trim().is_empty()) {
        return Err(invalid_field(
            field,
            format!("expected {minimum}..={maximum} characters, got {length}"),
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(invalid_field(field, "control characters are not allowed"));
    }
    Ok(())
}

fn validate_torrent(torrent: &TorrentV1) -> Result<(), Error> {
    if torrent.size == 0 {
        return Err(invalid_field("torrent.size", "must be positive"));
    }
    if torrent.files.is_empty() || torrent.files.len() > 10_000 {
        return Err(invalid_field("torrent.files", "expected 1..=10000 files"));
    }
    let mut paths = Vec::with_capacity(torrent.files.len());
    let mut total = 0_u64;
    for file in &torrent.files {
        validate_portable_path(&file.path)?;
        total = total
            .checked_add(file.size)
            .ok_or_else(|| invalid_field("torrent.files", "file sizes overflow u64"))?;
        paths.push(file.path.as_str());
    }
    paths.sort_unstable();
    for pair in paths.windows(2) {
        if pair[0] == pair[1] || pair[1].starts_with(&format!("{}/", pair[0])) {
            return Err(invalid_field(
                "torrent.files",
                "duplicate or file/directory-colliding paths",
            ));
        }
    }
    if total != torrent.size {
        return Err(Error::SizeMismatch {
            files_total: total,
            torrent_total: torrent.size,
        });
    }
    validate_trackers(&torrent.trackers)
}

fn validate_portable_path(value: &str) -> Result<(), Error> {
    if value.is_empty() || value.len() > 4_096 || Path::new(value).is_absolute() {
        return Err(Error::InvalidPath {
            path: value.to_owned(),
            reason: "path must be a non-empty relative path up to 4096 bytes",
        });
    }
    let components: Vec<_> = Path::new(value).components().collect();
    if components.is_empty()
        || components.len() > 64
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error::InvalidPath {
            path: value.to_owned(),
            reason: "path contains non-normal components",
        });
    }
    let canonical = components
        .iter()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if canonical != value {
        return Err(Error::InvalidPath {
            path: value.to_owned(),
            reason: "path is not in canonical `/`-separated form",
        });
    }
    if components.iter().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        text.is_empty()
            || text.len() > 255
            || text.contains('\\')
            || text.chars().any(char::is_control)
            || text.ends_with(['.', ' '])
            || is_windows_reserved_name(&text)
    }) {
        return Err(Error::InvalidPath {
            path: value.to_owned(),
            reason: "path contains a non-portable component",
        });
    }
    Ok(())
}

fn is_windows_reserved_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn validate_trackers(trackers: &[String]) -> Result<(), Error> {
    if trackers.len() > 16 {
        return Err(invalid_field("torrent.trackers", "maximum is 16"));
    }
    for tracker in trackers {
        catalog::validate_tracker_url(tracker)?;
    }
    Ok(())
}

fn validate_tags(tags: &[String]) -> Result<(), Error> {
    if tags.len() > 16 {
        return Err(invalid_field("tags", "maximum is 16"));
    }
    let mut previous = None;
    for tag in tags {
        if tag.is_empty()
            || tag.len() > 32
            || !tag
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(invalid_field(
                "tags",
                "tags must be 1..=32 lowercase ASCII letters, digits, or hyphens",
            ));
        }
        if previous.is_some_and(|value| value >= tag.as_str()) {
            return Err(invalid_field("tags", "tags must be sorted and unique"));
        }
        previous = Some(tag.as_str());
    }
    Ok(())
}

/// Dataset manifest object schema name.
pub const DATASET_MANIFEST_SCHEMA: &str = "pubky.swarm/dataset-manifest";
/// Current dataset manifest object version.
pub const DATASET_MANIFEST_VERSION: u16 = 1;
/// Maximum number of logical objects in one dataset manifest.
pub const MAX_MANIFEST_OBJECTS: usize = 100_000;

/// Domain separator prefixing the canonical manifest byte encoding.
const CANONICAL_MANIFEST_PREFIX: &[u8] = b"pubky.swarm/dataset-manifest/v1\0";

fn parse_hex_digest(value: &str, error: fn(String) -> Error) -> Result<[u8; 32], Error> {
    if value.len() != 64 {
        return Err(error(format!(
            "expected 64 hexadecimal characters, got {}",
            value.len()
        )));
    }
    let bytes = hex::decode(value).map_err(|decode_error| error(decode_error.to_string()))?;
    bytes
        .try_into()
        .map_err(|bytes: Vec<u8>| error(format!("expected 32 bytes, got {}", bytes.len())))
}

/// BLAKE3 digest of one logical dataset object's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectDigest([u8; 32]);

impl ObjectDigest {
    /// Digest the given object bytes.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Construct from the canonical 32 raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }

    /// Check whether `bytes` hash to this digest.
    #[must_use]
    pub fn verify(&self, bytes: &[u8]) -> bool {
        blake3::hash(bytes).as_bytes() == &self.0
    }
}

impl FromStr for ObjectDigest {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex_digest(value, Error::InvalidObjectDigest).map(Self)
    }
}

impl Display for ObjectDigest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for ObjectDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ObjectDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// BLAKE3 digest of a manifest's canonical bytes, identifying a dataset state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ManifestDigest([u8; 32]);

impl ManifestDigest {
    /// Construct from the canonical 32 raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Return lowercase hexadecimal.
    #[must_use]
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl FromStr for ManifestDigest {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        parse_hex_digest(value, Error::InvalidManifestDigest).map(Self)
    }
}

impl Display for ManifestDigest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl Serialize for ManifestDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ManifestDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// One logical object declared by a dataset manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestObjectV1 {
    /// Canonical `/`-separated relative logical path.
    pub path: String,
    /// Object size in bytes.
    pub size: u64,
    /// BLAKE3 digest of the object bytes.
    pub digest: ObjectDigest,
}

impl ManifestObjectV1 {
    /// Declare an object from its path and bytes, computing size and digest.
    #[must_use]
    pub fn from_bytes(path: String, bytes: &[u8]) -> Self {
        Self {
            path,
            size: bytes.len() as u64,
            digest: ObjectDigest::of(bytes),
        }
    }
}

/// Validated, versioned, transport-neutral dataset manifest.
///
/// Objects are always sorted by canonical logical path. The manifest
/// authenticates object bytes; authority over which manifest is current is a
/// transport concern (for example a BEP 46 head signed by `publisher`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DatasetManifestV1 {
    schema: String,
    version: u16,
    publisher: PublisherId,
    created_at: u64,
    objects: Vec<ManifestObjectV1>,
}

#[derive(Deserialize)]
struct ManifestWire {
    schema: String,
    version: u16,
    publisher: PublisherId,
    created_at: u64,
    objects: Vec<ManifestObjectV1>,
}

impl DatasetManifestV1 {
    /// Construct and validate a manifest, sorting objects into canonical order.
    ///
    /// # Errors
    ///
    /// Returns a field, path, duplicate, prefix-collision, or resource-limit
    /// validation error.
    pub fn new(
        publisher: PublisherId,
        created_at: u64,
        mut objects: Vec<ManifestObjectV1>,
    ) -> Result<Self, Error> {
        objects.sort_by(|left, right| left.path.cmp(&right.path));
        let manifest = Self {
            schema: DATASET_MANIFEST_SCHEMA.to_owned(),
            version: DATASET_MANIFEST_VERSION,
            publisher,
            created_at,
            objects,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    fn try_from_wire(wire: ManifestWire) -> Result<Self, Error> {
        let manifest = Self {
            schema: wire.schema,
            version: wire.version,
            publisher: wire.publisher,
            created_at: wire.created_at,
            objects: wire.objects,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.schema != DATASET_MANIFEST_SCHEMA {
            return Err(invalid_field("schema", "unsupported schema"));
        }
        if self.version != DATASET_MANIFEST_VERSION {
            return Err(invalid_field("version", "unsupported version"));
        }
        if self.created_at == 0 {
            return Err(invalid_field("created_at", "must be positive"));
        }
        if self.objects.len() > MAX_MANIFEST_OBJECTS {
            return Err(invalid_field(
                "objects",
                format!("maximum is {MAX_MANIFEST_OBJECTS} objects"),
            ));
        }
        let mut previous: Option<&str> = None;
        for object in &self.objects {
            validate_portable_path(&object.path)?;
            if let Some(previous) = previous {
                if object.path.as_str() <= previous {
                    return Err(invalid_field(
                        "objects",
                        "objects must be sorted and unique by path",
                    ));
                }
                if object.path.starts_with(&format!("{previous}/")) {
                    return Err(invalid_field(
                        "objects",
                        "object path collides with a file used as a directory",
                    ));
                }
            }
            previous = Some(object.path.as_str());
        }
        Ok(())
    }

    /// Publisher identity owning this dataset.
    #[must_use]
    pub const fn publisher(&self) -> &PublisherId {
        &self.publisher
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Objects sorted by canonical logical path.
    #[must_use]
    pub fn objects(&self) -> &[ManifestObjectV1] {
        &self.objects
    }

    /// Look up a declared object by exact path.
    #[must_use]
    pub fn object(&self, path: &str) -> Option<&ManifestObjectV1> {
        self.objects
            .binary_search_by(|object| object.path.as_str().cmp(path))
            .ok()
            .map(|index| &self.objects[index])
    }

    /// Deterministic canonical byte encoding of this manifest.
    ///
    /// The encoding is platform-independent: fixed domain-separation prefix,
    /// big-endian integers, length-prefixed UTF-8 paths, objects in sorted
    /// order.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes =
            Vec::with_capacity(CANONICAL_MANIFEST_PREFIX.len() + 48 + self.objects.len() * 48);
        bytes.extend_from_slice(CANONICAL_MANIFEST_PREFIX);
        bytes.extend_from_slice(&self.publisher.to_bytes());
        bytes.extend_from_slice(&self.created_at.to_be_bytes());
        bytes.extend_from_slice(&(self.objects.len() as u64).to_be_bytes());
        for object in &self.objects {
            bytes.extend_from_slice(&(object.path.len() as u64).to_be_bytes());
            bytes.extend_from_slice(object.path.as_bytes());
            bytes.extend_from_slice(&object.size.to_be_bytes());
            bytes.extend_from_slice(object.digest.as_bytes());
        }
        bytes
    }

    /// Parse and validate canonical bytes produced by [`Self::to_canonical_bytes`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidCanonicalBytes`] for malformed input and the
    /// usual validation errors for non-canonical or invalid manifests.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, Error> {
        fn take<'a>(cursor: &mut &'a [u8], length: usize) -> Result<&'a [u8], Error> {
            if cursor.len() < length {
                return Err(Error::InvalidCanonicalBytes("truncated"));
            }
            let (head, tail) = cursor.split_at(length);
            *cursor = tail;
            Ok(head)
        }

        fn take_array<const N: usize>(cursor: &mut &[u8]) -> Result<[u8; N], Error> {
            let bytes = take(cursor, N)?;
            let mut array = [0_u8; N];
            array.copy_from_slice(bytes);
            Ok(array)
        }

        let mut cursor = bytes;
        if take(&mut cursor, CANONICAL_MANIFEST_PREFIX.len())? != CANONICAL_MANIFEST_PREFIX {
            return Err(Error::InvalidCanonicalBytes("unsupported domain prefix"));
        }
        let publisher = pubky::pkarr::PublicKey::try_from(take(&mut cursor, 32)?)
            .map(|key| PublisherId::new(PublicKey::from(key)))
            .map_err(|error| Error::InvalidPublisher(error.to_string()))?;
        let created_at = u64::from_be_bytes(take_array(&mut cursor)?);
        let count = u64::from_be_bytes(take_array(&mut cursor)?);
        if count > MAX_MANIFEST_OBJECTS as u64 {
            return Err(Error::InvalidCanonicalBytes("object count exceeds limit"));
        }
        let count = usize::try_from(count)
            .map_err(|_| Error::InvalidCanonicalBytes("object count exceeds usize"))?;
        let mut objects = Vec::with_capacity(count);
        for _ in 0..count {
            let path_length = u64::from_be_bytes(take_array(&mut cursor)?);
            let path_length = usize::try_from(path_length)
                .map_err(|_| Error::InvalidCanonicalBytes("path length exceeds usize"))?;
            let path = String::from_utf8(take(&mut cursor, path_length)?.to_vec())
                .map_err(|_| Error::InvalidCanonicalBytes("object path is not UTF-8"))?;
            let size = u64::from_be_bytes(take_array(&mut cursor)?);
            let digest = ObjectDigest::from_bytes(take_array(&mut cursor)?);
            objects.push(ManifestObjectV1 { path, size, digest });
        }
        if !cursor.is_empty() {
            return Err(Error::InvalidCanonicalBytes("trailing bytes"));
        }
        let manifest = Self {
            schema: DATASET_MANIFEST_SCHEMA.to_owned(),
            version: DATASET_MANIFEST_VERSION,
            publisher,
            created_at,
            objects,
        };
        manifest.validate()?;
        Ok(manifest)
    }

    /// BLAKE3 digest of the canonical bytes, identifying this dataset state.
    #[must_use]
    pub fn digest(&self) -> ManifestDigest {
        ManifestDigest(*blake3::hash(&self.to_canonical_bytes()).as_bytes())
    }

    /// Verify object bytes against the declared size and BLAKE3 digest.
    ///
    /// # Errors
    ///
    /// Returns [`Error::UnknownObject`] when `path` is undeclared and
    /// [`Error::ObjectMismatch`] when size or digest differ.
    pub fn verify_object(&self, path: &str, bytes: &[u8]) -> Result<&ManifestObjectV1, Error> {
        let object = self.object(path).ok_or_else(|| Error::UnknownObject {
            path: path.to_owned(),
        })?;
        if object.size != bytes.len() as u64 {
            return Err(Error::ObjectMismatch {
                path: path.to_owned(),
                reason: "byte length does not match the manifest",
            });
        }
        if !object.digest.verify(bytes) {
            return Err(Error::ObjectMismatch {
                path: path.to_owned(),
                reason: "BLAKE3 digest does not match the manifest",
            });
        }
        Ok(object)
    }
}

impl<'de> Deserialize<'de> for DatasetManifestV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::try_from_wire(ManifestWire::deserialize(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_hash_hex_and_serde_round_trip() {
        let hash = InfoHashV1::from_bytes([0xab; 20]);
        let encoded = serde_json::to_string(&hash).unwrap();
        assert_eq!(encoded, format!("\"{}\"", "ab".repeat(20)));
        assert_eq!(serde_json::from_str::<InfoHashV1>(&encoded).unwrap(), hash);
        assert_eq!(hash.to_string().parse::<InfoHashV1>().unwrap(), hash);
    }

    #[test]
    fn info_hash_rejects_bad_length_and_hex() {
        assert!(matches!(
            "ab".parse::<InfoHashV1>(),
            Err(Error::InvalidInfoHash(_))
        ));
        assert!(matches!(
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".parse::<InfoHashV1>(),
            Err(Error::InvalidInfoHash(_))
        ));
    }

    fn release() -> ReleaseV1 {
        let publisher = PublisherId::new(pubky::Keypair::from_secret(&[0x41; 32]).public_key());
        ReleaseV1::new(
            publisher,
            1_786_000_000_000,
            "Open Movie".to_owned(),
            "Freely distributable media".to_owned(),
            TorrentV1 {
                info_hash: InfoHashV1::from_bytes([0xab; 20]),
                size: 15,
                files: vec![
                    ReleaseFile {
                        path: "media/movie.mp4".to_owned(),
                        size: 10,
                    },
                    ReleaseFile {
                        path: "README.txt".to_owned(),
                        size: 5,
                    },
                ],
                trackers: vec!["https://tracker.example/announce".to_owned()],
            },
            vec!["film".to_owned(), "open-media".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn release_round_trip_and_deterministic_id() {
        let release = release();
        let json = serde_json::to_string_pretty(&release).unwrap();
        let decoded: ReleaseV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, release);
        assert_eq!(decoded.id().to_string().len(), 32);
        assert_eq!(
            decoded.storage_path(),
            format!("{RELEASES_PATH}{}.json", decoded.id())
        );
        assert!(json.contains("\"publisher\": \""));
        assert!(json.contains("\"info_hash\": \"abab"));
        assert_eq!(
            decoded.torrent_ref(),
            TorrentRef::btih(InfoHashV1::from_bytes([0xab; 20]))
        );
        assert!(!json.contains("torrent_ref"));
    }

    #[test]
    fn release_rejects_tampered_id_and_size() {
        let release = release();
        let mut value = serde_json::to_value(&release).unwrap();
        value["id"] = serde_json::Value::String("00".repeat(16));
        assert!(serde_json::from_value::<ReleaseV1>(value).is_err());

        let mut value = serde_json::to_value(&release).unwrap();
        value["torrent"]["size"] = serde_json::Value::from(16);
        assert!(serde_json::from_value::<ReleaseV1>(value).is_err());
    }

    #[test]
    fn release_rejects_unsafe_paths_tags_and_trackers() {
        let publisher = PublisherId::new(pubky::Keypair::from_secret(&[0x42; 32]).public_key());
        let base = TorrentV1 {
            info_hash: InfoHashV1::from_bytes([1; 20]),
            size: 1,
            files: vec![ReleaseFile {
                path: "../escape".to_owned(),
                size: 1,
            }],
            trackers: Vec::new(),
        };
        assert!(
            ReleaseV1::new(
                publisher.clone(),
                1,
                "title".to_owned(),
                String::new(),
                base.clone(),
                Vec::new()
            )
            .is_err()
        );

        let mut torrent = base;
        torrent.files[0].path = "NUL.txt".to_owned();
        assert!(
            ReleaseV1::new(
                publisher.clone(),
                1,
                "title".to_owned(),
                String::new(),
                torrent.clone(),
                Vec::new()
            )
            .is_err()
        );
        torrent.files[0].path = "safe.bin".to_owned();
        torrent.trackers = vec!["file:///etc/passwd".to_owned()];
        assert!(
            ReleaseV1::new(
                publisher.clone(),
                1,
                "title".to_owned(),
                String::new(),
                torrent.clone(),
                Vec::new()
            )
            .is_err()
        );
        torrent.trackers.clear();
        assert!(
            ReleaseV1::new(
                publisher,
                1,
                "title".to_owned(),
                String::new(),
                torrent,
                vec!["Unnormalized".to_owned()]
            )
            .is_err()
        );
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    fn publisher(seed: u8) -> PublisherId {
        PublisherId::new(pubky::Keypair::from_secret(&[seed; 32]).public_key())
    }

    fn object(path: &str, bytes: &[u8]) -> ManifestObjectV1 {
        ManifestObjectV1::from_bytes(path.to_owned(), bytes)
    }

    fn sample_objects() -> Vec<ManifestObjectV1> {
        vec![
            object("README.md", b"readme"),
            object("data/alpha.bin", b"alpha"),
            object("data/beta.bin", b"beta"),
            object("data/sub/gamma.bin", b"gamma"),
            object("index.json", b"{}"),
            object("z-last.txt", b"z"),
        ]
    }

    fn manifest() -> DatasetManifestV1 {
        DatasetManifestV1::new(publisher(7), 1_786_000_000_000, sample_objects()).unwrap()
    }

    #[test]
    fn digest_hex_round_trip_and_rejection() {
        let digest = ObjectDigest::of(b"payload");
        let encoded = serde_json::to_string(&digest).unwrap();
        assert_eq!(
            serde_json::from_str::<ObjectDigest>(&encoded).unwrap(),
            digest
        );
        assert_eq!(digest.to_string().parse::<ObjectDigest>().unwrap(), digest);
        assert!(matches!(
            "ab".parse::<ObjectDigest>(),
            Err(Error::InvalidObjectDigest(_))
        ));
        assert!(matches!(
            "zz".repeat(32).parse::<ObjectDigest>(),
            Err(Error::InvalidObjectDigest(_))
        ));

        let manifest_digest = manifest().digest();
        assert_eq!(
            manifest_digest
                .to_string()
                .parse::<ManifestDigest>()
                .unwrap(),
            manifest_digest
        );
        assert!(matches!(
            "00".parse::<ManifestDigest>(),
            Err(Error::InvalidManifestDigest(_))
        ));
    }

    #[test]
    fn canonical_bytes_and_digest_are_order_independent() {
        let baseline = manifest();
        let baseline_bytes = baseline.to_canonical_bytes();
        assert!(baseline_bytes.starts_with(b"pubky.swarm/dataset-manifest/v1\0"));

        let original = sample_objects();
        let count = original.len();
        for rotation in 0..count {
            let mut shuffled = original.clone();
            shuffled.rotate_left(rotation);
            if rotation % 2 == 1 {
                shuffled.reverse();
            }
            let rebuilt =
                DatasetManifestV1::new(publisher(7), 1_786_000_000_000, shuffled).unwrap();
            assert_eq!(rebuilt.to_canonical_bytes(), baseline_bytes);
            assert_eq!(rebuilt.digest(), baseline.digest());
            assert_eq!(
                serde_json::to_string(&rebuilt).unwrap(),
                serde_json::to_string(&baseline).unwrap()
            );
        }
    }

    #[test]
    fn canonical_bytes_round_trip_and_strict_parsing() {
        let manifest = manifest();
        let bytes = manifest.to_canonical_bytes();
        assert_eq!(
            DatasetManifestV1::from_canonical_bytes(&bytes).unwrap(),
            manifest
        );

        for truncated in [0, 8, 40, 48, bytes.len() - 1] {
            assert!(matches!(
                DatasetManifestV1::from_canonical_bytes(&bytes[..truncated]),
                Err(Error::InvalidCanonicalBytes(_))
            ));
        }
        let mut trailed = bytes.clone();
        trailed.push(0);
        assert!(matches!(
            DatasetManifestV1::from_canonical_bytes(&trailed),
            Err(Error::InvalidCanonicalBytes("trailing bytes"))
        ));
        let mut wrong_prefix = bytes;
        wrong_prefix[0] = b'X';
        assert!(matches!(
            DatasetManifestV1::from_canonical_bytes(&wrong_prefix),
            Err(Error::InvalidCanonicalBytes("unsupported domain prefix"))
        ));
    }

    #[test]
    fn canonical_and_wire_reject_unsorted_objects() {
        let mut objects = sample_objects();
        objects.swap(0, 1);
        let unsorted = DatasetManifestV1 {
            schema: DATASET_MANIFEST_SCHEMA.to_owned(),
            version: DATASET_MANIFEST_VERSION,
            publisher: publisher(7),
            created_at: 1_786_000_000_000,
            objects,
        };
        assert!(matches!(
            DatasetManifestV1::from_canonical_bytes(&unsorted.to_canonical_bytes()),
            Err(Error::InvalidField {
                field: "objects",
                ..
            })
        ));
        let json = serde_json::to_string(&unsorted).unwrap();
        assert!(serde_json::from_str::<DatasetManifestV1>(&json).is_err());
    }

    #[test]
    fn json_round_trip_and_schema_version_checks() {
        let manifest = manifest();
        let json = serde_json::to_string_pretty(&manifest).unwrap();
        assert_eq!(
            serde_json::from_str::<DatasetManifestV1>(&json).unwrap(),
            manifest
        );
        assert!(json.contains("\"schema\": \"pubky.swarm/dataset-manifest\""));

        let mut value = serde_json::to_value(&manifest).unwrap();
        value["version"] = serde_json::Value::from(2);
        assert!(serde_json::from_value::<DatasetManifestV1>(value).is_err());

        let mut value = serde_json::to_value(&manifest).unwrap();
        value["created_at"] = serde_json::Value::from(0);
        assert!(serde_json::from_value::<DatasetManifestV1>(value).is_err());
    }

    #[test]
    fn rejects_duplicate_and_prefix_colliding_paths() {
        for paths in [
            vec!["a.txt", "a.txt"],
            vec!["dir", "dir/file.txt"],
            vec!["dir/file.txt", "dir"],
        ] {
            let objects = paths.into_iter().map(|path| object(path, b"x")).collect();
            assert!(matches!(
                DatasetManifestV1::new(publisher(1), 1, objects),
                Err(Error::InvalidField {
                    field: "objects",
                    ..
                })
            ));
        }
        // Sibling directory prefix is not a collision.
        let objects = vec![object("dir", b"x"), object("dir-other/f.txt", b"y")];
        assert!(DatasetManifestV1::new(publisher(1), 1, objects).is_ok());
    }

    #[test]
    fn rejects_non_portable_and_traversal_paths() {
        let overlong_component = "a".repeat(256);
        let overlong_path = format!("{}/{}", "d".repeat(4_000), "f");
        let too_deep = (0..65).map(|_| "d").collect::<Vec<_>>().join("/");
        for path in [
            "../escape".to_owned(),
            "a/../b".to_owned(),
            "/absolute".to_owned(),
            "a//b".to_owned(),
            "a/".to_owned(),
            "./a".to_owned(),
            "back\\slash".to_owned(),
            "NUL".to_owned(),
            "con.txt".to_owned(),
            "trailing.".to_owned(),
            "trailing ".to_owned(),
            String::new(),
            overlong_component,
            overlong_path,
            too_deep,
        ] {
            assert!(
                matches!(
                    DatasetManifestV1::new(publisher(1), 1, vec![object(&path, b"x")]),
                    Err(Error::InvalidPath { .. })
                ),
                "path {path:?} must be rejected"
            );
        }
    }

    #[test]
    fn enforces_object_count_resource_limit() {
        let at_limit: Vec<_> = (0..MAX_MANIFEST_OBJECTS)
            .map(|index| object(&format!("f{index:06}"), b""))
            .collect();
        assert!(DatasetManifestV1::new(publisher(1), 1, at_limit).is_ok());

        let mut over_limit: Vec<_> = (0..=MAX_MANIFEST_OBJECTS)
            .map(|index| object(&format!("f{index:06}"), b""))
            .collect();
        over_limit.sort_by(|left, right| left.path.cmp(&right.path));
        assert!(matches!(
            DatasetManifestV1::new(publisher(1), 1, over_limit),
            Err(Error::InvalidField {
                field: "objects",
                ..
            })
        ));
    }

    #[test]
    fn verify_object_detects_changed_bytes_and_unknown_paths() {
        let manifest = manifest();
        assert_eq!(
            manifest
                .verify_object("data/alpha.bin", b"alpha")
                .unwrap()
                .path,
            "data/alpha.bin"
        );

        let mut tampered = b"alpha".to_vec();
        tampered[0] ^= 0x01;
        assert!(matches!(
            manifest.verify_object("data/alpha.bin", &tampered),
            Err(Error::ObjectMismatch { .. })
        ));
        assert!(matches!(
            manifest.verify_object("data/alpha.bin", b"alpha!"),
            Err(Error::ObjectMismatch { .. })
        ));
        assert!(matches!(
            manifest.verify_object("missing.bin", b""),
            Err(Error::UnknownObject { .. })
        ));
    }

    #[test]
    fn empty_object_and_empty_dataset_are_valid() {
        let empty_object =
            DatasetManifestV1::new(publisher(2), 5, vec![object("empty.bin", b"")]).unwrap();
        assert!(empty_object.verify_object("empty.bin", b"").is_ok());
        assert!(empty_object.verify_object("empty.bin", b"x").is_err());

        let empty = DatasetManifestV1::new(publisher(2), 5, Vec::new()).unwrap();
        assert!(empty.objects().is_empty());
        assert_eq!(
            DatasetManifestV1::from_canonical_bytes(&empty.to_canonical_bytes()).unwrap(),
            empty
        );
    }
}
