//! Persistent opt-in external catalog source configuration.

use catalog_client::{MAX_SOURCES, SourceKind, validate_source_url};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use url::Url;

use crate::{Error, Result, Store, unix_millis};

/// One shipped RSS preset. Inserted as `built_in` on migrate / open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltInRssPreset {
    /// Display name shown in Discover.
    pub name: &'static str,
    /// Credential-free HTTPS RSS endpoint.
    pub endpoint: &'static str,
    /// Whether new installs enable this feed by default.
    pub enabled_by_default: bool,
    /// Short non-authoritative description for the UI.
    pub description: &'static str,
}

/// Curated public RSS feeds (open research / public datasets). Not scrapers.
pub const BUILT_IN_RSS_PRESETS: &[BuiltInRssPreset] = &[
    BuiltInRssPreset {
        name: "Academic Torrents — Recent",
        endpoint: "https://academictorrents.com/rss.xml",
        enabled_by_default: true,
        description: "Latest public research datasets and papers on Academic Torrents.",
    },
    BuiltInRssPreset {
        name: "Academic Torrents — Deep Learning",
        endpoint: "https://academictorrents.com/collection/deep-learning.xml",
        enabled_by_default: false,
        description: "Deep-learning datasets mirrored through Academic Torrents.",
    },
    BuiltInRssPreset {
        name: "Academic Torrents — Computer Vision",
        endpoint: "https://academictorrents.com/collection/computer-vision.xml",
        enabled_by_default: false,
        description: "Computer-vision datasets from the Academic Torrents collection.",
    },
    BuiltInRssPreset {
        name: "Academic Torrents — Medical",
        endpoint: "https://academictorrents.com/collection/medical.xml",
        enabled_by_default: false,
        description: "Medical and life-sciences datasets on Academic Torrents.",
    },
    BuiltInRssPreset {
        name: "Academic Torrents — Video Lectures",
        endpoint: "https://academictorrents.com/collection/video-lectures.xml",
        enabled_by_default: false,
        description: "Academic video lectures distributed as torrents.",
    },
    BuiltInRssPreset {
        name: "Academic Torrents — English Wikipedia",
        endpoint: "https://academictorrents.com/collection/english-wikipedia.xml",
        enabled_by_default: false,
        description: "English Wikipedia dumps mirrored via Academic Torrents.",
    },
];

/// One locally configured external catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSource {
    /// Stable local database identifier.
    pub id: i64,
    /// Human-readable source name.
    pub name: String,
    /// Feed protocol.
    pub kind: SourceKind,
    /// Validated credential-free endpoint.
    pub endpoint: String,
    /// Whether searches currently query this source.
    pub enabled: bool,
    /// Whether this source shipped with the application.
    pub built_in: bool,
    /// Whether searches need a session-only API key.
    pub requires_api_key: bool,
    /// Local creation time in Unix milliseconds.
    pub added_at: u64,
}

/// Insert any missing built-in RSS presets without changing existing rows.
pub(crate) fn ensure_built_in_rss_presets(connection: &Connection) -> Result<()> {
    let mut insert = connection.prepare(
        "INSERT INTO catalog_sources
             (name, kind, endpoint, enabled, built_in, requires_api_key, added_at)
         SELECT ?1, 'rss', ?2, ?3, 1, 0, ?4
         WHERE NOT EXISTS (
             SELECT 1 FROM catalog_sources WHERE endpoint = ?2
         )",
    )?;
    for preset in BUILT_IN_RSS_PRESETS {
        insert.execute(params![
            preset.name,
            preset.endpoint,
            i64::from(preset.enabled_by_default),
            1_i64,
        ])?;
    }
    Ok(())
}

