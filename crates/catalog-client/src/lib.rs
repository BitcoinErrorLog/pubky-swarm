//! Bounded parsing and URL construction for opt-in RSS and Torznab catalogs.

#![forbid(unsafe_code)]

use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::str::FromStr;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};
use serde::{Deserialize, Serialize};
use url::Url;

/// Maximum accepted XML response size.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
/// Maximum sources persisted by one desktop profile.
pub const MAX_SOURCES: usize = 32;
/// Maximum normalized results returned by one source.
pub const MAX_RESULTS: usize = 100;

/// Supported external catalog protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// RSS 2.0 feed containing magnets or v1 infohashes.
    Rss,
    /// Torznab-compatible search endpoint.
    Torznab,
}

impl Display for SourceKind {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Rss => "rss",
            Self::Torznab => "torznab",
        })
    }
}

impl FromStr for SourceKind {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "rss" => Ok(Self::Rss),
            "torznab" => Ok(Self::Torznab),
            _ => Err(Error::InvalidKind),
        }
    }
}

/// One normalized, non-authoritative catalog result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    /// Persistent local source identifier.
    pub source_id: i64,
    /// Human-readable source name.
    pub source_name: String,
    /// Result title.
    pub title: String,
    /// Bounded plain-text description.
    pub description: String,
    /// Actionable magnet URI.
    pub magnet: String,
    /// Lowercase hexadecimal v1 infohash when known.
    pub info_hash: Option<String>,
    /// Payload size reported by the source.
    pub size: Option<u64>,
    /// Non-authoritative source categories.
    pub tags: Vec<String>,
    /// Credential-free details page when supplied.
    pub details_url: Option<String>,
}

/// Catalog parsing or validation failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Source kind is unsupported.
    #[error("catalog kind must be rss or torznab")]
    InvalidKind,
    /// Source URL is malformed or violates transport policy.
    #[error("invalid catalog URL: {0}")]
    InvalidUrl(String),
    /// Search query is too large.
    #[error("catalog query exceeds 256 characters")]
    QueryTooLong,
    /// API key is malformed.
    #[error("catalog API key exceeds 512 characters or contains control characters")]
    InvalidApiKey,
    /// XML could not be decoded safely.
    #[error("invalid catalog XML: {0}")]
    Xml(String),
}

/// Validate and normalize a source endpoint.
///
/// Remote sources require HTTPS. Loopback HTTP is allowed for local Jackett
/// and Prowlarr installations. URL credentials, fragments, sensitive query
/// parameters, and explicit non-loopback IP literals are rejected.
///
/// # Errors
///
/// Returns [`Error::InvalidUrl`] when the endpoint violates these rules.
pub fn validate_source_url(value: &str) -> Result<Url, Error> {
    if value.len() > 2_048 {
        return Err(Error::InvalidUrl("URL exceeds 2048 bytes".to_owned()));
    }
    let url = Url::parse(value).map_err(|error| Error::InvalidUrl(error.to_string()))?;
    if url.cannot_be_a_base() || url.host_str().is_none() {
        return Err(Error::InvalidUrl(
            "URL must be an absolute HTTP(S) endpoint".to_owned(),
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::InvalidUrl(
            "credentials must not be embedded in the URL".to_owned(),
        ));
    }
    if url.fragment().is_some() {
        return Err(Error::InvalidUrl("fragments are not allowed".to_owned()));
    }
    for (name, _) in url.query_pairs() {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "apikey" | "api_key" | "token" | "access_token" | "key"
        ) {
            return Err(Error::InvalidUrl(
                "API keys must use the separate in-memory credential field".to_owned(),
            ));
        }
    }
    let loopback = is_loopback_host(&url);
    match url.scheme() {
        "https" => {}
        "http" if loopback => {}
        "http" => {
            return Err(Error::InvalidUrl(
                "remote catalog sources require HTTPS".to_owned(),
            ));
        }
        _ => {
            return Err(Error::InvalidUrl(
                "URL scheme must be HTTP or HTTPS".to_owned(),
            ));
        }
    }
    if let Some(host) = url.host_str()
        && let Ok(address) = host.parse::<IpAddr>()
        && !address.is_loopback()
    {
        return Err(Error::InvalidUrl(
            "explicit non-loopback IP addresses are not allowed".to_owned(),
        ));
    }
    Ok(url)
}

