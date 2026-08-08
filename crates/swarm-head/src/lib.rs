//! BEP 46 mutable torrent heads owned by Pubky identities.
//!
//! Pubky and BEP 46 both use Ed25519 identities. This crate uses the same
//! 32-byte identity key in a salted BEP 44 mutable slot, keeping torrent state
//! separate from the Pubky `_pubky` DNS packet.

#![forbid(unsafe_code)]

use mainline::{MutableItem, SigningKey, async_dht::AsyncDht};

pub use swarm_protocol::InfoHashV1;

/// Domain-separating salt for the current Pubky Swarm dataset torrent.
pub const DATASET_HEAD_SALT: &[u8] = b"pubky.swarm/v1/dataset";

const BEP46_VALUE_PREFIX: &[u8] = b"d2:ih20:";
const BEP46_VALUE_LEN: usize = BEP46_VALUE_PREFIX.len() + 20 + 1;

/// Errors returned by BEP 46 head operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The BEP 46 value was not the canonical `d2:ih20:<20 bytes>e` dictionary.
    #[error("invalid BEP 46 value")]
    InvalidBep46Value,
    /// The mutable item used a different domain-separation salt.
    #[error("unexpected BEP 46 salt")]
    UnexpectedSalt,
    /// BEP 44 sequence numbers must be positive.
    #[error("invalid BEP 44 sequence {0}: expected a positive integer")]
    InvalidSequence(i64),
    /// Incrementing the current sequence would overflow.
    #[error("BEP 44 sequence exhausted")]
    SequenceExhausted,
    /// The caller's expected previous sequence did not match the DHT.
    #[error("concurrent head update: expected {expected:?}, found {actual:?}")]
    ConcurrentUpdate {
        /// Sequence the caller expected, or `None` for first publication.
        expected: Option<i64>,
        /// Most recent sequence found in the DHT.
        actual: Option<i64>,
    },
    /// A resolved head is older than state this client has already accepted.
    #[error("rollback detected: highest accepted sequence {highest_seen}, resolved {resolved}")]
    Rollback {
        /// Highest sequence persisted by the caller.
        highest_seen: i64,
        /// Sequence returned by the DHT.
        resolved: i64,
    },
    /// Mainline DHT publication failed.
    #[error("Mainline DHT operation failed: {0}")]
    Dht(String),
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Encode the exact BEP 46 value dictionary.
#[must_use]
pub fn encode_bep46_value(info_hash: InfoHashV1) -> [u8; BEP46_VALUE_LEN] {
    let mut value = [0_u8; BEP46_VALUE_LEN];
    value[..BEP46_VALUE_PREFIX.len()].copy_from_slice(BEP46_VALUE_PREFIX);
    value[BEP46_VALUE_PREFIX.len()..BEP46_VALUE_PREFIX.len() + 20]
        .copy_from_slice(info_hash.as_bytes());
    value[BEP46_VALUE_LEN - 1] = b'e';
    value
}

/// Decode a canonical BEP 46 value dictionary.
///
/// # Errors
///
/// Returns [`Error::InvalidBep46Value`] for non-canonical or malformed input.
pub fn decode_bep46_value(value: &[u8]) -> Result<InfoHashV1> {
    if value.len() != BEP46_VALUE_LEN
        || !value.starts_with(BEP46_VALUE_PREFIX)
        || value[BEP46_VALUE_LEN - 1] != b'e'
    {
        return Err(Error::InvalidBep46Value);
    }
    let hash: [u8; 20] = value[BEP46_VALUE_PREFIX.len()..BEP46_VALUE_LEN - 1]
        .try_into()
        .map_err(|_| Error::InvalidBep46Value)?;
    Ok(InfoHashV1::from_bytes(hash))
}

/// A verified, signed BEP 46 dataset head.
#[derive(Debug, Clone)]
pub struct SignedHead {
    item: MutableItem,
    info_hash: InfoHashV1,
}

impl SignedHead {
    /// Validate and wrap a mutable item returned by Mainline.
    ///
    /// Mainline has already verified the Ed25519 signature and DHT target.
    ///
    /// # Errors
    ///
    /// Rejects a wrong salt, non-positive sequence, or malformed BEP 46 value.
    pub fn from_mutable_item(item: MutableItem) -> Result<Self> {
        if item.salt() != Some(DATASET_HEAD_SALT) {
            return Err(Error::UnexpectedSalt);
        }
        if item.seq() <= 0 {
            return Err(Error::InvalidSequence(item.seq()));
        }
        let info_hash = decode_bep46_value(item.value())?;
        Ok(Self { item, info_hash })
    }

    /// Publisher's Pubky/Ed25519 public key.
    #[must_use]
    pub fn publisher(&self) -> &[u8; 32] {
        self.item.key()
    }

