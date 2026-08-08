//! Optional non-authoritative Pubky Swarm discovery index.

#![forbid(unsafe_code)]

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE, HOST};
use axum::http::uri::Authority;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use pubky::PublicKey;
use pubky_adapter::{PubkyAdapter, RELEASES_PATH};
use serde::{Deserialize, Serialize};
use swarm_protocol::{PublisherId, ReleaseId, ReleaseV1};
use swarm_store::Store;
use tokio::net::TcpListener;

mod compatibility;

use compatibility::{
    AUTHORITY_SIGNAL, MAX_OFFSET, MAX_RESULTS, TORZNAB_CATEGORY, details_url, matches,
    open_search_description, rss_feed, torznab_caps, torznab_feed, validate_query,
};

const PYTHON_PLUGIN: &str = include_str!("../pubky_swarm.py");

#[derive(Debug)]
struct ServiceState {
    adapter: PubkyAdapter,
    store: Store,
    public_base_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
    authority: &'static str,
}

#[derive(Debug, Serialize)]
struct RefreshResult {
    publisher: String,
    indexed: usize,
}

#[derive(Debug, Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct LimitQuery {
    #[serde(default = "default_limit")]
    limit: usize,
}

#[derive(Debug, Serialize)]
struct SearchResult {
    release: ReleaseV1,
    source: &'static str,
    non_authoritative: bool,
    client_validation_required: bool,
    details: String,
    provenance: Provenance,
}

#[derive(Debug, Serialize)]
struct Provenance {
    publisher: String,
    pubky_path: String,
    pubky_uri: String,
    cache: &'static str,
    validation: &'static str,
    authority: &'static str,
}

#[derive(Debug, Deserialize)]
struct TorznabQuery {
    #[serde(default = "default_torznab_action")]
    t: String,
    #[serde(default)]
    q: String,
    cat: Option<String>,
    tag: Option<String>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default)]
    offset: usize,
}

#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: error.to_string(),
        }
    }

    fn upstream(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            message: error.to_string(),
        }
    }

    fn internal(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }

    fn not_found(error: impl std::fmt::Display) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bind: SocketAddr = std::env::var("PUBKY_SWARM_DISCOVERY_BIND")
        .unwrap_or_else(|_| "127.0.0.1:7780".to_owned())
        .parse()?;
    let database = PathBuf::from(
        std::env::var("PUBKY_SWARM_DISCOVERY_DB")
            .unwrap_or_else(|_| "data/discovery/swarm.sqlite3".to_owned()),
    );
    let state = Arc::new(ServiceState {
        adapter: PubkyAdapter::mainnet()?,
        store: Store::open(database)?,
        public_base_url: std::env::var("PUBKY_SWARM_DISCOVERY_PUBLIC_URL")
            .ok()
            .map(|value| validate_public_base_url(&value))
            .transpose()?,
    });
    let router = app(state);
    let listener = TcpListener::bind(bind).await?;
    println!("discovery-listening http://{}", listener.local_addr()?);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

fn app(state: Arc<ServiceState>) -> Router {
    Router::new()
        .route("/health", get(health))
        .route(
            "/v1/publishers/{publisher}/refresh",
            post(refresh_publisher),
        )
        .route("/v1/publishers/{publisher}/releases", get(cached_releases))
        .route(
            "/v1/publishers/{publisher}/releases/{release}",
            get(release_details),
        )
        .route(
            "/v1/publishers/{publisher}/releases.rss",
            get(publisher_rss),
        )
        .route("/v1/search", get(search))
        .route("/v1/search.rss", get(search_rss))
        .route("/api", get(torznab_api))
        .route("/torznab/api", get(torznab_api))
        .route("/v1/torznab/caps", get(torznab_caps_endpoint))
        .route("/v1/torznab/search", get(torznab_search_endpoint))
        .route("/opensearch.xml", get(open_search))
        .route("/plugins/pubky_swarm.py", get(python_plugin))
        .with_state(state)
}

async fn health() -> Json<Health> {
    Json(Health {
        status: "ok",
        authority: "none-client-validation-required",
    })
}

async fn refresh_publisher(
    State(state): State<Arc<ServiceState>>,
    Path(publisher): Path<String>,
) -> Result<Json<RefreshResult>, ApiError> {
    let key = PublicKey::try_from(publisher.as_str()).map_err(ApiError::bad_request)?;
    let identity = PublisherId::new(key.clone());
    let resources = state
        .adapter
        .list_public(&key, RELEASES_PATH, None, 1_000)
        .await
        .map_err(ApiError::upstream)?;
    let mut indexed = 0;
    for resource in resources {
        let release: ReleaseV1 = state
            .adapter
            .get_public_json(&key, resource.path.as_str())
            .await
            .map_err(ApiError::upstream)?;
        if release.publisher() != &identity {
            return Err(ApiError::upstream(
                "release publisher does not match refreshed Pubky",
            ));
        }
        state
            .store
            .cache_release(&release)
            .map_err(ApiError::internal)?;
        indexed += 1;
    }
    state.store.follow(&identity).map_err(ApiError::internal)?;
    Ok(Json(RefreshResult {
        publisher: identity.to_string(),
        indexed,
    }))
}