/// Build a Torznab search URL without mutating the persisted source endpoint.
///
/// # Errors
///
/// Rejects an oversized query or malformed API key.
pub fn torznab_search_url(
    endpoint: &Url,
    query: &str,
    limit: usize,
    api_key: Option<&str>,
) -> Result<Url, Error> {
    if query.chars().count() > 256 {
        return Err(Error::QueryTooLong);
    }
    if api_key.is_some_and(|value| value.len() > 512 || value.chars().any(char::is_control)) {
        return Err(Error::InvalidApiKey);
    }
    let mut url = endpoint.clone();
    {
        let mut pairs = url.query_pairs_mut();
        pairs.append_pair("t", "search");
        pairs.append_pair("q", query.trim());
        pairs.append_pair("limit", &limit.clamp(1, MAX_RESULTS).to_string());
        if let Some(api_key) = api_key.filter(|value| !value.is_empty()) {
            pairs.append_pair("apikey", api_key);
        }
    }
    Ok(url)
}

/// Parse RSS or Torznab XML into bounded actionable magnet results.
///
/// Items without a magnet URI or a valid v1 infohash are omitted.
///
/// # Errors
///
/// Returns [`Error::Xml`] when the document is not well-formed XML.
pub fn parse_catalog(
    source_id: i64,
    source_name: &str,
    xml: &[u8],
    limit: usize,
) -> Result<Vec<CatalogItem>, Error> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut results = Vec::new();
    let mut item = None::<ItemBuilder>;
    let mut field = Field::None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(start)) => {
                let local = start.local_name();
                let name = local.as_ref();
                if name == b"item" {
                    item = Some(ItemBuilder::default());
                    field = Field::None;
                } else if item.is_some() {
                    field = Field::from_name(name);
                    if name == b"enclosure" {
                        apply_enclosure(&reader, &start, current_item(&mut item)?)?;
                    } else if name == b"attr" {
                        apply_torznab_attribute(&reader, &start, current_item(&mut item)?)?;
                    }
                }
            }
            Ok(Event::Empty(start)) if item.is_some() => {
                let local = start.local_name();
                let name = local.as_ref();
                if name == b"enclosure" {
                    apply_enclosure(&reader, &start, current_item(&mut item)?)?;
                } else if name == b"attr" {
                    apply_torznab_attribute(&reader, &start, current_item(&mut item)?)?;
                }
            }
            Ok(Event::Text(text)) if item.is_some() => {
                let value = text
                    .xml_content(XmlVersion::Implicit1_0)
                    .map_err(|error| Error::Xml(error.to_string()))?;
                apply_text(current_item(&mut item)?, field, &value);
            }
            Ok(Event::CData(text)) if item.is_some() => {
                let value = text
                    .decode()
                    .map_err(|error| Error::Xml(error.to_string()))?;
                apply_text(current_item(&mut item)?, field, &value);
            }
            Ok(Event::GeneralRef(reference)) if item.is_some() => {
                let value = if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|error| Error::Xml(error.to_string()))?
                {
                    character.to_string()
                } else {
                    let name = reference
                        .decode()
                        .map_err(|error| Error::Xml(error.to_string()))?;
                    match name.as_ref() {
                        "lt" => "<",
                        "gt" => ">",
                        "amp" => "&",
                        "apos" => "'",
                        "quot" => "\"",
                        _ => {
                            return Err(Error::Xml(format!(
                                "unsupported entity reference &{name};"
                            )));
                        }
                    }
                    .to_owned()
                };
                apply_text(current_item(&mut item)?, field, &value);
            }
            Ok(Event::End(end)) => {
                let name = end.local_name();
                if name.as_ref() == b"item" {
                    if let Some(candidate) = item.take()
                        && let Some(result) = candidate.finish(source_id, source_name)
                    {
                        results.push(result);
                        if results.len() >= limit.clamp(1, MAX_RESULTS) {
                            break;
                        }
                    }
                    field = Field::None;
                } else if item.is_some() && Field::from_name(name.as_ref()) == field {
                    field = Field::None;
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(Error::Xml(error.to_string())),
        }
    }
    Ok(results)
}

fn current_item(item: &mut Option<ItemBuilder>) -> Result<&mut ItemBuilder, Error> {
    item.as_mut()
        .ok_or_else(|| Error::Xml("item parser state was lost".to_owned()))
}

fn is_loopback_host(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    None,
    Title,
    Description,
    Link,
    Guid,
    Category,
    InfoHash,
    Size,
}

