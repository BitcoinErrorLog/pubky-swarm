//! `SQLite` persistence for portable catalog and moderation artifacts.

use rusqlite::{OptionalExtension, params};
use swarm_protocol::{
    ArtifactId, BlocklistV1, CanonicalUri, CollectionV1, ModerationDecisionV1, PublisherId,
    SubjectRef, TagClaimV1, TombstoneV1,
};

use crate::{Error, Result, Store, unix_millis};

/// One locally configured blocklist subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlocklistSubscription {
    /// Blocklist authority.
    pub issuer: PublisherId,
    /// Canonical location used to retrieve the blocklist.
    pub source_uri: CanonicalUri,
    /// Local subscription time in Unix milliseconds.
    pub added_at: u64,
}

impl Store {
    /// Cache a validated issuer-attributed tag claim.
    ///
    /// Repeated insertion of the same deterministic claim is idempotent.
    ///
    /// # Errors
    ///
    /// Returns serialization, lock, or `SQLite` errors.
    pub fn cache_tag_claim(&self, claim: &TagClaimV1) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO tag_claims
                 (claim_id, issuer, subject, created_at, revision, claim_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(claim_id) DO NOTHING",
            params![
                claim.id().to_string(),
                claim.issuer().to_string(),
                claim.subject().to_string(),
                claim.created_at(),
                claim.revision(),
                serde_json::to_string(claim)?,
            ],
        )?;
        Ok(())
    }

    /// Load tag claims for a subject, newest first.
    ///
    /// These remain separate claims and are never merged into `ReleaseV1`
    /// tags by the store.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn tag_claims_for(&self, subject: &SubjectRef) -> Result<Vec<TagClaimV1>> {
        self.load_artifacts(
            "SELECT claim_json FROM tag_claims
             WHERE subject = ?1
             ORDER BY created_at DESC, issuer ASC, claim_id ASC",
            subject.to_string(),
        )
    }

    /// Return the highest cached tag-claim revision for an issuer.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn highest_tag_claim_revision(&self, issuer: &PublisherId) -> Result<Option<u64>> {
        self.connection()?
            .query_row(
                "SELECT MAX(revision) FROM tag_claims WHERE issuer = ?1",
                [issuer.to_string()],
                |row| row.get(0),
            )
            .map_err(Error::from)
    }

    /// Cache a validated collection snapshot.
    ///
    /// # Errors
    ///
    /// Returns serialization, lock, or `SQLite` errors.
    pub fn cache_collection(&self, collection: &CollectionV1) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO collections
                 (collection_id, issuer, created_at, revision, collection_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(collection_id) DO NOTHING",
            params![
                collection.id().to_string(),
                collection.issuer().to_string(),
                collection.created_at(),
                collection.revision(),
                serde_json::to_string(collection)?,
            ],
        )?;
        Ok(())
    }

    /// Load one collection by deterministic artifact identifier.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn collection(&self, id: ArtifactId) -> Result<Option<CollectionV1>> {
        self.load_artifact(
            "SELECT collection_json FROM collections WHERE collection_id = ?1",
            id.to_string(),
        )
    }

    /// Load an issuer's collection snapshots by descending revision.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn collections_for(&self, issuer: &PublisherId) -> Result<Vec<CollectionV1>> {
        self.load_artifacts(
            "SELECT collection_json FROM collections
             WHERE issuer = ?1
             ORDER BY revision DESC, created_at DESC, collection_id ASC",
            issuer.to_string(),
        )
    }

    /// Cache a validated tombstone.
    ///
    /// # Errors
    ///
    /// Returns serialization, lock, or `SQLite` errors.
    pub fn cache_tombstone(&self, tombstone: &TombstoneV1) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO tombstones
                 (tombstone_id, issuer, subject, created_at, revision, tombstone_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(tombstone_id) DO NOTHING",
            params![
                tombstone.id().to_string(),
                tombstone.issuer().to_string(),
                tombstone.subject().to_string(),
                tombstone.created_at(),
                tombstone.revision(),
                serde_json::to_string(tombstone)?,
            ],
        )?;
        Ok(())
    }

    /// Load tombstones for a subject, newest first.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn tombstones_for(&self, subject: &SubjectRef) -> Result<Vec<TombstoneV1>> {
        self.load_artifacts(
            "SELECT tombstone_json FROM tombstones
             WHERE subject = ?1
             ORDER BY created_at DESC, issuer ASC, tombstone_id ASC",
            subject.to_string(),
        )
    }

    /// Cache a validated moderation decision.
    ///
    /// # Errors
    ///
    /// Returns serialization, lock, or `SQLite` errors.
    pub fn cache_moderation_decision(&self, decision: &ModerationDecisionV1) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO moderation_decisions
                 (decision_id, issuer, subject, created_at, revision, decision_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(decision_id) DO NOTHING",
            params![
                decision.id().to_string(),
                decision.issuer().to_string(),
                decision.subject().to_string(),
                decision.created_at(),
                decision.revision(),
                serde_json::to_string(decision)?,
            ],
        )?;
        Ok(())
    }

    /// Load moderation decisions for a subject, newest first.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn moderation_decisions_for(
        &self,
        subject: &SubjectRef,
    ) -> Result<Vec<ModerationDecisionV1>> {
        self.load_artifacts(
            "SELECT decision_json FROM moderation_decisions
             WHERE subject = ?1
             ORDER BY created_at DESC, issuer ASC, decision_id ASC",
            subject.to_string(),
        )
    }

    /// Cache a validated blocklist snapshot.
    ///
    /// # Errors
    ///
    /// Returns serialization, lock, or `SQLite` errors.
    pub fn cache_blocklist(&self, blocklist: &BlocklistV1) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO blocklists
                 (blocklist_id, issuer, created_at, revision, blocklist_json)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(blocklist_id) DO NOTHING",
            params![
                blocklist.id().to_string(),
                blocklist.issuer().to_string(),
                blocklist.created_at(),
                blocklist.revision(),
                serde_json::to_string(blocklist)?,
            ],
        )?;
        Ok(())
    }

    /// Load one blocklist by deterministic artifact identifier.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn blocklist(&self, id: ArtifactId) -> Result<Option<BlocklistV1>> {
        self.load_artifact(
            "SELECT blocklist_json FROM blocklists WHERE blocklist_id = ?1",
            id.to_string(),
        )
    }

    /// Load an issuer's blocklist snapshots by descending revision.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or artifact validation errors.
    pub fn blocklists_for(&self, issuer: &PublisherId) -> Result<Vec<BlocklistV1>> {
        self.load_artifacts(
            "SELECT blocklist_json FROM blocklists
             WHERE issuer = ?1
             ORDER BY revision DESC, created_at DESC, blocklist_id ASC",
            issuer.to_string(),
        )
    }

    /// Record an authority sequence only if it advances the stored high-water
    /// mark.
    ///
    /// Returns `true` when the sequence was inserted or advanced and `false`
    /// for a replay or older sequence.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn record_authority_sequence(
        &self,
        authority: &PublisherId,
        sequence: u64,
    ) -> Result<bool> {
        if sequence == 0 {
            return Err(Error::InvalidAuthoritySequence);
        }
        let changed = self.connection()?.execute(
            "INSERT INTO authority_sequences (authority, highest_sequence)
             VALUES (?1, ?2)
             ON CONFLICT(authority) DO UPDATE SET
                 highest_sequence = excluded.highest_sequence
             WHERE excluded.highest_sequence > authority_sequences.highest_sequence",
            params![authority.to_string(), sequence],
        )?;
        Ok(changed == 1)
    }

    /// Return the highest accepted sequence for an authority.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn highest_authority_sequence(&self, authority: &PublisherId) -> Result<Option<u64>> {
        self.connection()?
            .query_row(
                "SELECT highest_sequence FROM authority_sequences WHERE authority = ?1",
                [authority.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Subscribe to a publisher's blocklist at a canonical source URI.
    ///
    /// Repeated calls are idempotent and retain the initial subscription time.
    ///
    /// # Errors
    ///
    /// Returns clock, lock, or `SQLite` errors.
    pub fn subscribe_blocklist(
        &self,
        issuer: &PublisherId,
        source_uri: &CanonicalUri,
    ) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO blocklist_subscriptions (issuer, source_uri, added_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(issuer, source_uri) DO NOTHING",
            params![issuer.to_string(), source_uri.as_str(), unix_millis()?],
        )?;
        Ok(())
    }

    /// Remove a blocklist subscription.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn unsubscribe_blocklist(
        &self,
        issuer: &PublisherId,
        source_uri: &CanonicalUri,
    ) -> Result<()> {
        self.connection()?.execute(
            "DELETE FROM blocklist_subscriptions
             WHERE issuer = ?1 AND source_uri = ?2",
            params![issuer.to_string(), source_uri.as_str()],
        )?;
        Ok(())
    }

    /// List blocklist subscriptions in insertion order.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, identity, or URI validation errors.
    pub fn blocklist_subscriptions(&self) -> Result<Vec<BlocklistSubscription>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT issuer, source_uri, added_at FROM blocklist_subscriptions
             ORDER BY subscription_order ASC",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(issuer, source_uri, added_at)| {
                Ok(BlocklistSubscription {
                    issuer: issuer.parse().map_err(|error: swarm_protocol::Error| {
                        Error::InvalidPublisher(error.to_string())
                    })?,
                    source_uri: source_uri.parse()?,
                    added_at,
                })
            })
            .collect()
    }

    fn load_artifact<T>(&self, sql: &str, parameter: String) -> Result<Option<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let json = self
            .connection()?
            .query_row(sql, [parameter], |row| row.get::<_, String>(0))
            .optional()?;
        json.map(|value| serde_json::from_str(&value).map_err(Error::from))
            .transpose()
    }

    fn load_artifacts<T>(&self, sql: &str, parameter: String) -> Result<Vec<T>>
    where
        T: serde::de::DeserializeOwned,
    {
        let connection = self.connection()?;
        let mut statement = connection.prepare(sql)?;
        let json = statement
            .query_map([parameter], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        json.into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use pubky::Keypair;
    use swarm_protocol::{
        BlocklistV1, Btmh, CollectionV1, InfoHashV1, ModerationAction, ModerationDecisionV1,
        SourceAttribution, SubjectRef, TagOperation, TombstoneV1, TorrentRef,
    };

    use super::*;

    fn publisher(seed: u8) -> PublisherId {
        PublisherId::new(Keypair::from_secret(&[seed; 32]).public_key())
    }

    fn subject(seed: u8) -> SubjectRef {
        SubjectRef::Torrent(TorrentRef::verified_hybrid(
            InfoHashV1::from_bytes([seed; 20]),
            Btmh::from_sha256([seed; 32]),
        ))
    }

    #[test]
    fn artifacts_round_trip_without_merging_claims_into_releases() {
        let store = Store::in_memory().unwrap();
        let alice = publisher(1);
        let target = subject(3);
        let claim = TagClaimV1::new(
            alice.clone(),
            target.clone(),
            "community-tag".to_owned(),
            TagOperation::Add,
            10,
            1,
            SourceAttribution::Direct,
        )
        .unwrap();
        let collection = CollectionV1::new(
            alice.clone(),
            "Collection".to_owned(),
            "Description".to_owned(),
            vec![target.clone()],
            11,
            2,
            SourceAttribution::Direct,
        )
        .unwrap();
        let tombstone = TombstoneV1::new(
            alice.clone(),
            target.clone(),
            "withdrawn".to_owned(),
            12,
            3,
            SourceAttribution::Direct,
        )
        .unwrap();
        let decision = ModerationDecisionV1::new(
            alice.clone(),
            target.clone(),
            ModerationAction::Review,
            "review required".to_owned(),
            13,
            4,
            SourceAttribution::Direct,
        )
        .unwrap();
        let blocklist = BlocklistV1::new(
            alice.clone(),
            "Blocked".to_owned(),
            vec![target.clone()],
            14,
            5,
            SourceAttribution::Direct,
        )
        .unwrap();

        assert_eq!(store.highest_tag_claim_revision(&alice).unwrap(), None);
        store.cache_tag_claim(&claim).unwrap();
        store.cache_tag_claim(&claim).unwrap();
        store.cache_collection(&collection).unwrap();
        store.cache_tombstone(&tombstone).unwrap();
        store.cache_moderation_decision(&decision).unwrap();
        store.cache_blocklist(&blocklist).unwrap();

        assert_eq!(store.tag_claims_for(&target).unwrap(), vec![claim]);
        assert_eq!(store.highest_tag_claim_revision(&alice).unwrap(), Some(1));
        assert_eq!(
            store.collection(collection.id()).unwrap(),
            Some(collection.clone())
        );
        assert_eq!(store.collections_for(&alice).unwrap(), vec![collection]);
        assert_eq!(store.tombstones_for(&target).unwrap(), vec![tombstone]);
        assert_eq!(
            store.moderation_decisions_for(&target).unwrap(),
            vec![decision]
        );
        assert_eq!(
            store.blocklist(blocklist.id()).unwrap(),
            Some(blocklist.clone())
        );
        assert_eq!(store.blocklists_for(&alice).unwrap(), vec![blocklist]);
    }

    #[test]
    fn authority_sequence_is_monotonic() {
        let store = Store::in_memory().unwrap();
        let authority = publisher(4);
        assert_eq!(store.highest_authority_sequence(&authority).unwrap(), None);
        assert!(matches!(
            store.record_authority_sequence(&authority, 0),
            Err(Error::InvalidAuthoritySequence)
        ));
        assert!(store.record_authority_sequence(&authority, 8).unwrap());
        assert!(!store.record_authority_sequence(&authority, 7).unwrap());
        assert_eq!(
            store.highest_authority_sequence(&authority).unwrap(),
            Some(8)
        );
    }

    #[test]
    fn blocklist_subscriptions_are_ordered_idempotent_and_removable() {
        let store = Store::in_memory().unwrap();
        let alice = publisher(1);
        let bob = publisher(2);
        let first = CanonicalUri::new("https://example.com/alice/blocklist.json").unwrap();
        let second = CanonicalUri::new("https://example.com/bob/blocklist.json").unwrap();

        store.subscribe_blocklist(&alice, &first).unwrap();
        store.subscribe_blocklist(&alice, &first).unwrap();
        store.subscribe_blocklist(&bob, &second).unwrap();
        let subscriptions = store.blocklist_subscriptions().unwrap();
        assert_eq!(subscriptions.len(), 2);
        assert_eq!(subscriptions[0].issuer, alice);
        assert_eq!(subscriptions[0].source_uri, first);
        assert_eq!(subscriptions[1].issuer, bob.clone());

        store.unsubscribe_blocklist(&bob, &second).unwrap();
        assert_eq!(store.blocklist_subscriptions().unwrap().len(), 1);
    }
}