    /// Current v1 dataset torrent info hash.
    #[must_use]
    pub const fn info_hash(&self) -> InfoHashV1 {
        self.info_hash
    }

    /// Monotonic BEP 44 sequence.
    #[must_use]
    pub fn sequence(&self) -> i64 {
        self.item.seq()
    }

    /// Mainline target derived from publisher key and salt.
    #[must_use]
    pub fn target(&self) -> mainline::Id {
        *self.item.target()
    }

    fn mutable_item(&self) -> MutableItem {
        self.item.clone()
    }
}

/// Root identity signer used only in trusted signing processes.
///
/// Application sessions cannot construct this type without the 32-byte root
/// seed. Production desktop clients should obtain signatures from a trusted
/// signer rather than importing a user's primary root key.
#[derive(Debug, Clone)]
pub struct HeadSigner(SigningKey);

impl HeadSigner {
    /// Construct from a Pubky Ed25519 root seed.
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self(SigningKey::from_bytes(&seed))
    }

    /// The Pubky-compatible Ed25519 public key.
    #[must_use]
    pub fn public_key(&self) -> [u8; 32] {
        self.0.verifying_key().to_bytes()
    }

    /// Sign a specific sequence and info hash.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidSequence`] unless `sequence` is positive.
    pub fn sign(&self, info_hash: InfoHashV1, sequence: i64) -> Result<SignedHead> {
        if sequence <= 0 {
            return Err(Error::InvalidSequence(sequence));
        }
        let value = encode_bep46_value(info_hash);
        SignedHead::from_mutable_item(MutableItem::new(
            self.0.clone(),
            &value,
            sequence,
            Some(DATASET_HEAD_SALT),
        ))
    }
}

/// Mainline client for resolving, publishing, and reannouncing dataset heads.
#[derive(Debug, Clone)]
pub struct HeadClient {
    dht: AsyncDht,
}

impl HeadClient {
    /// Wrap an isolated or public Mainline DHT client.
    #[must_use]
    pub const fn new(dht: AsyncDht) -> Self {
        Self { dht }
    }

    /// Resolve the most recent signed head.
    ///
    /// `highest_seen` must come from durable local state. A lower result is
    /// rejected as a rollback. This cannot prevent first-contact rollback.
    ///
    /// # Errors
    ///
    /// Returns validation errors for malformed heads and [`Error::Rollback`]
    /// when the network result is older than `highest_seen`.
    pub async fn resolve(
        &self,
        publisher: &[u8; 32],
        highest_seen: Option<i64>,
    ) -> Result<Option<SignedHead>> {
        let Some(item) = self
            .dht
            .get_mutable_most_recent(publisher, Some(DATASET_HEAD_SALT))
            .await
        else {
            return Ok(None);
        };
        let head = SignedHead::from_mutable_item(item)?;
        if let Some(highest_seen) = highest_seen
            && head.sequence() < highest_seen
        {
            return Err(Error::Rollback {
                highest_seen,
                resolved: head.sequence(),
            });
        }
        Ok(Some(head))
    }