impl Field {
    fn from_name(name: &[u8]) -> Self {
        match name {
            b"title" => Self::Title,
            b"description" => Self::Description,
            b"link" => Self::Link,
            b"guid" => Self::Guid,
            b"category" => Self::Category,
            b"infohash" => Self::InfoHash,
            b"size" => Self::Size,
            _ => Self::None,
        }
    }
}

#[derive(Default)]
struct ItemBuilder {
    title: String,
    description: String,
    link: String,
    guid: String,
    enclosure: String,
    magnet: String,
    info_hash: String,
    size: Option<u64>,
    tags: Vec<String>,
}

impl ItemBuilder {
    fn finish(self, source_id: i64, source_name: &str) -> Option<CatalogItem> {
        let info_hash = normalize_info_hash(&self.info_hash)
            .or_else(|| magnet_info_hash(&self.magnet))
            .or_else(|| magnet_info_hash(&self.enclosure))
            .or_else(|| magnet_info_hash(&self.link));
        let magnet = [&self.magnet, &self.enclosure, &self.link, &self.guid]
            .into_iter()
            .find(|value| value.starts_with("magnet:?"))
            .cloned()
            .or_else(|| {
                info_hash.as_ref().map(|hash| {
                    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
                    serializer.append_pair("xt", &format!("urn:btih:{hash}"));
                    if !self.title.trim().is_empty() {
                        serializer.append_pair("dn", self.title.trim());
                    }
                    format!("magnet:?{}", serializer.finish())
                })
            })?;
        if magnet.len() > 8_192 || self.title.trim().is_empty() {
            return None;
        }
        let details_url = [&self.link, &self.guid]
            .into_iter()
            .find_map(|value| credential_free_http_url(value));
        let mut tags = self
            .tags
            .into_iter()
            .filter_map(|tag| normalize_tag(&tag))
            .collect::<Vec<_>>();
        tags.sort();
        tags.dedup();
        tags.truncate(32);
        Some(CatalogItem {
            source_id,
            source_name: truncate_chars(source_name.trim(), 100),
            title: truncate_chars(self.title.trim(), 200),
            description: truncate_chars(self.description.trim(), 4_000),
            magnet,
            info_hash,
            size: self.size,
            tags,
            details_url,
        })
    }
}

fn apply_text(item: &mut ItemBuilder, field: Field, value: &str) {
    match field {
        Field::Title => append_text(&mut item.title, value, 200),
        Field::Description => append_text(&mut item.description, value, 4_000),
        Field::Link => append_text(&mut item.link, value, 8_192),
        Field::Guid => append_text(&mut item.guid, value, 8_192),
        Field::Category => {
            if item.tags.len() < 64 {
                item.tags.push(truncate_chars(value.trim(), 100));
            }
        }
        Field::InfoHash => append_text(&mut item.info_hash, value, 64),
        Field::Size => {
            if item.size.is_none() {
                item.size = value.trim().parse().ok();
            }
        }
        Field::None => {}
    }
}

fn apply_enclosure(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    item: &mut ItemBuilder,
) -> Result<(), Error> {
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let name = attribute.key.local_name();
        let value = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Xml(error.to_string()))?;
        match name.as_ref() {
            b"url" => item.enclosure = truncate_chars(&value, 8_192),
            b"length" if item.size.is_none() => item.size = value.parse().ok(),
            _ => {}
        }
    }
    Ok(())
}

fn apply_torznab_attribute(
    reader: &Reader<&[u8]>,
    element: &BytesStart<'_>,
    item: &mut ItemBuilder,
) -> Result<(), Error> {
    let mut name = None;
    let mut value = None;
    for attribute in element.attributes().with_checks(false) {
        let attribute = attribute.map_err(|error| Error::Xml(error.to_string()))?;
        let decoded = attribute
            .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
            .map_err(|error| Error::Xml(error.to_string()))?;
        match attribute.key.local_name().as_ref() {
            b"name" => name = Some(decoded.into_owned()),
            b"value" => value = Some(decoded.into_owned()),
            _ => {}
        }
    }
    let (Some(name), Some(value)) = (name, value) else {
        return Ok(());
    };
    match name.to_ascii_lowercase().as_str() {
        "magneturl" => item.magnet = truncate_chars(&value, 8_192),
        "infohash" => item.info_hash = truncate_chars(&value, 64),
        "size" if item.size.is_none() => item.size = value.parse().ok(),
        "tag" | "category" if item.tags.len() < 64 => {
            item.tags.push(truncate_chars(&value, 100));
        }
        _ => {}
    }
    Ok(())
}

