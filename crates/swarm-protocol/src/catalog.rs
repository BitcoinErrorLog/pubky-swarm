//! Portable catalog, import-provenance, and social-claim protocol types.

use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Error, InfoHashV1, PublisherId, invalid_field, validate_portable_path, validate_text};

/// Maximum number of subjects in a collection or blocklist.
pub const MAX_ARTIFACT_SUBJECTS: usize = 10_000;
/// Maximum number of import observations accepted in one mapping result.
pub const MAX_IMPORT_OBSERVATIONS: usize = 10_000;

const TAG_CLAIM_SCHEMA: &str = "pubky.swarm/tag-claim";
const COLLECTION_SCHEMA: &str = "pubky.swarm/collection";
const TOMBSTONE_SCHEMA: &str = "pubky.swarm/tombstone";
const BLOCKLIST_SCHEMA: &str = "pubky.swarm/blocklist";
const MODERATION_SCHEMA: &str = "pubky.swarm/moderation-decision";
const ARTIFACT_VERSION: u16 = 1;

/// BEP 52 v2 SHA-256 multihash used by a `btmh` magnet topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Btmh([u8; 34]);

impl Btmh {
    /// Construct a canonical SHA2-256 multihash from its 32-byte digest.
    #[must_use]
    pub const fn from_sha256(digest: [u8; 32]) -> Self {
        let mut bytes = [0_u8; 34];
        bytes[0] = 0x12;
        bytes[1] = 0x20;
        let mut index = 0;
        while index < digest.len() {
            bytes[index + 2] = digest[index];
            index += 1;
        }
        Self(bytes)
    }

    /// Return the SHA2-256 digest carried by the multihash.
    #[must_use]
    pub fn sha256(&self) -> [u8; 32] {
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&self.0[2..]);
        digest
    }

    /// Return the canonical multihash bytes (`0x12 0x20` plus digest).
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 34] {
        &self.0
    }
}

impl Display for Btmh {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for Btmh {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 68 {
            return Err(invalid_field(
                "btmh",
                format!(
                    "expected 68 lowercase hexadecimal characters, got {}",
                    value.len()
                ),
            ));
        }
        if value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid_field("btmh", "must be lowercase hexadecimal"));
        }
        let bytes = hex::decode(value).map_err(|error| invalid_field("btmh", error.to_string()))?;
        let bytes: [u8; 34] = bytes
            .try_into()
            .map_err(|_| invalid_field("btmh", "expected 34 multihash bytes"))?;
        if bytes[..2] != [0x12, 0x20] {
            return Err(invalid_field(
                "btmh",
                "only canonical SHA2-256 multihashes are supported",
            ));
        }
        Ok(Self(bytes))
    }
}

impl Serialize for Btmh {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Btmh {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Normalized `BitTorrent` identity suitable for portable catalog references.
///
/// Hybrid identity can only be created through [`Self::verified_hybrid`].
/// Calling code must verify that both hashes were computed from the same
/// authenticated hybrid info dictionary before constructing that variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TorrentRef {
    /// BEP 9 v1 info hash (`btih`).
    Btih(InfoHashV1),
    /// BEP 52 v2 SHA2-256 multihash (`btmh`).
    Btmh(Btmh),
    /// Both identities, verified to describe the same hybrid info dictionary.
    VerifiedHybrid {
        /// v1 identity.
        btih: InfoHashV1,
        /// v2 identity.
        btmh: Btmh,
    },
}

impl TorrentRef {
    /// Construct a normalized v1 reference.
    #[must_use]
    pub const fn btih(info_hash: InfoHashV1) -> Self {
        Self::Btih(info_hash)
    }

    /// Construct a normalized v2 reference.
    #[must_use]
    pub const fn btmh(multihash: Btmh) -> Self {
        Self::Btmh(multihash)
    }

    /// Construct a hybrid reference after the caller verifies both hashes
    /// originate from the same authenticated info dictionary.
    #[must_use]
    pub const fn verified_hybrid(v1_hash: InfoHashV1, v2_hash: Btmh) -> Self {
        Self::VerifiedHybrid {
            btih: v1_hash,
            btmh: v2_hash,
        }
    }

    /// Return the v1 hash when one is present.
    #[must_use]
    pub const fn v1(&self) -> Option<InfoHashV1> {
        match self {
            Self::Btih(hash) | Self::VerifiedHybrid { btih: hash, .. } => Some(*hash),
            Self::Btmh(_) => None,
        }
    }

    /// Return the v2 multihash when one is present.
    #[must_use]
    pub const fn v2(&self) -> Option<Btmh> {
        match self {
            Self::Btmh(hash) | Self::VerifiedHybrid { btmh: hash, .. } => Some(*hash),
            Self::Btih(_) => None,
        }
    }
}

impl Display for TorrentRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Btih(hash) => write!(formatter, "btih:{hash}"),
            Self::Btmh(hash) => write!(formatter, "btmh:{hash}"),
            Self::VerifiedHybrid { btih, btmh } => {
                write!(formatter, "hybrid:btih:{btih}:btmh:{btmh}")
            }
        }
    }
}

impl FromStr for TorrentRef {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Some(value) = value.strip_prefix("btih:") {
            return parse_normalized_btih(value).map(Self::Btih);
        }
        if let Some(value) = value.strip_prefix("btmh:") {
            return value.parse().map(Self::Btmh);
        }
        if let Some(value) = value.strip_prefix("hybrid:btih:") {
            let (v1_hash, v2_hash) = value
                .split_once(":btmh:")
                .ok_or_else(|| invalid_field("torrent_ref", "malformed hybrid reference"))?;
            return Ok(Self::verified_hybrid(
                parse_normalized_btih(v1_hash)?,
                v2_hash.parse()?,
            ));
        }
        Err(invalid_field(
            "torrent_ref",
            "expected btih:, btmh:, or hybrid:btih:...:btmh:...",
        ))
    }
}

impl Serialize for TorrentRef {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TorrentRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Absolute, normalized, credential-free URI used as a catalog subject.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CanonicalUri(String);

impl CanonicalUri {
    /// Parse an absolute URI and require the input to already be normalized.
    ///
    /// # Errors
    ///
    /// Rejects relative, credential-bearing, fragmented, or non-canonical
    /// values.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.len() > 2_048 {
            return Err(invalid_field("uri", "maximum is 2048 bytes"));
        }
        let parsed =
            url::Url::parse(&value).map_err(|error| invalid_field("uri", error.to_string()))?;
        if parsed.cannot_be_a_base() && parsed.scheme() != "urn" {
            return Err(invalid_field(
                "uri",
                "opaque URIs are only supported for urn:",
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(invalid_field("uri", "credentials are not allowed"));
        }
        if parsed.fragment().is_some() {
            return Err(invalid_field("uri", "fragments are not allowed"));
        }
        if parsed.to_string() != value {
            return Err(invalid_field(
                "uri",
                format!("URI is not canonical; expected {}", parsed.as_str()),
            ));
        }
        Ok(Self(value))
    }

    /// Borrow the canonical URI string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for CanonicalUri {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for CanonicalUri {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value.to_owned())
    }
}

impl Serialize for CanonicalUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CanonicalUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Subject addressed by a portable social artifact.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SubjectRef {
    /// `BitTorrent` payload identity.
    Torrent(TorrentRef),
    /// Canonical absolute URI.
    Uri(CanonicalUri),
}