async fn cached_releases(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(publisher): Path<String>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let identity: PublisherId = publisher.parse().map_err(ApiError::bad_request)?;
    let base_url = request_base_url(&headers, &state)?;
    let releases = state
        .store
        .releases_for(&identity)
        .map_err(ApiError::internal)?;
    Ok(Json(
        releases
            .into_iter()
            .map(|release| result(release, &base_url))
            .collect(),
    ))
}

async fn release_details(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path((publisher, release)): Path<(String, String)>,
) -> Result<Json<SearchResult>, ApiError> {
    let publisher: PublisherId = publisher.parse().map_err(ApiError::bad_request)?;
    let release: ReleaseId = release.parse().map_err(ApiError::bad_request)?;
    let base_url = request_base_url(&headers, &state)?;
    let found = state
        .store
        .releases_for(&publisher)
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|candidate| candidate.id() == release)
        .ok_or_else(|| ApiError::not_found("cached release not found"))?;
    Ok(Json(result(found, &base_url)))
}

async fn search(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<SearchResult>>, ApiError> {
    let needle = validate_query(&query.q).map_err(ApiError::bad_request)?;
    let base_url = request_base_url(&headers, &state)?;
    let releases = find_releases(&state, needle, None, 0, query.limit)?;
    Ok(Json(
        releases
            .into_iter()
            .map(|release| result(release, &base_url))
            .collect(),
    ))
}

async fn publisher_rss(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Path(publisher): Path<String>,
    Query(query): Query<LimitQuery>,
) -> Result<Response, ApiError> {
    let publisher: PublisherId = publisher.parse().map_err(ApiError::bad_request)?;
    let base_url = request_base_url(&headers, &state)?;
    let limit = query.limit.clamp(1, MAX_RESULTS);
    let self_url = format!("{base_url}/v1/publishers/{publisher}/releases.rss?limit={limit}");
    let mut releases = state
        .store
        .releases_for(&publisher)
        .map_err(ApiError::internal)?;
    releases.truncate(limit);
    Ok(rss_response(rss_feed(
        &format!("Pubky Swarm publisher {publisher}"),
        &format!("Opt-in cached releases for Pubky publisher {publisher}"),
        &self_url,
        &base_url,
        &releases,
    )))
}

async fn search_rss(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Query(query): Query<SearchQuery>,
) -> Result<Response, ApiError> {
    let needle = validate_query(&query.q).map_err(ApiError::bad_request)?;
    let limit = query.limit.clamp(1, MAX_RESULTS);
    let base_url = request_base_url(&headers, &state)?;
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("q", needle);
    serializer.append_pair("limit", &limit.to_string());
    let self_url = format!("{base_url}/v1/search.rss?{}", serializer.finish());
    let releases = find_releases(&state, needle, None, 0, limit)?;
    Ok(rss_response(rss_feed(
        &format!("Pubky Swarm search: {needle}"),
        "Lossy RSS view of the opt-in validated release cache",
        &self_url,
        &base_url,
        &releases,
    )))
}

async fn torznab_api(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Query(query): Query<TorznabQuery>,
) -> Result<Response, ApiError> {
    match query.t.as_str() {
        "caps" => torznab_caps_response(&headers, &state),
        "search" => torznab_search_response(&headers, &state, &query),
        _ => Err(ApiError::bad_request(
            "unsupported Torznab action; expected caps or search",
        )),
    }
}

async fn torznab_caps_endpoint(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    torznab_caps_response(&headers, &state)
}

async fn torznab_search_endpoint(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
    Query(query): Query<TorznabQuery>,
) -> Result<Response, ApiError> {
    torznab_search_response(&headers, &state, &query)
}

fn torznab_caps_response(headers: &HeaderMap, state: &ServiceState) -> Result<Response, ApiError> {
    let base_url = request_base_url(headers, state)?;
    Ok(xml_response(torznab_caps(&base_url)))
}

fn torznab_search_response(
    headers: &HeaderMap,
    state: &ServiceState,
    query: &TorznabQuery,
) -> Result<Response, ApiError> {
    let needle = validate_query(&query.q).map_err(ApiError::bad_request)?;
    let limit = query.limit.clamp(1, MAX_RESULTS);
    let offset = query.offset.min(MAX_OFFSET);
    let tag = torznab_tag(query)?;
    let base_url = request_base_url(headers, state)?;
    let matching = matching_releases(state, needle, tag)?;
    let total = matching.len();
    let releases: Vec<_> = matching.into_iter().skip(offset).take(limit).collect();
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("t", "search");
    serializer.append_pair("q", needle);
    serializer.append_pair("limit", &limit.to_string());
    serializer.append_pair("offset", &offset.to_string());
    if let Some(tag) = tag {
        serializer.append_pair("tag", tag);
    }
    let self_url = format!("{base_url}/api?{}", serializer.finish());
    Ok(rss_response(torznab_feed(
        &self_url, &base_url, offset, total, &releases,
    )))
}

fn torznab_tag(query: &TorznabQuery) -> Result<Option<&str>, ApiError> {
    if let Some(tag) = query.tag.as_deref() {
        if tag.is_empty()
            || tag.len() > 32
            || !tag
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(ApiError::bad_request("invalid Torznab tag filter"));
        }
        return Ok(Some(tag));
    }
    if let Some(categories) = query.cat.as_deref() {
        let accepted = categories
            .split(',')
            .all(|category| matches!(category.trim(), "" | "0" | TORZNAB_CATEGORY));
        if !accepted {
            return Err(ApiError::bad_request(
                "unsupported Torznab category; use 8000",
            ));
        }
    }
    Ok(None)
}