    /// Publish the next head after checking the expected current sequence.
    ///
    /// Pass `None` only for first publication. The method resolves current
    /// state, increments its sequence, and supplies BEP 44 CAS to Mainline.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ConcurrentUpdate`] when `expected_previous` differs
    /// from the DHT and [`Error::Dht`] when publication fails.
    pub async fn publish_next(
        &self,
        signer: &HeadSigner,
        info_hash: InfoHashV1,
        expected_previous: Option<i64>,
    ) -> Result<SignedHead> {
        let current = self.resolve(&signer.public_key(), None).await?;
        let actual = current.as_ref().map(SignedHead::sequence);
        if actual != expected_previous {
            return Err(Error::ConcurrentUpdate {
                expected: expected_previous,
                actual,
            });
        }
        let next = actual
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::SequenceExhausted)?;
        let head = signer.sign(info_hash, next)?;
        self.dht
            .put_mutable(head.mutable_item(), actual)
            .await
            .map_err(|error| Error::Dht(error.to_string()))?;
        Ok(head)
    }

    /// Reannounce an already signed head without possessing its private key.
    ///
    /// This refreshes DHT retention and cannot change the signed value.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Dht`] if Mainline cannot store the item.
    pub async fn reannounce(&self, head: &SignedHead) -> Result<()> {
        self.dht
            .put_mutable(head.mutable_item(), None)
            .await
            .map_err(|error| Error::Dht(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::time::Duration;

    use mainline::{Dht, MutableItem, Testnet};

    use super::*;

    fn hash(byte: u8) -> InfoHashV1 {
        InfoHashV1::from_bytes([byte; 20])
    }

    fn local_client(bootstrap: &[String]) -> HeadClient {
        let dht = Dht::builder()
            .bootstrap(bootstrap)
            .bind_address(Ipv4Addr::LOCALHOST)
            .build()
            .expect("local DHT client");
        HeadClient::new(dht.as_async())
    }

    #[test]
    fn canonical_bep46_value_round_trip() {
        let info_hash = hash(0x42);
        let encoded = encode_bep46_value(info_hash);
        assert_eq!(encoded.len(), 29);
        assert_eq!(&encoded[..8], b"d2:ih20:");
        assert_eq!(encoded[28], b'e');
        assert_eq!(decode_bep46_value(&encoded).unwrap(), info_hash);

        for malformed in [
            &encoded[..28],
            b"d2:ih19:1234567890123456789e".as_slice(),
            b"d2:zz20:12345678901234567890e".as_slice(),
        ] {
            assert!(matches!(
                decode_bep46_value(malformed),
                Err(Error::InvalidBep46Value)
            ));
        }
    }

    #[test]
    fn official_bep46_salted_target_vector() {
        let public_key: [u8; 32] =
            hex::decode("8543d3e6115f0f98c944077a4493dcd543e49c739fd998550a1f614ab36ed63e")
                .unwrap()
                .try_into()
                .unwrap();
        let target = MutableItem::target_from_key(&public_key, Some(&[0x6e]));
        assert_eq!(
            target.to_string(),
            "59ee7c2cb9b4f7eb1986ee2d18fd2fdb8a56554f"
        );
    }

    #[test]
    fn pubky_and_mainline_derive_the_same_identity() {
        let seed = [0x5a; 32];
        let signer = HeadSigner::from_seed(seed);
        let pubky_keypair = pubky::Keypair::from_secret(&seed);
        assert_eq!(signer.public_key(), pubky_keypair.public_key().to_bytes());
    }

    #[test]
    fn rejects_wrong_salt_sequence_and_value() {
        let key = SigningKey::from_bytes(&[7; 32]);
        let value = encode_bep46_value(hash(1));
        let wrong_salt = MutableItem::new(key.clone(), &value, 1, Some(b"wrong"));
        assert!(matches!(
            SignedHead::from_mutable_item(wrong_salt),
            Err(Error::UnexpectedSalt)
        ));

        let zero = MutableItem::new(key.clone(), &value, 0, Some(DATASET_HEAD_SALT));
        assert!(matches!(
            SignedHead::from_mutable_item(zero),
            Err(Error::InvalidSequence(0))
        ));

        let malformed = MutableItem::new(key, b"not-bep46", 1, Some(DATASET_HEAD_SALT));
        assert!(matches!(
            SignedHead::from_mutable_item(malformed),
            Err(Error::InvalidBep46Value)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn publish_resolve_cas_rollback_and_keyless_reannounce() {
        let testnet = Testnet::builder(5).build().expect("local testnet");
        let publisher = local_client(&testnet.bootstrap);
        let reader = local_client(&testnet.bootstrap);
        let republisher = local_client(&testnet.bootstrap);
        let signer = HeadSigner::from_seed([0x33; 32]);

        let first = tokio::time::timeout(
            Duration::from_secs(10),
            publisher.publish_next(&signer, hash(1), None),
        )
        .await
        .expect("first publish timed out")
        .expect("first publish");
        assert_eq!(first.sequence(), 1);

        let resolved = tokio::time::timeout(
            Duration::from_secs(10),
            reader.resolve(&signer.public_key(), None),
        )
        .await
        .expect("resolve timed out")
        .expect("resolve")
        .expect("head");
        assert_eq!(resolved.info_hash(), hash(1));

        let second = publisher
            .publish_next(&signer, hash(2), Some(1))
            .await
            .expect("second publish");
        assert_eq!(second.sequence(), 2);

        assert!(matches!(
            publisher.publish_next(&signer, hash(3), Some(1)).await,
            Err(Error::ConcurrentUpdate {
                expected: Some(1),
                actual: Some(2)
            })
        ));
        assert!(matches!(
            reader.resolve(&signer.public_key(), Some(3)).await,
            Err(Error::Rollback {
                highest_seen: 3,
                resolved: 2
            })
        ));

        drop(signer);
        republisher
            .reannounce(&second)
            .await
            .expect("keyless reannounce");
        let after_reannounce = reader
            .resolve(second.publisher(), Some(2))
            .await
            .expect("resolve after reannounce")
            .expect("head after reannounce");
        assert_eq!(after_reannounce.sequence(), 2);
        assert_eq!(after_reannounce.info_hash(), hash(2));
    }
}