impl Display for SubjectRef {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Torrent(reference) => write!(formatter, "torrent:{reference}"),
            Self::Uri(uri) => write!(formatter, "uri:{uri}"),
        }
    }
}

/// Trust boundary of imported metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImportProvenance {
    /// Value authenticated inside the torrent info dictionary.
    InfoAuthenticated,
    /// Unauthenticated top-level torrent metadata.
    TopLevelHint,
    /// Unauthenticated magnet-query hint.
    MagnetHint,
    /// Claim made by an identified indexer.
    IndexerClaim {
        /// Identity accountable for the imported claim.
        issuer: PublisherId,
    },
    /// Metadata observed from a local client.
    LocalClient,
}

/// Attribution retained by a published artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceAttribution {
    /// Authored directly by the artifact issuer.
    Direct,
    /// Imported from a canonical source URI.
    Uri {
        /// Source URI.
        uri: CanonicalUri,
    },
    /// Derived from imported metadata with its trust boundary retained.
    Import {
        /// Import trust boundary.
        provenance: ImportProvenance,
        /// Optional canonical location from which the metadata was fetched.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        locator: Option<CanonicalUri>,
    },
}

/// Broad media class derived from authenticated file names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedContentType {
    /// Audio media.
    Audio,
    /// Video media.
    Video,
    /// Still image.
    Image,
    /// Human-readable document.
    Document,
    /// Compressed archive.
    Archive,
    /// Executable or software package.
    Software,
    /// Type is not recognized.
    Other,
}

/// Source syntax that supplied a tracker endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackerSource {
    /// Torrent `announce`.
    Announce,
    /// Torrent `announce-list`.
    AnnounceList,
    /// Magnet `tr`.
    Magnet,
    /// Indexer response field.
    Indexer,
    /// Local client configuration.
    LocalClient,
}

/// BEP 38 relation-hint meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelationKind {
    /// Similar torrent relation.
    Similar,
    /// Collection membership relation.
    Collection,
}

/// External suggestion namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionNamespace {
    /// `kt` convention.
    Kt,
    /// Torznab attribute.
    Torznab,
    /// qBittorrent metadata.
    QBittorrent,
}

/// A typed metadata value carried by an import observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ObservationValue {
    /// Display-title hint from authenticated `name` or magnet `dn`.
    Title {
        /// Bounded display-title value.
        value: String,
    },
    /// Torrent privacy bit from the authenticated info dictionary.
    Privacy {
        /// Authenticated privacy bit.
        private: bool,
    },
    /// Lowercase file extension derived from an authenticated file path.
    FileExtension {
        /// Normalized extension without a leading dot.
        value: String,
    },
    /// Broad file type derived from an authenticated file extension.
    FileType {
        /// Derived broad content type.
        value: DerivedContentType,
    },
    /// Untrusted top-level comment.
    Comment {
        /// Bounded comment hint.
        value: String,
    },
    /// Untrusted top-level creator string.
    Creator {
        /// Bounded creator hint.
        value: String,
    },
    /// Untrusted top-level creation time in Unix seconds.
    CreationDate {
        /// Positive Unix timestamp supplied by the top-level field.
        unix_seconds: u64,
    },
    /// Validated tracker endpoint and syntax source.
    Tracker {
        /// Credential-free HTTP(S) or UDP URL.
        url: String,
        /// Field or system that supplied it.
        source: TrackerSource,
    },
    /// BEP 38 relation hint.
    Relation {
        /// Relation semantics.
        relation: RelationKind,
        /// Related torrent.
        target: TorrentRef,
    },
    /// Issuer-attributed de-facto metadata suggestion.
    Suggestion {
        /// Convention supplying the value.
        namespace: SuggestionNamespace,
        /// Bounded field name.
        field: String,
        /// Bounded suggested value.
        value: String,
        /// Identity accountable for the suggestion.
        issuer: PublisherId,
    },
}

/// One provenance-preserving import observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportObservation {
    /// Torrent being described.
    subject: TorrentRef,
    /// Time the importer observed this value, in Unix milliseconds.
    observed_at: u64,
    /// Authentication and attribution boundary.
    provenance: ImportProvenance,
    /// Typed observation.
    value: ObservationValue,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ImportObservationWire {
    subject: TorrentRef,
    observed_at: u64,
    provenance: ImportProvenance,
    value: ObservationValue,
}

impl ImportObservation {
    fn new(
        subject: TorrentRef,
        observed_at: u64,
        provenance: ImportProvenance,
        value: ObservationValue,
    ) -> Result<Self, Error> {
        if observed_at == 0 {
            return Err(invalid_field("observed_at", "must be positive"));
        }
        let observation = Self {
            subject,
            observed_at,
            provenance,
            value,
        };
        observation.validate()?;
        Ok(observation)
    }

    /// Torrent being described.
    #[must_use]
    pub const fn subject(&self) -> TorrentRef {
        self.subject
    }

    /// Observation time in Unix milliseconds.
    #[must_use]
    pub const fn observed_at(&self) -> u64 {
        self.observed_at
    }

    /// Authentication and attribution boundary.
    #[must_use]
    pub const fn provenance(&self) -> &ImportProvenance {
        &self.provenance
    }

    /// Typed observed value.
    #[must_use]
    pub const fn value(&self) -> &ObservationValue {
        &self.value
    }