async fn open_search(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let base_url = request_base_url(&headers, &state)?;
    Ok(xml_response(open_search_description(&base_url)))
}

async fn python_plugin(
    State(state): State<Arc<ServiceState>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let base_url = request_base_url(&headers, &state)?;
    let plugin = PYTHON_PLUGIN.replace("http://127.0.0.1:7780", &base_url);
    Ok((
        [
            (CONTENT_TYPE, "text/x-python; charset=utf-8"),
            (
                CONTENT_DISPOSITION,
                "attachment; filename=\"pubky_swarm.py\"",
            ),
        ],
        plugin,
    )
        .into_response())
}

fn find_releases(
    state: &ServiceState,
    needle: &str,
    tag: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<Vec<ReleaseV1>, ApiError> {
    Ok(matching_releases(state, needle, tag)?
        .into_iter()
        .skip(offset.min(MAX_OFFSET))
        .take(limit.clamp(1, MAX_RESULTS))
        .collect())
}

fn matching_releases(
    state: &ServiceState,
    needle: &str,
    tag: Option<&str>,
) -> Result<Vec<ReleaseV1>, ApiError> {
    Ok(state
        .store
        .all_releases(MAX_OFFSET)
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|release| matches(release, needle, tag))
        .collect())
}

fn result(release: ReleaseV1, base_url: &str) -> SearchResult {
    let details = details_url(base_url, &release);
    let publisher = release.publisher().to_string();
    let pubky_path = release.storage_path();
    let provenance = Provenance {
        pubky_uri: format!("pubky://{publisher}{pubky_path}"),
        publisher,
        pubky_path,
        cache: "swarm-store release cache",
        validation: "ReleaseV1 schema validation and publisher identity match on refresh",
        authority: AUTHORITY_SIGNAL,
    };
    SearchResult {
        release,
        source: "validated-local-cache",
        non_authoritative: true,
        client_validation_required: true,
        details,
        provenance,
    }
}

const fn default_limit() -> usize {
    25
}

fn default_torznab_action() -> String {
    "search".to_owned()
}

fn xml_response(body: String) -> Response {
    ([(CONTENT_TYPE, "application/xml; charset=utf-8")], body).into_response()
}

fn rss_response(body: String) -> Response {
    ([(CONTENT_TYPE, "application/rss+xml; charset=utf-8")], body).into_response()
}

fn request_base_url(headers: &HeaderMap, state: &ServiceState) -> Result<String, ApiError> {
    if let Some(base_url) = &state.public_base_url {
        return Ok(base_url.clone());
    }
    let host = headers
        .get(HOST)
        .ok_or_else(|| ApiError::bad_request("Host header is required"))?
        .to_str()
        .map_err(ApiError::bad_request)?;
    if host.contains(['@', '/', '\\']) {
        return Err(ApiError::bad_request("invalid Host header"));
    }
    Authority::from_str(host).map_err(ApiError::bad_request)?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    if !matches!(scheme, "http" | "https") {
        return Err(ApiError::bad_request("invalid forwarded protocol"));
    }
    Ok(format!("{scheme}://{host}"))
}

