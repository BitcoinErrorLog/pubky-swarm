//! Migrated local cache for followed publishers, validated releases, and
//! homeserver event cursors.

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use swarm_protocol::{PublisherId, ReleaseV1};

mod catalog;
mod settings;
mod sources;

pub use catalog::BlocklistSubscription;
pub use settings::{ClientSettings, MAX_LIMIT_KBPS};
pub use sources::CatalogSource;

const CURRENT_SCHEMA_VERSION: i64 = 4;

/// Persistent cache failures.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Filesystem operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// `SQLite` operation failed.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    /// Cached JSON did not match the validated release schema.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A persisted protocol value was corrupt.
    #[error(transparent)]
    Protocol(#[from] swarm_protocol::Error),
    /// The connection lock was poisoned.
    #[error("store connection lock poisoned")]
    LockPoisoned,
    /// System time could not be represented as Unix milliseconds.
    #[error("system clock is before Unix epoch or exceeds u64 milliseconds")]
    InvalidSystemTime,
    /// A persisted publisher identity was corrupt.
    #[error("invalid cached publisher identity: {0}")]
    InvalidPublisher(String),
    /// Signed authority sequences are positive.
    #[error("invalid authority sequence 0")]
    InvalidAuthoritySequence,
    /// Catalog source name is invalid.
    #[error("catalog source name must contain 1..=100 non-control characters")]
    InvalidCatalogName,
    /// Local source count reached the enforced bound.
    #[error("catalog source limit of 32 reached")]
    CatalogSourceLimit,
    /// Built-in catalog sources can be disabled but not deleted.
    #[error("built-in catalog sources cannot be deleted")]
    BuiltInCatalogSource,
    /// Catalog source endpoint or kind is invalid.
    #[error(transparent)]
    Catalog(#[from] catalog_client::Error),
    /// Client settings failed validation.
    #[error("invalid client settings: {0}")]
    InvalidClientSettings(String),
    /// On-disk schema was created by a newer unsupported application.
    #[error("unsupported store schema {found}; maximum supported is {supported}")]
    UnsupportedSchema {
        /// Version read from `SQLite`.
        found: i64,
        /// Highest version this build supports.
        supported: i64,
    },
}

/// Convenience result alias.
pub type Result<T> = std::result::Result<T, Error>;

/// Thread-safe SQLite-backed local cache.
#[derive(Debug)]
pub struct Store {
    connection: Mutex<Connection>,
}

impl Store {
    /// Open or create a store and apply all migrations transactionally.
    ///
    /// # Errors
    ///
    /// Returns filesystem, `SQLite`, migration, or unsupported-schema errors.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        Self::from_connection(connection)
    }

    /// Create a migrated in-memory store.
    ///
    /// # Errors
    ///
    /// Returns `SQLite` or migration errors.
    pub fn in_memory() -> Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(mut connection: Connection) -> Result<Self> {
        connection.pragma_update(None, "foreign_keys", true)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        migrate(&mut connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// Record a followed publisher. Repeated calls are idempotent.
    ///
    /// # Errors
    ///
    /// Returns clock, lock, or `SQLite` errors.
    pub fn follow(&self, publisher: &PublisherId) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO followed_publishers (publisher, added_at)
             VALUES (?1, ?2)
             ON CONFLICT(publisher) DO NOTHING",
            params![publisher.to_string(), unix_millis()?],
        )?;
        Ok(())
    }

    /// Stop following a publisher and remove its cursor, retaining cached
    /// release records for offline provenance.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn unfollow(&self, publisher: &PublisherId) -> Result<()> {
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "DELETE FROM followed_publishers WHERE publisher = ?1",
            [publisher.to_string()],
        )?;
        transaction.execute(
            "DELETE FROM event_cursors WHERE publisher = ?1",
            [publisher.to_string()],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// List followed publishers in insertion order.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or identity validation errors.
    pub fn followed_publishers(&self) -> Result<Vec<PublisherId>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT publisher FROM followed_publishers
             ORDER BY follow_order ASC",
        )?;
        let values = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        values
            .into_iter()
            .map(|value| {
                value
                    .parse::<PublisherId>()
                    .map_err(|error| Error::InvalidPublisher(error.to_string()))
            })
            .collect()
    }

    /// Insert or replace a fully validated release.
    ///
    /// # Errors
    ///
    /// Returns serialization, clock, lock, or `SQLite` errors.
    pub fn cache_release(&self, release: &ReleaseV1) -> Result<()> {
        let json = serde_json::to_string(release)?;
        self.connection()?.execute(
            "INSERT INTO release_cache
                 (publisher, release_id, created_at, release_json, validated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(publisher, release_id) DO UPDATE SET
                 created_at = excluded.created_at,
                 release_json = excluded.release_json,
                 validated_at = excluded.validated_at",
            params![
                release.publisher().to_string(),
                release.id().to_string(),
                release.created_at(),
                json,
                unix_millis()?
            ],
        )?;
        Ok(())
    }

    /// Load validated cached releases newest first.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or release validation errors.
    pub fn releases_for(&self, publisher: &PublisherId) -> Result<Vec<ReleaseV1>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT release_json FROM release_cache
             WHERE publisher = ?1
             ORDER BY created_at DESC, release_id ASC",
        )?;
        let json = statement
            .query_map([publisher.to_string()], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        json.into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .collect()
    }

    /// Load validated releases across all publishers, newest first.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or release validation errors.
    pub fn all_releases(&self, limit: usize) -> Result<Vec<ReleaseV1>> {
        let limit = i64::try_from(limit.min(10_000)).unwrap_or(10_000);
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT release_json FROM release_cache
             ORDER BY created_at DESC, publisher ASC, release_id ASC
             LIMIT ?1",
        )?;
        let json = statement
            .query_map([limit], |row| row.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        json.into_iter()
            .map(|value| serde_json::from_str(&value).map_err(Error::from))
            .collect()
    }

    /// Persist a homeserver-local event cursor.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn set_cursor(&self, publisher: &PublisherId, cursor: u64) -> Result<()> {
        self.connection()?.execute(
            "INSERT INTO event_cursors (publisher, cursor)
             VALUES (?1, ?2)
             ON CONFLICT(publisher) DO UPDATE SET cursor = excluded.cursor",
            params![publisher.to_string(), cursor],
        )?;
        Ok(())
    }

    /// Return the persisted event cursor for a publisher.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn cursor(&self, publisher: &PublisherId) -> Result<Option<u64>> {
        self.connection()?
            .query_row(
                "SELECT cursor FROM event_cursors WHERE publisher = ?1",
                [publisher.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::from)
    }

    /// Current on-disk schema version.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn schema_version(&self) -> Result<i64> {
        self.connection()?
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(Error::from)
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection.lock().map_err(|_| Error::LockPoisoned)
    }
}