    fn validate(&self) -> Result<(), Error> {
        if self.observed_at == 0 {
            return Err(invalid_field("observed_at", "must be positive"));
        }
        match (&self.provenance, &self.value) {
            (
                ImportProvenance::InfoAuthenticated | ImportProvenance::MagnetHint,
                ObservationValue::Title { value },
            ) => validate_text("title", value, 1, 200),
            (
                ImportProvenance::InfoAuthenticated,
                ObservationValue::Privacy { .. }
                | ObservationValue::FileExtension { .. }
                | ObservationValue::FileType { .. },
            ) => {
                if let ObservationValue::FileExtension { value } = &self.value {
                    validate_extension(value)?;
                }
                Ok(())
            }
            (
                ImportProvenance::TopLevelHint,
                ObservationValue::Comment { value } | ObservationValue::Creator { value },
            ) => validate_text("top_level_hint", value, 1, 1_024),
            (ImportProvenance::TopLevelHint, ObservationValue::CreationDate { unix_seconds })
                if *unix_seconds > 0 =>
            {
                Ok(())
            }
            (
                ImportProvenance::TopLevelHint
                | ImportProvenance::MagnetHint
                | ImportProvenance::IndexerClaim { .. }
                | ImportProvenance::LocalClient,
                ObservationValue::Tracker { url, source },
            ) => {
                validate_tracker_url(url)?;
                let matches_boundary = matches!(
                    (&self.provenance, source),
                    (
                        ImportProvenance::TopLevelHint,
                        TrackerSource::Announce | TrackerSource::AnnounceList
                    ) | (ImportProvenance::MagnetHint, TrackerSource::Magnet)
                        | (
                            ImportProvenance::IndexerClaim { .. },
                            TrackerSource::Indexer
                        )
                        | (ImportProvenance::LocalClient, TrackerSource::LocalClient)
                );
                if matches_boundary {
                    Ok(())
                } else {
                    Err(invalid_field(
                        "tracker.source",
                        "source does not match provenance boundary",
                    ))
                }
            }
            (ImportProvenance::TopLevelHint, ObservationValue::Relation { target, .. })
                if target != &self.subject =>
            {
                Ok(())
            }
            (
                ImportProvenance::IndexerClaim {
                    issuer: provenance_issuer,
                },
                ObservationValue::Suggestion {
                    field,
                    value,
                    issuer,
                    ..
                },
            ) if provenance_issuer == issuer => {
                validate_key("suggestion.field", field, 64)?;
                validate_text("suggestion.value", value, 1, 512)
            }
            (
                ImportProvenance::LocalClient,
                ObservationValue::Suggestion {
                    namespace: SuggestionNamespace::QBittorrent,
                    field,
                    value,
                    ..
                },
            ) => {
                validate_key("suggestion.field", field, 64)?;
                validate_text("suggestion.value", value, 1, 512)
            }
            _ => Err(invalid_field(
                "observation",
                "value is not valid for its provenance boundary",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for ImportObservation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ImportObservationWire::deserialize(deserializer)?;
        let observation = Self {
            subject: wire.subject,
            observed_at: wire.observed_at,
            provenance: wire.provenance,
            value: wire.value,
        };
        observation.validate().map_err(serde::de::Error::custom)?;
        Ok(observation)
    }
}

/// Validated observation set eligible for ingestion into a public catalog.
///
/// Construction requires an authenticated, non-private info-dictionary
/// observation. This prevents magnet or top-level hints from bypassing the
/// private-torrent boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PublicCatalogImport {
    subject: TorrentRef,
    observations: Vec<ImportObservation>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicCatalogImportWire {
    subject: TorrentRef,
    observations: Vec<ImportObservation>,
}

impl PublicCatalogImport {
    /// Validate observations for public catalog ingestion.
    ///
    /// # Errors
    ///
    /// Rejects empty or oversized sets, mixed subjects, missing authenticated
    /// privacy metadata, or a private torrent.
    pub fn new(subject: TorrentRef, observations: Vec<ImportObservation>) -> Result<Self, Error> {
        let value = Self {
            subject,
            observations,
        };
        value.validate()?;
        Ok(value)
    }

    fn validate(&self) -> Result<(), Error> {
        if self.observations.is_empty() || self.observations.len() > MAX_IMPORT_OBSERVATIONS {
            return Err(invalid_field(
                "observations",
                format!("expected 1..={MAX_IMPORT_OBSERVATIONS} observations"),
            ));
        }
        if self
            .observations
            .iter()
            .any(|observation| observation.subject != self.subject)
        {
            return Err(invalid_field(
                "observations",
                "all observations must describe the import subject",
            ));
        }
        let privacy = self.observations.iter().filter_map(|observation| {
            match (&observation.provenance, &observation.value) {
                (ImportProvenance::InfoAuthenticated, ObservationValue::Privacy { private }) => {
                    Some(*private)
                }
                _ => None,
            }
        });
        let mut saw_public = false;
        for private in privacy {
            ensure_public_catalog_eligible(private)?;
            saw_public = true;
        }
        if !saw_public {
            return Err(invalid_field(
                "observations",
                "authenticated privacy metadata is required for public cataloging",
            ));
        }
        Ok(())
    }

    /// Imported torrent identity.
    #[must_use]
    pub const fn subject(&self) -> TorrentRef {
        self.subject
    }

    /// Validated provenance-preserving observations.
    #[must_use]
    pub fn observations(&self) -> &[ImportObservation] {
        &self.observations
    }
}

impl<'de> Deserialize<'de> for PublicCatalogImport {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PublicCatalogImportWire::deserialize(deserializer)?;
        Self::new(wire.subject, wire.observations).map_err(serde::de::Error::custom)
    }
}

/// Map authenticated info-dictionary `name` to a title observation.
///
/// # Errors
///
/// Rejects an invalid timestamp or title bound.
pub fn map_info_name(
    subject: TorrentRef,
    observed_at: u64,
    name: impl Into<String>,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::InfoAuthenticated,
        ObservationValue::Title { value: name.into() },
    )
}

/// Map magnet `dn` to an unauthenticated title hint.
///
/// # Errors
///
/// Rejects an invalid timestamp or title bound.
pub fn map_magnet_dn(
    subject: TorrentRef,
    observed_at: u64,
    display_name: impl Into<String>,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::MagnetHint,
        ObservationValue::Title {
            value: display_name.into(),
        },
    )
}

/// Map the authenticated info-dictionary `private` bit.
///
/// # Errors
///
/// Rejects a zero observation timestamp.
pub fn map_private(
    subject: TorrentRef,
    observed_at: u64,
    private: bool,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::InfoAuthenticated,
        ObservationValue::Privacy { private },
    )
}

/// Reject attempts to publish a private torrent into the public catalog.
///
/// # Errors
///
/// Returns a validation error whenever the authenticated `private` bit is set.
pub fn ensure_public_catalog_eligible(private: bool) -> Result<(), Error> {
    if private {
        Err(invalid_field(
            "torrent.private",
            "private torrents cannot be published to the public catalog",
        ))
    } else {
        Ok(())
    }
}

/// Derive normalized extension and broad type observations from an
/// authenticated torrent file path.
///
/// # Errors
///
/// Rejects a zero timestamp or a non-portable extension.
pub fn derive_file_observations(
    subject: TorrentRef,
    observed_at: u64,
    path: &str,
) -> Result<Vec<ImportObservation>, Error> {
    validate_portable_path(path)?;
    let extension = path
        .rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .filter(|extension| !extension.is_empty());
    let Some(extension) = extension else {
        return Ok(Vec::new());
    };
    validate_extension(&extension)?;
    let content_type = content_type_for_extension(&extension);
    Ok(vec![
        ImportObservation::new(
            subject,
            observed_at,
            ImportProvenance::InfoAuthenticated,
            ObservationValue::FileExtension {
                value: extension.clone(),
            },
        )?,
        ImportObservation::new(
            subject,
            observed_at,
            ImportProvenance::InfoAuthenticated,
            ObservationValue::FileType {
                value: content_type,
            },
        )?,
    ])
}

/// Map an untrusted top-level `comment` field.
///
/// # Errors
///
/// Rejects an invalid timestamp or comment bound.
pub fn map_top_level_comment(
    subject: TorrentRef,
    observed_at: u64,
    value: impl Into<String>,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::TopLevelHint,
        ObservationValue::Comment {
            value: value.into(),
        },
    )
}

/// Map an untrusted top-level `created by` field.
///
/// # Errors
///
/// Rejects an invalid timestamp or creator bound.
pub fn map_top_level_creator(
    subject: TorrentRef,
    observed_at: u64,
    value: impl Into<String>,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::TopLevelHint,
        ObservationValue::Creator {
            value: value.into(),
        },
    )
}

/// Map an untrusted top-level `creation date` field.
///
/// # Errors
///
/// Rejects zero observation or creation timestamps.
pub fn map_top_level_creation_date(
    subject: TorrentRef,
    observed_at: u64,
    unix_seconds: u64,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::TopLevelHint,
        ObservationValue::CreationDate { unix_seconds },
    )
}

