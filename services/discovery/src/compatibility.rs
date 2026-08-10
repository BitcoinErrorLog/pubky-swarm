//! Lossy compatibility representations backed by validated cached releases.

use swarm_protocol::ReleaseV1;

pub(crate) const AUTHORITY_SIGNAL: &str = "non-authoritative-client-validation-required";
pub(crate) const TORZNAB_CATEGORY: &str = "8000";
pub(crate) const MAX_QUERY_CHARS: usize = 256;
pub(crate) const MAX_RESULTS: usize = 100;
pub(crate) const MAX_OFFSET: usize = 10_000;

pub(crate) fn validate_query(value: &str) -> Result<&str, &'static str> {
    let value = value.trim();
    if value.chars().count() > MAX_QUERY_CHARS {
        return Err("query exceeds 256 characters");
    }
    Ok(value)
}

pub(crate) fn matches(release: &ReleaseV1, needle: &str, tag: Option<&str>) -> bool {
    let matches_tag = tag.is_none_or(|wanted| {
        release
            .tags()
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(wanted))
    });
    if !matches_tag {
        return false;
    }
    if needle.is_empty() {
        return true;
    }
    let needle = needle.to_lowercase();
    release.title().to_lowercase().contains(&needle)
        || release.description().to_lowercase().contains(&needle)
        || release.tags().iter().any(|tag| tag.contains(&needle))
        || release.publisher().to_string().contains(&needle)
        || release.torrent().info_hash.to_string().contains(&needle)
}

pub(crate) fn details_url(base_url: &str, release: &ReleaseV1) -> String {
    format!(
        "{base_url}/v1/publishers/{}/releases/{}",
        release.publisher(),
        release.id()
    )
}

pub(crate) fn magnet_url(release: &ReleaseV1) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("xt", &format!("urn:btih:{}", release.torrent().info_hash));
    serializer.append_pair("dn", release.title());
    for tracker in &release.torrent().trackers {
        serializer.append_pair("tr", tracker);
    }
    format!("magnet:?{}", serializer.finish())
}

pub(crate) fn rss_feed(
    title: &str,
    description: &str,
    self_url: &str,
    base_url: &str,
    releases: &[ReleaseV1],
) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" \
         xmlns:pubky=\"https://pubky.org/swarm/discovery/1.0\">\n<channel>\n",
    );
    element(&mut xml, "title", title);
    element(&mut xml, "link", base_url);
    element(&mut xml, "description", description);
    element(&mut xml, "pubky:authority", AUTHORITY_SIGNAL);
    xml.push_str("<atom:link href=\"");
    attribute(&mut xml, self_url);
    xml.push_str("\" rel=\"self\" type=\"application/rss+xml\" />\n");
    for release in releases {
        rss_item(&mut xml, base_url, release);
    }
    xml.push_str("</channel>\n</rss>\n");
    xml
}

fn rss_item(xml: &mut String, base_url: &str, release: &ReleaseV1) {
    let details = details_url(base_url, release);
    let magnet = magnet_url(release);
    xml.push_str("<item>\n");
    element(xml, "title", release.title());
    element(xml, "link", &details);
    xml.push_str("<guid isPermaLink=\"true\">");
    text(xml, &details);
    xml.push_str("</guid>\n");
    let description = if release.description().is_empty() {
        format!("Torky cached release. {AUTHORITY_SIGNAL}.")
    } else {
        format!(
            "{}\n\nTorky cached release. {AUTHORITY_SIGNAL}.",
            release.description()
        )
    };
    element(xml, "description", &description);
    element(xml, "pubky:publisher", &release.publisher().to_string());
    element(xml, "pubky:releaseId", &release.id().to_string());
    element(
        xml,
        "pubky:infoHash",
        &release.torrent().info_hash.to_string(),
    );
    element(xml, "pubky:authority", AUTHORITY_SIGNAL);
    for tag in release.tags() {
        element(xml, "category", tag);
    }
    xml.push_str("<enclosure url=\"");
    attribute(xml, &magnet);
    xml.push_str("\" length=\"");
    xml.push_str(&release.torrent().size.to_string());
    xml.push_str("\" type=\"application/x-bittorrent\" />\n</item>\n");
}