fn migrate(connection: &mut Connection) -> Result<()> {
    let mut version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > CURRENT_SCHEMA_VERSION {
        return Err(Error::UnsupportedSchema {
            found: version,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }
    if version == 0 {
        migrate_v1(connection)?;
        version = 1;
    }
    if version == 1 {
        migrate_v2(connection)?;
        version = 2;
    }
    if version == 2 {
        migrate_v3(connection)?;
        version = 3;
    }
    if version == 3 {
        migrate_v4(connection)?;
    }
    Ok(())
}

fn migrate_v1(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE followed_publishers (
             follow_order INTEGER PRIMARY KEY AUTOINCREMENT,
             publisher TEXT UNIQUE NOT NULL,
             added_at INTEGER NOT NULL CHECK (added_at > 0)
         );
         CREATE TABLE release_cache (
             publisher TEXT NOT NULL,
             release_id TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             release_json TEXT NOT NULL,
             validated_at INTEGER NOT NULL CHECK (validated_at > 0),
             PRIMARY KEY (publisher, release_id)
         );
         CREATE INDEX release_cache_publisher_created
             ON release_cache (publisher, created_at DESC);
         CREATE TABLE event_cursors (
             publisher TEXT PRIMARY KEY NOT NULL,
             cursor INTEGER NOT NULL CHECK (cursor >= 0),
             FOREIGN KEY (publisher) REFERENCES followed_publishers(publisher)
                 ON DELETE CASCADE
         );
         PRAGMA user_version = 1;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v2(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE tag_claims (
             claim_id TEXT PRIMARY KEY NOT NULL,
             issuer TEXT NOT NULL,
             subject TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             revision INTEGER NOT NULL CHECK (revision > 0),
             claim_json TEXT NOT NULL
         );
         CREATE INDEX tag_claims_subject_created
             ON tag_claims (subject, created_at DESC, issuer ASC);
         CREATE INDEX tag_claims_issuer_revision
             ON tag_claims (issuer, revision DESC);
         CREATE TABLE collections (
             collection_id TEXT PRIMARY KEY NOT NULL,
             issuer TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             revision INTEGER NOT NULL CHECK (revision > 0),
             collection_json TEXT NOT NULL
         );
         CREATE INDEX collections_issuer_revision
             ON collections (issuer, revision DESC);
         CREATE TABLE tombstones (
             tombstone_id TEXT PRIMARY KEY NOT NULL,
             issuer TEXT NOT NULL,
             subject TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             revision INTEGER NOT NULL CHECK (revision > 0),
             tombstone_json TEXT NOT NULL
         );
         CREATE INDEX tombstones_subject_created
             ON tombstones (subject, created_at DESC, issuer ASC);
         CREATE TABLE moderation_decisions (
             decision_id TEXT PRIMARY KEY NOT NULL,
             issuer TEXT NOT NULL,
             subject TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             revision INTEGER NOT NULL CHECK (revision > 0),
             decision_json TEXT NOT NULL
         );
         CREATE INDEX moderation_decisions_subject_created
             ON moderation_decisions (subject, created_at DESC, issuer ASC);
         CREATE TABLE blocklists (
             blocklist_id TEXT PRIMARY KEY NOT NULL,
             issuer TEXT NOT NULL,
             created_at INTEGER NOT NULL CHECK (created_at > 0),
             revision INTEGER NOT NULL CHECK (revision > 0),
             blocklist_json TEXT NOT NULL
         );
         CREATE INDEX blocklists_issuer_revision
             ON blocklists (issuer, revision DESC);
         CREATE TABLE authority_sequences (
             authority TEXT PRIMARY KEY NOT NULL,
             highest_sequence INTEGER NOT NULL CHECK (highest_sequence >= 0)
         );
         CREATE TABLE blocklist_subscriptions (
             subscription_order INTEGER PRIMARY KEY AUTOINCREMENT,
             issuer TEXT NOT NULL,
             source_uri TEXT NOT NULL,
             added_at INTEGER NOT NULL CHECK (added_at > 0),
             UNIQUE (issuer, source_uri)
         );
         PRAGMA user_version = 2;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v3(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE catalog_sources (
             source_id INTEGER PRIMARY KEY AUTOINCREMENT,
             name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
             kind TEXT NOT NULL CHECK (kind IN ('rss', 'torznab')),
             endpoint TEXT UNIQUE NOT NULL CHECK (length(endpoint) BETWEEN 1 AND 2048),
             enabled INTEGER NOT NULL CHECK (enabled IN (0, 1)),
             built_in INTEGER NOT NULL CHECK (built_in IN (0, 1)),
             requires_api_key INTEGER NOT NULL CHECK (requires_api_key IN (0, 1)),
             added_at INTEGER NOT NULL CHECK (added_at > 0)
         );
         INSERT INTO catalog_sources
             (name, kind, endpoint, enabled, built_in, requires_api_key, added_at)
         VALUES (
             'Academic Torrents — Recent',
             'rss',
             'https://academictorrents.com/rss.xml',
             1,
             1,
             0,
             1
         );
         PRAGMA user_version = 3;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn migrate_v4(connection: &mut Connection) -> Result<()> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "CREATE TABLE client_settings (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             download_dir TEXT CHECK (
                 download_dir IS NULL
                 OR (length(download_dir) BETWEEN 1 AND 4096)
             ),
             dht_enabled INTEGER NOT NULL CHECK (dht_enabled IN (0, 1)),
             upnp_enabled INTEGER NOT NULL CHECK (upnp_enabled IN (0, 1)),
             download_limit_kbps INTEGER CHECK (
                 download_limit_kbps IS NULL
                 OR (download_limit_kbps > 0 AND download_limit_kbps <= 1000000)
             ),
             upload_limit_kbps INTEGER CHECK (
                 upload_limit_kbps IS NULL
                 OR (upload_limit_kbps > 0 AND upload_limit_kbps <= 1000000)
             ),
             listen_port INTEGER CHECK (
                 listen_port IS NULL
                 OR (listen_port BETWEEN 1 AND 65535)
             )
         );
         INSERT INTO client_settings (
             id, download_dir, dht_enabled, upnp_enabled,
             download_limit_kbps, upload_limit_kbps, listen_port
         ) VALUES (1, NULL, 1, 1, NULL, NULL, NULL);
         PRAGMA user_version = 4;",
    )?;
    transaction.commit()?;
    Ok(())
}

fn unix_millis() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidSystemTime)?
        .as_millis();
    u64::try_from(millis).map_err(|_| Error::InvalidSystemTime)
}