/// Map and validate a tracker endpoint while retaining its source syntax.
///
/// # Errors
///
/// Rejects credential-bearing or unsupported tracker URLs, zero timestamps,
/// and indexer sources without an issuer.
pub fn map_tracker(
    subject: TorrentRef,
    observed_at: u64,
    url: impl Into<String>,
    source: TrackerSource,
    indexer: Option<PublisherId>,
) -> Result<ImportObservation, Error> {
    let provenance = match source {
        TrackerSource::Announce | TrackerSource::AnnounceList => ImportProvenance::TopLevelHint,
        TrackerSource::Magnet => ImportProvenance::MagnetHint,
        TrackerSource::Indexer => ImportProvenance::IndexerClaim {
            issuer: indexer.ok_or_else(|| {
                invalid_field("tracker.indexer", "indexer source requires an issuer")
            })?,
        },
        TrackerSource::LocalClient => ImportProvenance::LocalClient,
    };
    ImportObservation::new(
        subject,
        observed_at,
        provenance,
        ObservationValue::Tracker {
            url: url.into(),
            source,
        },
    )
}

/// Map an untrusted BEP 38 relation hint.
///
/// # Errors
///
/// Rejects a zero timestamp or self-relation.
pub fn map_bep38_relation(
    subject: TorrentRef,
    observed_at: u64,
    relation: RelationKind,
    target: TorrentRef,
) -> Result<ImportObservation, Error> {
    ImportObservation::new(
        subject,
        observed_at,
        ImportProvenance::TopLevelHint,
        ObservationValue::Relation { relation, target },
    )
}

/// Map a `kt`, Torznab, or qBittorrent value as an issuer-attributed
/// suggestion, never as authenticated release metadata.
///
/// # Errors
///
/// Rejects invalid timestamps, field names, or value bounds.
pub fn map_issuer_suggestion(
    subject: TorrentRef,
    observed_at: u64,
    issuer: PublisherId,
    namespace: SuggestionNamespace,
    field: impl Into<String>,
    value: impl Into<String>,
) -> Result<ImportObservation, Error> {
    let provenance = if namespace == SuggestionNamespace::QBittorrent {
        ImportProvenance::LocalClient
    } else {
        ImportProvenance::IndexerClaim {
            issuer: issuer.clone(),
        }
    };
    ImportObservation::new(
        subject,
        observed_at,
        provenance,
        ObservationValue::Suggestion {
            namespace,
            field: field.into(),
            value: value.into(),
            issuer,
        },
    )
}

/// Deterministic BLAKE3 identifier for a catalog artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArtifactId([u8; 32]);

impl ArtifactId {
    fn derive(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Return the raw digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Display for ArtifactId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl FromStr for ArtifactId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64
            || value
                .bytes()
                .any(|byte| !byte.is_ascii_digit() && !matches!(byte, b'a'..=b'f'))
        {
            return Err(invalid_field(
                "artifact_id",
                "expected 64 lowercase hexadecimal characters",
            ));
        }
        let bytes =
            hex::decode(value).map_err(|error| invalid_field("artifact_id", error.to_string()))?;
        Ok(Self(bytes.try_into().map_err(|_| {
            invalid_field("artifact_id", "expected 32 bytes")
        })?))
    }
}

impl Serialize for ArtifactId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ArtifactId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// Tag-claim operation. Claims never mutate tags embedded in a release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagOperation {
    /// Assert a tag.
    Add,
    /// Withdraw this issuer's tag assertion.
    Remove,
}

/// Versioned issuer-attributed tag claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TagClaimV1 {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    tag: String,
    operation: TagOperation,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TagClaimWire {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    tag: String,
    operation: TagOperation,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

impl TagClaimV1 {
    /// Construct a validated tag claim.
    ///
    /// # Errors
    ///
    /// Rejects invalid tags, timestamps, revisions, or source attribution.
    pub fn new(
        issuer: PublisherId,
        subject: SubjectRef,
        tag: String,
        operation: TagOperation,
        created_at: u64,
        revision: u64,
        source: SourceAttribution,
    ) -> Result<Self, Error> {
        let mut claim = Self {
            schema: TAG_CLAIM_SCHEMA.to_owned(),
            version: ARTIFACT_VERSION,
            id: ArtifactId([0; 32]),
            issuer,
            subject,
            tag,
            operation,
            created_at,
            revision,
            source,
        };
        claim.validate()?;
        claim.id = ArtifactId::derive(&claim.to_canonical_bytes());
        Ok(claim)
    }

    fn validate(&self) -> Result<(), Error> {
        validate_header(
            &self.schema,
            TAG_CLAIM_SCHEMA,
            self.version,
            self.created_at,
            self.revision,
        )?;
        validate_tag(&self.tag)?;
        validate_source(&self.source)
    }

    /// Deterministic claim identifier.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Claim issuer.
    #[must_use]
    pub const fn issuer(&self) -> &PublisherId {
        &self.issuer
    }

    /// Claimed subject.
    #[must_use]
    pub const fn subject(&self) -> &SubjectRef {
        &self.subject
    }

    /// Claimed normalized tag.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// Claim operation.
    #[must_use]
    pub const fn operation(&self) -> TagOperation {
        self.operation
    }

    /// Claim creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Issuer authority revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Source attribution.
    #[must_use]
    pub const fn source(&self) -> &SourceAttribution {
        &self.source
    }

    /// Deterministic, platform-independent bytes used to derive [`Self::id`].
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = CanonicalWriter::new(b"pubky.swarm/tag-claim/v1\0");
        bytes.publisher(&self.issuer);
        bytes.subject(&self.subject);
        bytes.string(&self.tag);
        bytes.byte(match self.operation {
            TagOperation::Add => 0,
            TagOperation::Remove => 1,
        });
        bytes.u64(self.created_at);
        bytes.u64(self.revision);
        bytes.source(&self.source);
        bytes.finish()
    }
}

impl<'de> Deserialize<'de> for TagClaimV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TagClaimWire::deserialize(deserializer)?;
        let claim = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            issuer: wire.issuer,
            subject: wire.subject,
            tag: wire.tag,
            operation: wire.operation,
            created_at: wire.created_at,
            revision: wire.revision,
            source: wire.source,
        };
        claim.validate().map_err(serde::de::Error::custom)?;
        if claim.id != ArtifactId::derive(&claim.to_canonical_bytes()) {
            return Err(serde::de::Error::custom(
                "tag claim id does not match content",
            ));
        }
        Ok(claim)
    }
}