pub(crate) fn torznab_caps(base_url: &str) -> String {
    let search_url = format!("{base_url}/api");
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<caps>\n");
    xml.push_str("<server version=\"1.0\" title=\"Torky Discovery\" />\n");
    xml.push_str("<limits max=\"100\" default=\"25\" />\n");
    xml.push_str(
        "<searching><search available=\"yes\" supportedParams=\"q,cat,limit,offset,tag\" />",
    );
    xml.push_str("<tv-search available=\"no\" supportedParams=\"\" />");
    xml.push_str("<movie-search available=\"no\" supportedParams=\"\" />");
    xml.push_str("<audio-search available=\"no\" supportedParams=\"\" />");
    xml.push_str("<book-search available=\"no\" supportedParams=\"\" /></searching>\n");
    xml.push_str("<categories><category id=\"");
    xml.push_str(TORZNAB_CATEGORY);
    xml.push_str("\" name=\"Torky\" /></categories>\n");
    xml.push_str("<tags><tag name=\"non-authoritative\" description=\"");
    attribute(&mut xml, AUTHORITY_SIGNAL);
    xml.push_str("\" /></tags>\n");
    xml.push_str("<pubky:metadata xmlns:pubky=\"https://pubky.org/swarm/discovery/1.0\" ");
    xml.push_str("authority=\"");
    attribute(&mut xml, AUTHORITY_SIGNAL);
    xml.push_str("\" releaseCache=\"swarm-store\" observations=\"unavailable\" searchUrl=\"");
    attribute(&mut xml, &search_url);
    xml.push_str("\" />\n</caps>\n");
    xml
}

pub(crate) fn torznab_feed(
    self_url: &str,
    base_url: &str,
    offset: usize,
    total: usize,
    releases: &[ReleaseV1],
) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <rss version=\"2.0\" xmlns:atom=\"http://www.w3.org/2005/Atom\" \
         xmlns:torznab=\"http://torznab.com/schemas/2015/feed\" \
         xmlns:pubky=\"https://pubky.org/swarm/discovery/1.0\">\n<channel>\n",
    );
    element(&mut xml, "title", "Torky Discovery");
    element(&mut xml, "link", base_url);
    element(
        &mut xml,
        "description",
        "Lossy Torznab view of the opt-in validated release cache",
    );
    element(&mut xml, "pubky:authority", AUTHORITY_SIGNAL);
    xml.push_str("<atom:link href=\"");
    attribute(&mut xml, self_url);
    xml.push_str("\" rel=\"self\" type=\"application/rss+xml\" />\n");
    xml.push_str("<newznab:response xmlns:newznab=\"http://www.newznab.com/DTD/2010/feeds/attributes/\" offset=\"");
    xml.push_str(&offset.to_string());
    xml.push_str("\" total=\"");
    xml.push_str(&total.to_string());
    xml.push_str("\" />\n");
    for release in releases {
        torznab_item(&mut xml, base_url, release);
    }
    xml.push_str("</channel>\n</rss>\n");
    xml
}

fn torznab_item(xml: &mut String, base_url: &str, release: &ReleaseV1) {
    let details = details_url(base_url, release);
    let magnet = magnet_url(release);
    xml.push_str("<item>\n");
    element(xml, "title", release.title());
    element(xml, "guid", &details);
    element(xml, "link", &magnet);
    element(xml, "comments", &details);
    element(xml, "category", TORZNAB_CATEGORY);
    let description = format!(
        "{}\n\nDetails and provenance: {details}\n{AUTHORITY_SIGNAL}.",
        release.description()
    );
    element(xml, "description", &description);
    xml.push_str("<enclosure url=\"");
    attribute(xml, &magnet);
    xml.push_str("\" length=\"");
    xml.push_str(&release.torrent().size.to_string());
    xml.push_str("\" type=\"application/x-bittorrent\" />\n");
    torznab_attr(xml, "category", TORZNAB_CATEGORY);
    torznab_attr(xml, "infohash", &release.torrent().info_hash.to_string());
    torznab_attr(xml, "magneturl", &magnet);
    torznab_attr(xml, "size", &release.torrent().size.to_string());
    torznab_attr(xml, "details", &details);
    torznab_attr(xml, "publisher", &release.publisher().to_string());
    torznab_attr(xml, "releaseid", &release.id().to_string());
    torznab_attr(xml, "authority", AUTHORITY_SIGNAL);
    for tag in release.tags() {
        torznab_attr(xml, "tag", tag);
    }
    xml.push_str("</item>\n");
}

fn torznab_attr(xml: &mut String, name: &str, value: &str) {
    xml.push_str("<torznab:attr name=\"");
    attribute(xml, name);
    xml.push_str("\" value=\"");
    attribute(xml, value);
    xml.push_str("\" />\n");
}