fn append_text(target: &mut String, value: &str, maximum: usize) {
    if !target.is_empty() && target.chars().count() < maximum {
        target.push(' ');
    }
    let remaining = maximum.saturating_sub(target.chars().count());
    target.push_str(&truncate_chars(value.trim(), remaining));
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn normalize_info_hash(value: &str) -> Option<String> {
    let value = value.trim();
    (value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| value.to_ascii_lowercase())
}

fn magnet_info_hash(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    if url.scheme() != "magnet" {
        return None;
    }
    url.query_pairs()
        .find_map(|(name, value)| {
            (name.eq_ignore_ascii_case("xt"))
                .then_some(value)
                .and_then(|value| value.strip_prefix("urn:btih:").map(str::to_owned))
        })
        .and_then(|value| normalize_info_hash(&value))
}

fn credential_free_http_url(value: &str) -> Option<String> {
    let url = Url::parse(value.trim()).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}

fn normalize_tag(value: &str) -> Option<String> {
    let value = value.trim().to_lowercase();
    (!value.is_empty() && value.chars().count() <= 64 && !value.chars().any(char::is_control))
        .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn validates_remote_and_loopback_transport_policy() {
        assert!(validate_source_url("https://example.com/feed.xml").is_ok());
        assert!(validate_source_url("http://127.0.0.1:9117/api").is_ok());
        assert!(validate_source_url("http://localhost:9696/api").is_ok());
        assert!(validate_source_url("http://example.com/feed").is_err());
        assert!(validate_source_url("https://192.168.1.2/api").is_err());
        assert!(validate_source_url("https://user:pass@example.com/api").is_err());
        assert!(validate_source_url("https://example.com/api?apikey=secret").is_err());
    }

    #[test]
    fn builds_encoded_torznab_queries_without_persisting_key() {
        let endpoint = validate_source_url("https://indexer.example/api").unwrap();
        let url = torznab_search_url(&endpoint, "open film", 500, Some("secret")).unwrap();
        let values = url.query_pairs().collect::<Vec<_>>();
        assert!(values.contains(&("t".into(), "search".into())));
        assert!(values.contains(&("q".into(), "open film".into())));
        assert!(values.contains(&("limit".into(), "100".into())));
        assert!(values.contains(&("apikey".into(), "secret".into())));
        assert_eq!(endpoint.as_str(), "https://indexer.example/api");
    }

    #[test]
    fn parses_academic_torrents_style_rss() {
        let xml = format!(
            r#"<?xml version="1.0"?>
            <rss xmlns:academictorrents="http://academictorrents.com" version="2.0">
              <channel><item>
                <title>Research &amp; Data</title>
                <category>Dataset</category>
                <academictorrents:infohash>{HASH}</academictorrents:infohash>
                <link>https://academictorrents.com/details/{HASH}</link>
                <description>Open research corpus</description>
                <academictorrents:size>42</academictorrents:size>
              </item></channel>
            </rss>"#
        );
        let items = parse_catalog(1, "Academic Torrents", xml.as_bytes(), 25).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Research & Data");
        assert_eq!(items[0].info_hash.as_deref(), Some(HASH));
        assert_eq!(items[0].size, Some(42));
        assert_eq!(items[0].tags, vec!["dataset"]);
        assert!(items[0].magnet.contains(HASH));
    }

    #[test]
    fn parses_torznab_magnet_attributes_and_bounds_results() {
        let xml = format!(
            r#"<?xml version="1.0"?>
            <rss xmlns:torznab="http://torznab.com/schemas/2015/feed">
              <channel><item>
                <title>Open Film</title>
                <description><![CDATA[A public-domain film]]></description>
                <torznab:attr name="infohash" value="{HASH}" />
                <torznab:attr name="magneturl" value="magnet:?xt=urn:btih:{HASH}&amp;dn=Open%20Film" />
                <torznab:attr name="size" value="1024" />
                <torznab:attr name="tag" value="Film" />
              </item></channel>
            </rss>"#
        );
        let items = parse_catalog(2, "Local Torznab", xml.as_bytes(), 1).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].size, Some(1_024));
        assert_eq!(items[0].tags, vec!["film"]);
        assert_eq!(items[0].info_hash.as_deref(), Some(HASH));
    }

    #[test]
    fn omits_non_actionable_items() {
        let xml = br"<rss><channel><item><title>No torrent</title></item></channel></rss>";
        assert!(parse_catalog(1, "feed", xml, 10).unwrap().is_empty());
    }
}