/// Versioned named set of canonical catalog subjects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CollectionV1 {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    title: String,
    description: String,
    subjects: Vec<SubjectRef>,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectionWire {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    title: String,
    #[serde(default)]
    description: String,
    subjects: Vec<SubjectRef>,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

impl CollectionV1 {
    /// Construct a collection, sorting subjects into canonical order.
    ///
    /// # Errors
    ///
    /// Rejects invalid fields, duplicate subjects, or resource-limit excess.
    pub fn new(
        issuer: PublisherId,
        title: String,
        description: String,
        mut subjects: Vec<SubjectRef>,
        created_at: u64,
        revision: u64,
        source: SourceAttribution,
    ) -> Result<Self, Error> {
        subjects.sort();
        let mut collection = Self {
            schema: COLLECTION_SCHEMA.to_owned(),
            version: ARTIFACT_VERSION,
            id: ArtifactId([0; 32]),
            issuer,
            title,
            description,
            subjects,
            created_at,
            revision,
            source,
        };
        collection.validate()?;
        collection.id = ArtifactId::derive(&collection.to_canonical_bytes());
        Ok(collection)
    }

    fn validate(&self) -> Result<(), Error> {
        validate_header(
            &self.schema,
            COLLECTION_SCHEMA,
            self.version,
            self.created_at,
            self.revision,
        )?;
        validate_text("title", &self.title, 1, 200)?;
        validate_text("description", &self.description, 0, 4_000)?;
        validate_subjects(&self.subjects)?;
        validate_source(&self.source)
    }

    /// Deterministic collection state identifier.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Collection issuer.
    #[must_use]
    pub const fn issuer(&self) -> &PublisherId {
        &self.issuer
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

    /// Canonically sorted subjects.
    #[must_use]
    pub fn subjects(&self) -> &[SubjectRef] {
        &self.subjects
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Issuer authority revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Source attribution.
    #[must_use]
    pub const fn source(&self) -> &SourceAttribution {
        &self.source
    }

    /// Deterministic collection bytes.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = CanonicalWriter::new(b"pubky.swarm/collection/v1\0");
        bytes.publisher(&self.issuer);
        bytes.string(&self.title);
        bytes.string(&self.description);
        bytes.subjects(&self.subjects);
        bytes.u64(self.created_at);
        bytes.u64(self.revision);
        bytes.source(&self.source);
        bytes.finish()
    }
}

impl<'de> Deserialize<'de> for CollectionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CollectionWire::deserialize(deserializer)?;
        let collection = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            issuer: wire.issuer,
            title: wire.title,
            description: wire.description,
            subjects: wire.subjects,
            created_at: wire.created_at,
            revision: wire.revision,
            source: wire.source,
        };
        collection.validate().map_err(serde::de::Error::custom)?;
        if collection.id != ArtifactId::derive(&collection.to_canonical_bytes()) {
            return Err(serde::de::Error::custom(
                "collection id does not match content",
            ));
        }
        Ok(collection)
    }
}

/// Versioned issuer tombstone for one catalog subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TombstoneV1 {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    reason: String,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TombstoneWire {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    #[serde(default)]
    reason: String,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

impl TombstoneV1 {
    /// Construct a validated tombstone.
    ///
    /// # Errors
    ///
    /// Rejects invalid bounds, timestamps, revisions, or attribution.
    pub fn new(
        issuer: PublisherId,
        subject: SubjectRef,
        reason: String,
        created_at: u64,
        revision: u64,
        source: SourceAttribution,
    ) -> Result<Self, Error> {
        let mut tombstone = Self {
            schema: TOMBSTONE_SCHEMA.to_owned(),
            version: ARTIFACT_VERSION,
            id: ArtifactId([0; 32]),
            issuer,
            subject,
            reason,
            created_at,
            revision,
            source,
        };
        tombstone.validate()?;
        tombstone.id = ArtifactId::derive(&tombstone.to_canonical_bytes());
        Ok(tombstone)
    }

    fn validate(&self) -> Result<(), Error> {
        validate_header(
            &self.schema,
            TOMBSTONE_SCHEMA,
            self.version,
            self.created_at,
            self.revision,
        )?;
        validate_text("reason", &self.reason, 0, 1_024)?;
        validate_source(&self.source)
    }

    /// Deterministic tombstone identifier.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Tombstone issuer.
    #[must_use]
    pub const fn issuer(&self) -> &PublisherId {
        &self.issuer
    }

    /// Removed subject.
    #[must_use]
    pub const fn subject(&self) -> &SubjectRef {
        &self.subject
    }

    /// Optional human-readable reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Issuer authority revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Source attribution.
    #[must_use]
    pub const fn source(&self) -> &SourceAttribution {
        &self.source
    }

    /// Deterministic tombstone bytes.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = CanonicalWriter::new(b"pubky.swarm/tombstone/v1\0");
        bytes.publisher(&self.issuer);
        bytes.subject(&self.subject);
        bytes.string(&self.reason);
        bytes.u64(self.created_at);
        bytes.u64(self.revision);
        bytes.source(&self.source);
        bytes.finish()
    }
}

impl<'de> Deserialize<'de> for TombstoneV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = TombstoneWire::deserialize(deserializer)?;
        let tombstone = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            issuer: wire.issuer,
            subject: wire.subject,
            reason: wire.reason,
            created_at: wire.created_at,
            revision: wire.revision,
            source: wire.source,
        };
        tombstone.validate().map_err(serde::de::Error::custom)?;
        if tombstone.id != ArtifactId::derive(&tombstone.to_canonical_bytes()) {
            return Err(serde::de::Error::custom(
                "tombstone id does not match content",
            ));
        }
        Ok(tombstone)
    }
}

/// Moderation outcome asserted by an issuer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModerationAction {
    /// Explicitly allow the subject.
    Allow,
    /// Hide or reject the subject.
    Block,
    /// Require human review before display.
    Review,
}

/// Versioned moderation decision for one subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModerationDecisionV1 {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    action: ModerationAction,
    reason: String,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModerationWire {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    subject: SubjectRef,
    action: ModerationAction,
    #[serde(default)]
    reason: String,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

impl ModerationDecisionV1 {
    /// Construct a validated moderation decision.
    ///
    /// # Errors
    ///
    /// Rejects invalid bounds, timestamps, revisions, or attribution.
    pub fn new(
        issuer: PublisherId,
        subject: SubjectRef,
        action: ModerationAction,
        reason: String,
        created_at: u64,
        revision: u64,
        source: SourceAttribution,
    ) -> Result<Self, Error> {
        let mut decision = Self {
            schema: MODERATION_SCHEMA.to_owned(),
            version: ARTIFACT_VERSION,
            id: ArtifactId([0; 32]),
            issuer,
            subject,
            action,
            reason,
            created_at,
            revision,
            source,
        };
        decision.validate()?;
        decision.id = ArtifactId::derive(&decision.to_canonical_bytes());
        Ok(decision)
    }

    fn validate(&self) -> Result<(), Error> {
        validate_header(
            &self.schema,
            MODERATION_SCHEMA,
            self.version,
            self.created_at,
            self.revision,
        )?;
        validate_text("reason", &self.reason, 0, 1_024)?;
        validate_source(&self.source)
    }

    /// Deterministic decision identifier.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Decision issuer.
    #[must_use]
    pub const fn issuer(&self) -> &PublisherId {
        &self.issuer
    }

    /// Moderated subject.
    #[must_use]
    pub const fn subject(&self) -> &SubjectRef {
        &self.subject
    }

    /// Moderation action.
    #[must_use]
    pub const fn action(&self) -> ModerationAction {
        self.action
    }

    /// Optional human-readable reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Issuer authority revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Source attribution.
    #[must_use]
    pub const fn source(&self) -> &SourceAttribution {
        &self.source
    }

    /// Deterministic moderation-decision bytes.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = CanonicalWriter::new(b"pubky.swarm/moderation-decision/v1\0");
        bytes.publisher(&self.issuer);
        bytes.subject(&self.subject);
        bytes.byte(match self.action {
            ModerationAction::Allow => 0,
            ModerationAction::Block => 1,
            ModerationAction::Review => 2,
        });
        bytes.string(&self.reason);
        bytes.u64(self.created_at);
        bytes.u64(self.revision);
        bytes.source(&self.source);
        bytes.finish()
    }
}

impl<'de> Deserialize<'de> for ModerationDecisionV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ModerationWire::deserialize(deserializer)?;
        let decision = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            issuer: wire.issuer,
            subject: wire.subject,
            action: wire.action,
            reason: wire.reason,
            created_at: wire.created_at,
            revision: wire.revision,
            source: wire.source,
        };
        decision.validate().map_err(serde::de::Error::custom)?;
        if decision.id != ArtifactId::derive(&decision.to_canonical_bytes()) {
            return Err(serde::de::Error::custom(
                "moderation decision id does not match content",
            ));
        }
        Ok(decision)
    }
}