#[cfg(test)]
mod tests {
    use pubky::Keypair;
    use swarm_protocol::{InfoHashV1, ReleaseFile, TorrentV1};

    use super::*;

    fn publisher(seed: u8) -> PublisherId {
        PublisherId::new(Keypair::from_secret(&[seed; 32]).public_key())
    }

    fn release(publisher: PublisherId, created_at: u64, hash: u8) -> ReleaseV1 {
        ReleaseV1::new(
            publisher,
            created_at,
            format!("Release {hash}"),
            String::new(),
            TorrentV1 {
                info_hash: InfoHashV1::from_bytes([hash; 20]),
                size: 10,
                files: vec![ReleaseFile {
                    path: "file.bin".to_owned(),
                    size: 10,
                }],
                trackers: Vec::new(),
            },
            Vec::new(),
        )
        .unwrap()
    }

    #[test]
    fn migrates_persistent_store_and_round_trips_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("swarm.sqlite3");
        let alice = publisher(1);
        let bob = publisher(2);
        {
            let store = Store::open(&path).unwrap();
            assert_eq!(store.schema_version().unwrap(), 4);
            store.follow(&alice).unwrap();
            store.follow(&bob).unwrap();
            store.set_cursor(&alice, 42).unwrap();
            store.cache_release(&release(alice.clone(), 10, 1)).unwrap();
            store.cache_release(&release(alice.clone(), 20, 2)).unwrap();
        }
        let reopened = Store::open(path).unwrap();
        assert_eq!(
            reopened.followed_publishers().unwrap(),
            vec![alice.clone(), bob]
        );
        assert_eq!(reopened.cursor(&alice).unwrap(), Some(42));
        let releases = reopened.releases_for(&alice).unwrap();
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].created_at(), 20);
        assert_eq!(reopened.all_releases(1).unwrap()[0].created_at(), 20);

        reopened.unfollow(&alice).unwrap();
        assert_eq!(reopened.cursor(&alice).unwrap(), None);
        assert_eq!(reopened.releases_for(&alice).unwrap().len(), 2);
    }

    #[test]
    fn rejects_newer_schema() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 99).unwrap();
        assert!(matches!(
            Store::from_connection(connection),
            Err(Error::UnsupportedSchema {
                found: 99,
                supported: 4
            })
        ));
    }

    #[test]
    fn migrates_existing_v1_without_losing_state() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE followed_publishers (
                     follow_order INTEGER PRIMARY KEY AUTOINCREMENT,
                     publisher TEXT UNIQUE NOT NULL,
                     added_at INTEGER NOT NULL CHECK (added_at > 0)
                 );
                 CREATE TABLE release_cache (
                     publisher TEXT NOT NULL,
                     release_id TEXT NOT NULL,
                     created_at INTEGER NOT NULL CHECK (created_at > 0),
                     release_json TEXT NOT NULL,
                     validated_at INTEGER NOT NULL CHECK (validated_at > 0),
                     PRIMARY KEY (publisher, release_id)
                 );
                 CREATE INDEX release_cache_publisher_created
                     ON release_cache (publisher, created_at DESC);
                 CREATE TABLE event_cursors (
                     publisher TEXT PRIMARY KEY NOT NULL,
                     cursor INTEGER NOT NULL CHECK (cursor >= 0),
                     FOREIGN KEY (publisher) REFERENCES followed_publishers(publisher)
                         ON DELETE CASCADE
                 );
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        let alice = publisher(8);
        let cached = release(alice.clone(), 50, 5);
        connection
            .execute(
                "INSERT INTO followed_publishers (publisher, added_at) VALUES (?1, 1)",
                [alice.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO event_cursors (publisher, cursor) VALUES (?1, 99)",
                [alice.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO release_cache
                     (publisher, release_id, created_at, release_json, validated_at)
                 VALUES (?1, ?2, ?3, ?4, 1)",
                params![
                    alice.to_string(),
                    cached.id().to_string(),
                    cached.created_at(),
                    serde_json::to_string(&cached).unwrap(),
                ],
            )
            .unwrap();

        migrate(&mut connection).unwrap();
        let store = Store::from_connection(connection).unwrap();
        assert_eq!(store.schema_version().unwrap(), 4);
        assert_eq!(store.followed_publishers().unwrap(), vec![alice.clone()]);
        assert_eq!(store.cursor(&alice).unwrap(), Some(99));
        assert_eq!(store.releases_for(&alice).unwrap(), vec![cached]);
        assert!(store.blocklist_subscriptions().unwrap().is_empty());
        assert_eq!(store.client_settings().unwrap(), ClientSettings::default());
    }

    #[test]
    fn client_settings_round_trip_and_validation() {
        let store = Store::in_memory().unwrap();
        assert_eq!(store.client_settings().unwrap(), ClientSettings::default());

        let mut settings = ClientSettings {
            download_dir: Some(" /tmp/pubky-swarm-downloads ".to_owned()),
            dht_enabled: false,
            upnp_enabled: false,
            download_limit_kbps: Some(0),
            upload_limit_kbps: Some(512),
            listen_port: Some(51413),
        };
        store.set_client_settings(&settings).unwrap();
        settings.download_dir = Some("/tmp/pubky-swarm-downloads".to_owned());
        settings.download_limit_kbps = None;
        assert_eq!(store.client_settings().unwrap(), settings);

        assert!(matches!(
            store.set_client_settings(&ClientSettings {
                listen_port: Some(0),
                ..ClientSettings::default()
            }),
            Err(Error::InvalidClientSettings(_))
        ));
        assert!(matches!(
            store.set_client_settings(&ClientSettings {
                download_limit_kbps: Some(MAX_LIMIT_KBPS + 1),
                ..ClientSettings::default()
            }),
            Err(Error::InvalidClientSettings(_))
        ));
    }
}