fn validate_public_base_url(value: &str) -> Result<String, &'static str> {
    let mut url = url::Url::parse(value).map_err(|_| "invalid discovery public URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "discovery public URL must be credential-free HTTP(S) without query or fragment",
        );
    }
    let path = url.path().trim_end_matches('/').to_owned();
    url.set_path(&path);
    Ok(url.to_string().trim_end_matches('/').to_owned())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::Request;
    use pubky::Keypair;
    use swarm_protocol::{InfoHashV1, ReleaseFile, TorrentV1};
    use tower::ServiceExt;

    use super::*;

    fn sample_release() -> ReleaseV1 {
        ReleaseV1::new(
            PublisherId::new(Keypair::from_secret(&[3; 32]).public_key()),
            1_786_000_000_000,
            "Open & Indexed Release".to_owned(),
            "Full native provenance".to_owned(),
            TorrentV1 {
                info_hash: InfoHashV1::from_bytes([0xcd; 20]),
                size: 12,
                files: vec![ReleaseFile {
                    path: "release.bin".to_owned(),
                    size: 12,
                }],
                trackers: Vec::new(),
            },
            vec!["open-data".to_owned()],
        )
        .unwrap()
    }

    fn test_app() -> (Router, ReleaseV1) {
        let release = sample_release();
        let store = Store::in_memory().unwrap();
        store.cache_release(&release).unwrap();
        let state = Arc::new(ServiceState {
            adapter: PubkyAdapter::mainnet().unwrap(),
            store,
            public_base_url: None,
        });
        (app(state), release)
    }

    async fn get(router: Router, path: &str) -> (StatusCode, HeaderMap, String) {
        let response = router
            .oneshot(
                Request::builder()
                    .uri(path)
                    .header(HOST, "index.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, headers, String::from_utf8(body.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn native_search_and_details_preserve_provenance() {
        let (router, release) = test_app();
        let (status, _, body) = get(router.clone(), "/v1/search?q=indexed&limit=500").await;
        assert_eq!(status, StatusCode::OK);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value.as_array().unwrap().len(), 1);
        assert_eq!(value[0]["non_authoritative"], true);
        assert_eq!(value[0]["client_validation_required"], true);
        assert_eq!(value[0]["provenance"]["pubky_path"], release.storage_path());
        let details_path = format!(
            "/v1/publishers/{}/releases/{}",
            release.publisher(),
            release.id()
        );
        let (status, _, details) = get(router, &details_path).await;
        assert_eq!(status, StatusCode::OK);
        assert!(details.contains("\"source\":\"validated-local-cache\""));
    }

    #[tokio::test]
    async fn rss_torznab_caps_and_plugin_handlers_are_compatible() {
        let (router, release) = test_app();
        let publisher_feed = format!("/v1/publishers/{}/releases.rss", release.publisher());
        let (status, headers, rss) = get(router.clone(), &publisher_feed).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(CONTENT_TYPE).unwrap(),
            "application/rss+xml; charset=utf-8"
        );
        assert!(rss.contains("<enclosure url=\"magnet:"));
        assert!(rss.contains("http://index.example/v1/publishers/"));
        assert!(rss.contains(AUTHORITY_SIGNAL));

        let (status, _, caps) = get(router.clone(), "/api?t=caps").await;
        assert_eq!(status, StatusCode::OK);
        assert!(caps.contains("<category id=\"8000\""));
        assert!(caps.contains("observations=\"unavailable\""));

        let (status, _, search) = get(
            router.clone(),
            "/api?t=search&q=indexed&cat=8000&tag=open-data",
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(search.contains("<torznab:attr name=\"infohash\""));
        assert!(!search.contains("name=\"seeders\""));

        let (status, headers, plugin) = get(router, "/plugins/pubky_swarm.py").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            headers.get(CONTENT_TYPE).unwrap(),
            "text/x-python; charset=utf-8"
        );
        assert!(plugin.contains("http://index.example"));
        assert!(plugin.contains("/v1/search"));
    }

    #[tokio::test]
    async fn rejects_invalid_publishers_queries_and_categories() {
        let (router, _) = test_app();
        let (status, _, _) = get(router.clone(), "/v1/publishers/not-a-key/releases.rss").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let query = "x".repeat(compatibility::MAX_QUERY_CHARS + 1);
        let (status, _, _) = get(router.clone(), &format!("/v1/search?q={query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (status, _, _) = get(router, "/api?t=search&cat=5000").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn public_url_rejects_credentials_and_non_http_schemes() {
        assert_eq!(
            validate_public_base_url("https://index.example/").unwrap(),
            "https://index.example"
        );
        assert!(validate_public_base_url("https://user@index.example").is_err());
        assert!(validate_public_base_url("file:///tmp/index").is_err());
    }
}