/// Versioned issuer-maintained blocklist snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BlocklistV1 {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    title: String,
    subjects: Vec<SubjectRef>,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BlocklistWire {
    schema: String,
    version: u16,
    id: ArtifactId,
    issuer: PublisherId,
    title: String,
    subjects: Vec<SubjectRef>,
    created_at: u64,
    revision: u64,
    source: SourceAttribution,
}

impl BlocklistV1 {
    /// Construct a blocklist, sorting subjects into canonical order.
    ///
    /// # Errors
    ///
    /// Rejects invalid fields, duplicate subjects, or resource-limit excess.
    pub fn new(
        issuer: PublisherId,
        title: String,
        mut subjects: Vec<SubjectRef>,
        created_at: u64,
        revision: u64,
        source: SourceAttribution,
    ) -> Result<Self, Error> {
        subjects.sort();
        let mut blocklist = Self {
            schema: BLOCKLIST_SCHEMA.to_owned(),
            version: ARTIFACT_VERSION,
            id: ArtifactId([0; 32]),
            issuer,
            title,
            subjects,
            created_at,
            revision,
            source,
        };
        blocklist.validate()?;
        blocklist.id = ArtifactId::derive(&blocklist.to_canonical_bytes());
        Ok(blocklist)
    }

    fn validate(&self) -> Result<(), Error> {
        validate_header(
            &self.schema,
            BLOCKLIST_SCHEMA,
            self.version,
            self.created_at,
            self.revision,
        )?;
        validate_text("title", &self.title, 1, 200)?;
        validate_subjects(&self.subjects)?;
        validate_source(&self.source)
    }

    /// Deterministic blocklist state identifier.
    #[must_use]
    pub const fn id(&self) -> ArtifactId {
        self.id
    }

    /// Blocklist issuer.
    #[must_use]
    pub const fn issuer(&self) -> &PublisherId {
        &self.issuer
    }

    /// Display title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Canonically sorted blocked subjects.
    #[must_use]
    pub fn subjects(&self) -> &[SubjectRef] {
        &self.subjects
    }

    /// Creation time in Unix milliseconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Issuer authority revision.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Source attribution.
    #[must_use]
    pub const fn source(&self) -> &SourceAttribution {
        &self.source
    }

    /// Deterministic blocklist bytes.
    #[must_use]
    pub fn to_canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = CanonicalWriter::new(b"pubky.swarm/blocklist/v1\0");
        bytes.publisher(&self.issuer);
        bytes.string(&self.title);
        bytes.subjects(&self.subjects);
        bytes.u64(self.created_at);
        bytes.u64(self.revision);
        bytes.source(&self.source);
        bytes.finish()
    }
}

impl<'de> Deserialize<'de> for BlocklistV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = BlocklistWire::deserialize(deserializer)?;
        let blocklist = Self {
            schema: wire.schema,
            version: wire.version,
            id: wire.id,
            issuer: wire.issuer,
            title: wire.title,
            subjects: wire.subjects,
            created_at: wire.created_at,
            revision: wire.revision,
            source: wire.source,
        };
        blocklist.validate().map_err(serde::de::Error::custom)?;
        if blocklist.id != ArtifactId::derive(&blocklist.to_canonical_bytes()) {
            return Err(serde::de::Error::custom(
                "blocklist id does not match content",
            ));
        }
        Ok(blocklist)
    }
}

fn validate_header(
    schema: &str,
    expected_schema: &str,
    version: u16,
    created_at: u64,
    revision: u64,
) -> Result<(), Error> {
    if schema != expected_schema {
        return Err(invalid_field("schema", "unsupported schema"));
    }
    if version != ARTIFACT_VERSION {
        return Err(invalid_field("version", "unsupported version"));
    }
    if created_at == 0 {
        return Err(invalid_field("created_at", "must be positive"));
    }
    if revision == 0 {
        return Err(invalid_field("revision", "must be positive"));
    }
    Ok(())
}

fn validate_subjects(subjects: &[SubjectRef]) -> Result<(), Error> {
    if subjects.is_empty() || subjects.len() > MAX_ARTIFACT_SUBJECTS {
        return Err(invalid_field(
            "subjects",
            format!("expected 1..={MAX_ARTIFACT_SUBJECTS} subjects"),
        ));
    }
    if subjects.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(invalid_field(
            "subjects",
            "subjects must be sorted and unique",
        ));
    }
    Ok(())
}

fn validate_source(source: &SourceAttribution) -> Result<(), Error> {
    if let SourceAttribution::Import {
        provenance: ImportProvenance::IndexerClaim { issuer: _ },
        locator: None,
    } = source
    {
        return Err(invalid_field(
            "source.locator",
            "indexer imports require a canonical source locator",
        ));
    }
    Ok(())
}

fn validate_tag(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(invalid_field(
            "tag",
            "must be 1..=32 lowercase ASCII letters, digits, or hyphens",
        ));
    }
    Ok(())
}

fn validate_key(field: &'static str, value: &str, maximum: usize) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > maximum
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(invalid_field(
            field,
            format!("must be 1..={maximum} portable key characters"),
        ));
    }
    Ok(())
}

fn validate_extension(value: &str) -> Result<(), Error> {
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(invalid_field(
            "file_extension",
            "must be 1..=32 lowercase ASCII letters or digits",
        ));
    }
    Ok(())
}

pub(crate) fn validate_tracker_url(value: &str) -> Result<(), Error> {
    if value.len() > 2_048 {
        return Err(invalid_field("tracker", "URL exceeds 2048 bytes"));
    }
    let url =
        url::Url::parse(value).map_err(|error| invalid_field("tracker", error.to_string()))?;
    if !matches!(url.scheme(), "http" | "https" | "udp")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query_pairs().any(|(key, _)| {
            matches!(
                key.to_ascii_lowercase().as_str(),
                "api_key"
                    | "apikey"
                    | "auth"
                    | "authorization"
                    | "key"
                    | "passkey"
                    | "secret"
                    | "token"
            )
        })
    {
        return Err(invalid_field(
            "tracker",
            "only credential-free HTTP(S)/UDP tracker URLs without fragments are allowed",
        ));
    }
    Ok(())
}

fn parse_normalized_btih(value: &str) -> Result<InfoHashV1, Error> {
    if value.len() != 40
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !matches!(byte, b'a'..=b'f'))
    {
        return Err(invalid_field(
            "btih",
            "expected 40 lowercase hexadecimal characters",
        ));
    }
    value.parse()
}

fn content_type_for_extension(extension: &str) -> DerivedContentType {
    match extension {
        "aac" | "flac" | "m4a" | "mp3" | "ogg" | "opus" | "wav" => DerivedContentType::Audio,
        "avi" | "m4v" | "mkv" | "mov" | "mp4" | "webm" => DerivedContentType::Video,
        "avif" | "gif" | "heic" | "jpeg" | "jpg" | "png" | "svg" | "webp" => {
            DerivedContentType::Image
        }
        "csv" | "epub" | "html" | "md" | "pdf" | "rtf" | "txt" => DerivedContentType::Document,
        "7z" | "bz2" | "gz" | "rar" | "tar" | "xz" | "zip" => DerivedContentType::Archive,
        "apk" | "appimage" | "deb" | "dmg" | "exe" | "msi" | "pkg" | "rpm" => {
            DerivedContentType::Software
        }
        _ => DerivedContentType::Other,
    }
}