impl Store {
    /// Add a user-controlled RSS or Torznab source.
    ///
    /// API keys are deliberately excluded from persistence.
    ///
    /// # Errors
    ///
    /// Rejects invalid names, endpoints, duplicates, or source counts above 32.
    pub fn add_catalog_source(
        &self,
        name: &str,
        kind: SourceKind,
        endpoint: &str,
        requires_api_key: bool,
    ) -> Result<CatalogSource> {
        let name = validate_name(name)?;
        let endpoint = validate_source_url(endpoint)?.to_string();
        let mut connection = self.connection()?;
        let transaction = connection.transaction()?;
        let count: usize =
            transaction.query_row("SELECT COUNT(*) FROM catalog_sources", [], |row| row.get(0))?;
        if count >= MAX_SOURCES {
            return Err(Error::CatalogSourceLimit);
        }
        transaction.execute(
            "INSERT INTO catalog_sources
                 (name, kind, endpoint, enabled, built_in, requires_api_key, added_at)
             VALUES (?1, ?2, ?3, 1, 0, ?4, ?5)",
            params![
                name,
                kind.to_string(),
                endpoint,
                requires_api_key,
                unix_millis()?
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        drop(connection);
        self.catalog_source(id)?
            .ok_or(rusqlite::Error::QueryReturnedNoRows.into())
    }

    /// Add an RSS feed from a pasted URL, deriving a name when none is given.
    ///
    /// # Errors
    ///
    /// Same as [`Store::add_catalog_source`].
    pub fn add_rss_feed(&self, feed_url: &str, name: Option<&str>) -> Result<CatalogSource> {
        let endpoint = validate_source_url(feed_url)?;
        let resolved_name = match name.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => validate_name(value)?,
            None => validate_name(&default_feed_name(&endpoint))?,
        };
        self.add_catalog_source(&resolved_name, SourceKind::Rss, endpoint.as_str(), false)
    }

    /// List external catalogs in stable insertion order.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or persisted-kind errors.
    pub fn catalog_sources(&self) -> Result<Vec<CatalogSource>> {
        let connection = self.connection()?;
        let mut statement = connection.prepare(
            "SELECT source_id, name, kind, endpoint, enabled, built_in,
                    requires_api_key, added_at
             FROM catalog_sources
             ORDER BY source_id ASC",
        )?;
        let rows = statement
            .query_map([], map_source)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        rows.into_iter().map(parse_source).collect()
    }

    /// Return one configured source.
    ///
    /// # Errors
    ///
    /// Returns lock, `SQLite`, or persisted-kind errors.
    pub fn catalog_source(&self, id: i64) -> Result<Option<CatalogSource>> {
        self.connection()?
            .query_row(
                "SELECT source_id, name, kind, endpoint, enabled, built_in,
                        requires_api_key, added_at
                 FROM catalog_sources
                 WHERE source_id = ?1",
                [id],
                map_source,
            )
            .optional()?
            .map(parse_source)
            .transpose()
    }

    /// Enable or disable a configured source.
    ///
    /// Returns `false` when no source has the supplied identifier.
    ///
    /// # Errors
    ///
    /// Returns lock or `SQLite` errors.
    pub fn set_catalog_source_enabled(&self, id: i64, enabled: bool) -> Result<bool> {
        Ok(self.connection()?.execute(
            "UPDATE catalog_sources SET enabled = ?1 WHERE source_id = ?2",
            params![enabled, id],
        )? == 1)
    }

    /// Delete a user-added source.
    ///
    /// Built-in sources can be disabled but cannot be deleted. Returns `false`
    /// when the identifier does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BuiltInCatalogSource`] for built-ins, or lock and
    /// `SQLite` errors.
    pub fn remove_catalog_source(&self, id: i64) -> Result<bool> {
        let built_in = self
            .connection()?
            .query_row(
                "SELECT built_in FROM catalog_sources WHERE source_id = ?1",
                [id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        match built_in {
            Some(true) => Err(Error::BuiltInCatalogSource),
            Some(false) => Ok(self
                .connection()?
                .execute("DELETE FROM catalog_sources WHERE source_id = ?1", [id])?
                == 1),
            None => Ok(false),
        }
    }
}

type SourceRow = (i64, String, String, String, bool, bool, bool, u64);

fn map_source(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
    ))
}

fn parse_source(row: SourceRow) -> Result<CatalogSource> {
    Ok(CatalogSource {
        id: row.0,
        name: row.1,
        kind: row.2.parse()?,
        endpoint: row.3,
        enabled: row.4,
        built_in: row.5,
        requires_api_key: row.6,
        added_at: row.7,
    })
}

fn validate_name(value: &str) -> Result<String> {
    let value = value.trim();
    let length = value.chars().count();
    if !(1..=100).contains(&length) || value.chars().any(char::is_control) {
        return Err(Error::InvalidCatalogName);
    }
    Ok(value.to_owned())
}

fn default_feed_name(endpoint: &Url) -> String {
    let host = endpoint.host_str().unwrap_or("RSS feed");
    let path = endpoint
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty() && *segment != "/")
        .unwrap_or("feed");
    let cleaned = path.trim_end_matches(".xml").trim_end_matches(".rss");
    let label = if cleaned.is_empty() || cleaned == "rss" || cleaned == "feed" {
        host.to_owned()
    } else {
        format!("{host} — {cleaned}")
    };
    label.chars().take(100).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_and_user_sources_round_trip() {
        let store = Store::in_memory().unwrap();
        let initial = store.catalog_sources().unwrap();
        assert_eq!(initial.len(), BUILT_IN_RSS_PRESETS.len());
        assert!(initial.iter().all(|source| source.built_in));
        assert_eq!(initial[0].kind, SourceKind::Rss);
        assert!(
            initial
                .iter()
                .any(|source| source.endpoint == BUILT_IN_RSS_PRESETS[0].endpoint && source.enabled)
        );
        assert!(
            initial
                .iter()
                .filter(|source| source.endpoint != BUILT_IN_RSS_PRESETS[0].endpoint)
                .all(|source| !source.enabled)
        );

        let source = store
            .add_catalog_source(
                "Local Jackett",
                SourceKind::Torznab,
                "http://127.0.0.1:9117/api/v2.0/indexers/all/results/torznab/api",
                true,
            )
            .unwrap();
        assert!(source.enabled);
        assert!(source.requires_api_key);
        assert!(!source.built_in);
        assert!(store.set_catalog_source_enabled(source.id, false).unwrap());
        assert!(!store.catalog_source(source.id).unwrap().unwrap().enabled);
        assert!(store.remove_catalog_source(source.id).unwrap());
        assert!(store.catalog_source(source.id).unwrap().is_none());
        assert!(matches!(
            store.remove_catalog_source(initial[0].id),
            Err(Error::BuiltInCatalogSource)
        ));
    }