pub(crate) fn open_search_description(base_url: &str) -> String {
    let search = format!("{base_url}/v1/search.rss?q={{searchTerms}}&limit={{count?}}");
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <OpenSearchDescription xmlns=\"http://a9.com/-/spec/opensearch/1.1/\">\n",
    );
    element(&mut xml, "ShortName", "Torky");
    element(
        &mut xml,
        "Description",
        "Non-authoritative search of opt-in cached Torky releases",
    );
    xml.push_str(
        "<InputEncoding>UTF-8</InputEncoding>\n<Url type=\"application/rss+xml\" template=\"",
    );
    attribute(&mut xml, &search);
    xml.push_str("\" />\n</OpenSearchDescription>\n");
    xml
}

fn element(xml: &mut String, name: &str, value: &str) {
    xml.push('<');
    xml.push_str(name);
    xml.push('>');
    text(xml, value);
    xml.push_str("</");
    xml.push_str(name);
    xml.push_str(">\n");
}

fn text(xml: &mut String, value: &str) {
    escape(xml, value, false);
}

fn attribute(xml: &mut String, value: &str) {
    escape(xml, value, true);
}

fn escape(output: &mut String, value: &str, quote: bool) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' if quote => output.push_str("&quot;"),
            '\'' if quote => output.push_str("&apos;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use pubky::Keypair;
    use swarm_protocol::{InfoHashV1, PublisherId, ReleaseFile, TorrentV1};

    use super::*;

    fn release() -> ReleaseV1 {
        ReleaseV1::new(
            PublisherId::new(Keypair::from_secret(&[7; 32]).public_key()),
            1_786_000_000_000,
            "A & B <Release>".to_owned(),
            "Quoted \"description\" & provenance".to_owned(),
            TorrentV1 {
                info_hash: InfoHashV1::from_bytes([0xab; 20]),
                size: 42,
                files: vec![ReleaseFile {
                    path: "payload.bin".to_owned(),
                    size: 42,
                }],
                trackers: vec!["https://tracker.example/announce?x=1&y=2".to_owned()],
            },
            vec!["open-data".to_owned()],
        )
        .unwrap()
    }

    #[test]
    fn escapes_xml_and_emits_bep36_enclosure() {
        let release = release();
        let xml = rss_feed(
            "Search: <all>",
            "A & B",
            "https://index.example/v1/search.rss?q=a&b",
            "https://index.example",
            std::slice::from_ref(&release),
        );
        assert!(xml.contains("<title>A &amp; B &lt;Release&gt;</title>"));
        assert!(xml.contains("type=\"application/x-bittorrent\""));
        assert!(xml.contains("length=\"42\""));
        assert!(xml.contains("url=\"magnet:?xt=urn%3Abtih%3A"));
        assert!(xml.contains(AUTHORITY_SIGNAL));
        assert!(!xml.contains("<title>A & B"));
    }

    #[test]
    fn magnet_has_name_and_all_trackers() {
        let magnet = magnet_url(&release());
        assert!(magnet.starts_with("magnet:?xt=urn%3Abtih%3A"));
        assert!(magnet.contains("&dn=A+%26+B+%3CRelease%3E"));
        assert!(magnet.contains("&tr=https%3A%2F%2Ftracker.example"));
    }

    #[test]
    fn query_and_filter_bounds_are_explicit() {
        assert!(validate_query(&"x".repeat(MAX_QUERY_CHARS)).is_ok());
        assert!(validate_query(&"x".repeat(MAX_QUERY_CHARS + 1)).is_err());
        assert!(matches(&release(), "release", Some("open-data")));
        assert!(!matches(&release(), "", Some("other")));
    }

    #[test]
    fn torznab_omits_unavailable_observations() {
        let xml = torznab_feed(
            "https://index.example/api?t=search",
            "https://index.example",
            0,
            1,
            &[release()],
        );
        assert!(xml.contains("name=\"infohash\""));
        assert!(xml.contains("name=\"magneturl\""));
        assert!(xml.contains("name=\"tag\" value=\"open-data\""));
        assert!(xml.contains("name=\"details\""));
        assert!(!xml.contains("name=\"seeders\""));
        assert!(!xml.contains("name=\"peers\""));
    }

    #[test]
    fn open_search_template_is_validly_escaped() {
        let xml = open_search_description("https://index.example");
        assert!(xml.contains(
            "template=\"https://index.example/v1/search.rss?q={searchTerms}&amp;limit={count?}\""
        ));
        assert!(!xml.contains("&amp;amp;"));
    }
}