struct CanonicalWriter(Vec<u8>);

impl CanonicalWriter {
    fn new(prefix: &[u8]) -> Self {
        Self(prefix.to_vec())
    }

    fn byte(&mut self, value: u8) {
        self.0.push(value);
    }

    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) {
        self.u64(value.len() as u64);
        self.0.extend_from_slice(value.as_bytes());
    }

    fn publisher(&mut self, value: &PublisherId) {
        self.0.extend_from_slice(&value.to_bytes());
    }

    fn subject(&mut self, value: &SubjectRef) {
        match value {
            SubjectRef::Torrent(reference) => {
                self.byte(0);
                self.string(&reference.to_string());
            }
            SubjectRef::Uri(uri) => {
                self.byte(1);
                self.string(uri.as_str());
            }
        }
    }

    fn subjects(&mut self, values: &[SubjectRef]) {
        self.u64(values.len() as u64);
        for value in values {
            self.subject(value);
        }
    }

    fn source(&mut self, value: &SourceAttribution) {
        match value {
            SourceAttribution::Direct => self.byte(0),
            SourceAttribution::Uri { uri } => {
                self.byte(1);
                self.string(uri.as_str());
            }
            SourceAttribution::Import {
                provenance,
                locator,
            } => {
                self.byte(2);
                match provenance {
                    ImportProvenance::InfoAuthenticated => self.byte(0),
                    ImportProvenance::TopLevelHint => self.byte(1),
                    ImportProvenance::MagnetHint => self.byte(2),
                    ImportProvenance::IndexerClaim { issuer } => {
                        self.byte(3);
                        self.publisher(issuer);
                    }
                    ImportProvenance::LocalClient => self.byte(4),
                }
                match locator {
                    Some(uri) => {
                        self.byte(1);
                        self.string(uri.as_str());
                    }
                    None => self.byte(0),
                }
            }
        }
    }

    fn finish(self) -> Vec<u8> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publisher(seed: u8) -> PublisherId {
        PublisherId::new(pubky::Keypair::from_secret(&[seed; 32]).public_key())
    }

    fn v1(byte: u8) -> InfoHashV1 {
        InfoHashV1::from_bytes([byte; 20])
    }

    fn torrent(byte: u8) -> TorrentRef {
        TorrentRef::verified_hybrid(v1(byte), Btmh::from_sha256([byte; 32]))
    }

    fn subjects() -> Vec<SubjectRef> {
        vec![
            SubjectRef::Torrent(torrent(2)),
            SubjectRef::Torrent(torrent(1)),
            SubjectRef::Uri(CanonicalUri::new("https://example.com/catalog/item").unwrap()),
        ]
    }

    #[test]
    fn torrent_refs_round_trip_and_normalize() {
        let refs = [
            TorrentRef::btih(v1(1)),
            TorrentRef::btmh(Btmh::from_sha256([2; 32])),
            torrent(3),
        ];
        for reference in refs {
            let text = reference.to_string();
            assert_eq!(text.parse::<TorrentRef>().unwrap(), reference);
            let json = serde_json::to_string(&reference).unwrap();
            assert_eq!(
                serde_json::from_str::<TorrentRef>(&json).unwrap(),
                reference
            );
        }
        assert!("btmh:1220AA".parse::<TorrentRef>().is_err());
        assert!(
            format!("btih:{}", "AB".repeat(20))
                .parse::<TorrentRef>()
                .is_err()
        );
        assert!(
            format!("btmh:{}", "00".repeat(34))
                .parse::<TorrentRef>()
                .is_err()
        );
    }

    #[test]
    fn canonical_uri_rejects_credentials_fragments_and_normalization_changes() {
        assert!(CanonicalUri::new("https://example.com/a").is_ok());
        assert!(CanonicalUri::new("https://user:pass@example.com/a").is_err());
        assert!(CanonicalUri::new("https://example.com/a#fragment").is_err());
        assert!(CanonicalUri::new("HTTPS://EXAMPLE.COM/a").is_err());
        assert!(CanonicalUri::new("/relative").is_err());
    }

    #[test]
    fn import_mapping_keeps_boundaries_and_rejects_private_publication() {
        let reference = torrent(1);
        assert!(matches!(
            map_info_name(reference, 1, "Authenticated")
                .unwrap()
                .provenance,
            ImportProvenance::InfoAuthenticated
        ));
        assert!(matches!(
            map_magnet_dn(reference, 1, "Hint").unwrap().provenance,
            ImportProvenance::MagnetHint
        ));
        assert!(matches!(
            map_top_level_comment(reference, 1, "untrusted")
                .unwrap()
                .provenance,
            ImportProvenance::TopLevelHint
        ));
        assert!(ensure_public_catalog_eligible(false).is_ok());
        assert!(ensure_public_catalog_eligible(true).is_err());

        let public = PublicCatalogImport::new(
            reference,
            vec![
                map_private(reference, 1, false).unwrap(),
                map_info_name(reference, 1, "Authenticated").unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(public.subject(), reference);
        assert_eq!(public.observations().len(), 2);
        assert!(
            PublicCatalogImport::new(reference, vec![map_private(reference, 1, true).unwrap()])
                .is_err()
        );
        assert!(
            PublicCatalogImport::new(
                reference,
                vec![map_magnet_dn(reference, 1, "No authenticated privacy").unwrap()]
            )
            .is_err()
        );
    }

    #[test]
    fn file_mapping_derives_extension_and_type() {
        let values = derive_file_observations(torrent(1), 1, "media/MOVIE.MP4").unwrap();
        assert_eq!(values.len(), 2);
        assert!(matches!(
            &values[0].value,
            ObservationValue::FileExtension { value } if value == "mp4"
        ));
        assert!(matches!(
            values[1].value,
            ObservationValue::FileType {
                value: DerivedContentType::Video
            }
        ));
        assert!(
            derive_file_observations(torrent(1), 1, "README")
                .unwrap()
                .is_empty()
        );
        assert!(derive_file_observations(torrent(1), 1, "../escape.mp4").is_err());
    }

    #[test]
    fn tracker_mapping_rejects_credentials_and_mismatched_issuer() {
        assert!(
            map_tracker(
                torrent(1),
                1,
                "https://user:secret@tracker.example/announce",
                TrackerSource::Announce,
                None,
            )
            .is_err()
        );
        assert!(
            map_tracker(
                torrent(1),
                1,
                "https://tracker.example/announce?passkey=secret",
                TrackerSource::Announce,
                None,
            )
            .is_err()
        );
        assert!(
            map_tracker(
                torrent(1),
                1,
                "https://tracker.example/announce",
                TrackerSource::Indexer,
                None,
            )
            .is_err()
        );
        assert!(
            map_tracker(
                torrent(1),
                1,
                "udp://tracker.example:80/announce",
                TrackerSource::Indexer,
                Some(publisher(1)),
            )
            .is_ok()
        );
    }

    #[test]
    fn suggestions_are_issuer_attributed_claims() {
        let issuer = publisher(7);
        let observation = map_issuer_suggestion(
            torrent(1),
            1,
            issuer.clone(),
            SuggestionNamespace::Torznab,
            "category",
            "2000",
        )
        .unwrap();
        assert!(matches!(
            observation.provenance,
            ImportProvenance::IndexerClaim { issuer: value } if value == issuer
        ));
        assert!(matches!(
            observation.value,
            ObservationValue::Suggestion { issuer: value, .. } if value == issuer
        ));

        let local = map_issuer_suggestion(
            torrent(1),
            1,
            issuer.clone(),
            SuggestionNamespace::QBittorrent,
            "category",
            "linux",
        )
        .unwrap();
        assert!(matches!(local.provenance(), ImportProvenance::LocalClient));
        assert!(matches!(
            local.value(),
            ObservationValue::Suggestion { issuer: value, .. } if value == &issuer
        ));
    }

    #[test]
    fn observation_wire_revalidates_provenance_and_bounds() {
        let observation = map_info_name(torrent(1), 1, "Authenticated").unwrap();
        let mut value = serde_json::to_value(observation).unwrap();
        value["value"] = serde_json::json!({"kind": "comment", "value": "not authenticated"});
        assert!(serde_json::from_value::<ImportObservation>(value).is_err());

        let observation = map_top_level_creation_date(torrent(1), 1, 2).unwrap();
        let mut value = serde_json::to_value(observation).unwrap();
        value["observed_at"] = serde_json::Value::from(0);
        assert!(serde_json::from_value::<ImportObservation>(value).is_err());

        assert!(map_top_level_creator(torrent(1), 1, "creator").is_ok());
        assert!(map_bep38_relation(torrent(1), 1, RelationKind::Similar, torrent(1)).is_err());
        assert!(map_bep38_relation(torrent(1), 1, RelationKind::Similar, torrent(2)).is_ok());
    }

    #[test]
    fn tag_claim_round_trip_tamper_and_bounds() {
        let claim = TagClaimV1::new(
            publisher(1),
            SubjectRef::Torrent(torrent(1)),
            "open-media".to_owned(),
            TagOperation::Add,
            10,
            1,
            SourceAttribution::Direct,
        )
        .unwrap();
        let json = serde_json::to_string(&claim).unwrap();
        assert_eq!(serde_json::from_str::<TagClaimV1>(&json).unwrap(), claim);
        let mut value = serde_json::to_value(&claim).unwrap();
        value["tag"] = serde_json::Value::String("changed".to_owned());
        assert!(serde_json::from_value::<TagClaimV1>(value).is_err());
        assert!(
            TagClaimV1::new(
                publisher(1),
                SubjectRef::Torrent(torrent(1)),
                "UPPER".to_owned(),
                TagOperation::Add,
                10,
                1,
                SourceAttribution::Direct,
            )
            .is_err()
        );
        assert!(
            TagClaimV1::new(
                publisher(1),
                SubjectRef::Torrent(torrent(1)),
                "a".repeat(32),
                TagOperation::Add,
                10,
                1,
                SourceAttribution::Direct,
            )
            .is_ok()
        );
        assert!(
            TagClaimV1::new(
                publisher(1),
                SubjectRef::Torrent(torrent(1)),
                "a".repeat(33),
                TagOperation::Add,
                10,
                1,
                SourceAttribution::Direct,
            )
            .is_err()
        );
        assert!(
            TagClaimV1::new(
                publisher(1),
                SubjectRef::Torrent(torrent(1)),
                "tag".to_owned(),
                TagOperation::Add,
                10,
                0,
                SourceAttribution::Direct,
            )
            .is_err()
        );
        assert!(
            TagClaimV1::new(
                publisher(1),
                SubjectRef::Torrent(torrent(1)),
                "tag".to_owned(),
                TagOperation::Add,
                10,
                1,
                SourceAttribution::Import {
                    provenance: ImportProvenance::IndexerClaim {
                        issuer: publisher(2),
                    },
                    locator: None,
                },
            )
            .is_err()
        );
    }

    #[test]
    fn collections_and_blocklists_are_order_independent() {
        let original = subjects();
        let collection = CollectionV1::new(
            publisher(1),
            "Favorites".to_owned(),
            String::new(),
            original.clone(),
            10,
            1,
            SourceAttribution::Direct,
        )
        .unwrap();
        let mut reversed = original;
        reversed.reverse();
        let rebuilt = CollectionV1::new(
            publisher(1),
            "Favorites".to_owned(),
            String::new(),
            reversed,
            10,
            1,
            SourceAttribution::Direct,
        )
        .unwrap();
        assert_eq!(collection.id(), rebuilt.id());
        assert_eq!(
            collection.to_canonical_bytes(),
            rebuilt.to_canonical_bytes()
        );
        assert_eq!(
            serde_json::from_str::<CollectionV1>(&serde_json::to_string(&collection).unwrap())
                .unwrap(),
            collection
        );

        let blocklist = BlocklistV1::new(
            publisher(2),
            "Blocked".to_owned(),
            subjects(),
            11,
            2,
            SourceAttribution::Direct,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<BlocklistV1>(&serde_json::to_string(&blocklist).unwrap())
                .unwrap(),
            blocklist
        );
    }

    #[test]
    fn duplicate_subjects_and_invalid_revisions_are_rejected() {
        let duplicate = vec![
            SubjectRef::Torrent(torrent(1)),
            SubjectRef::Torrent(torrent(1)),
        ];
        assert!(
            CollectionV1::new(
                publisher(1),
                "Duplicate".to_owned(),
                String::new(),
                duplicate,
                1,
                1,
                SourceAttribution::Direct,
            )
            .is_err()
        );
        assert!(
            BlocklistV1::new(
                publisher(1),
                "Empty".to_owned(),
                Vec::new(),
                1,
                1,
                SourceAttribution::Direct,
            )
            .is_err()
        );
    }

    #[test]
    fn collection_subject_count_boundaries_are_enforced() {
        let subjects = (0..MAX_ARTIFACT_SUBJECTS)
            .map(|index| {
                SubjectRef::Uri(
                    CanonicalUri::new(format!("https://example.com/items/{index:05}")).unwrap(),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            CollectionV1::new(
                publisher(1),
                "Maximum".to_owned(),
                String::new(),
                subjects.clone(),
                1,
                1,
                SourceAttribution::Direct,
            )
            .is_ok()
        );
        let mut over_limit = subjects;
        over_limit.push(SubjectRef::Uri(
            CanonicalUri::new("https://example.com/items/extra").unwrap(),
        ));
        assert!(
            CollectionV1::new(
                publisher(1),
                "Over".to_owned(),
                String::new(),
                over_limit,
                1,
                1,
                SourceAttribution::Direct,
            )
            .is_err()
        );
    }

    #[test]
    fn tombstones_and_decisions_round_trip() {
        let subject = SubjectRef::Torrent(torrent(9));
        let tombstone = TombstoneV1::new(
            publisher(1),
            subject.clone(),
            "withdrawn".to_owned(),
            5,
            3,
            SourceAttribution::Direct,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<TombstoneV1>(&serde_json::to_string(&tombstone).unwrap())
                .unwrap(),
            tombstone
        );

        let decision = ModerationDecisionV1::new(
            publisher(2),
            subject,
            ModerationAction::Block,
            "policy".to_owned(),
            6,
            4,
            SourceAttribution::Direct,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<ModerationDecisionV1>(
                &serde_json::to_string(&decision).unwrap()
            )
            .unwrap(),
            decision
        );
    }
}