    #[test]
    fn add_rss_feed_derives_name_from_url() {
        let store = Store::in_memory().unwrap();
        let source = store
            .add_rss_feed(
                "https://example.org/public/open-datasets.xml",
                None,
            )
            .unwrap();
        assert_eq!(source.name, "example.org — open-datasets");
        assert_eq!(source.kind, SourceKind::Rss);
        assert!(!source.built_in);
        assert!(source.enabled);
    }

    #[test]
    fn rejects_insecure_or_credential_bearing_sources() {
        let store = Store::in_memory().unwrap();
        assert!(
            store
                .add_catalog_source(
                    "Insecure",
                    SourceKind::Rss,
                    "http://example.com/feed",
                    false
                )
                .is_err()
        );
        assert!(
            store
                .add_catalog_source(
                    "Secret",
                    SourceKind::Torznab,
                    "https://example.com/api?apikey=secret",
                    true
                )
                .is_err()
        );
    }

    #[test]
    fn ensure_presets_is_idempotent() {
        let store = Store::in_memory().unwrap();
        let before = store.catalog_sources().unwrap();
        {
            let connection = store.connection().unwrap();
            ensure_built_in_rss_presets(&connection).unwrap();
        }
        assert_eq!(store.catalog_sources().unwrap(), before);
    }
}
