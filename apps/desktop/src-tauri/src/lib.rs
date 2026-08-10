//! Native application boundary for Torky.

#![forbid(unsafe_code)]

use std::collections::{HashMap, HashSet};
use std::net::{Ipv4Addr, TcpListener};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use catalog_client::{
    CatalogItem, MAX_RESPONSE_BYTES, SourceKind, parse_catalog, torznab_search_url,
    validate_source_url,
};
use futures::stream::{self, StreamExt};
use mainline::Dht;
use mainline_discovery::PeerDiscovery;
use pubky::{AuthFlowKind, Capabilities, ClientId, PubkyGrantAuthFlow, PubkySession, PublicKey};
use pubky_adapter::{PROFILE_PATH, PubkyAdapter, RELEASES_PATH, TAG_CLAIMS_PATH};
use qbittorrent_connector::{QbittorrentClient, TorrentInfo as QbittorrentTorrentInfo};
use serde::{Deserialize, Serialize};
use stream_gateway::StreamGateway;
use swarm_protocol::{
    InfoHashV1, PublisherId, ReleaseFile, ReleaseV1, SourceAttribution, SubjectRef, TagClaimV1,
    TagOperation, TorrentRef, TorrentV1,
};
use swarm_store::{BUILT_IN_RSS_PRESETS, CatalogSource, ClientSettings, Store};
use tauri::{Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock};
use torrent_engine::{
    AddOptions, CreateOptions, DhtMode, EngineConfig, TorrentEngine, magnet_v1_info_hash,
};
use url::Url;

const MAINLINE_IMPORT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const EXTERNAL_CATALOG_CONCURRENCY: usize = 4;
const MAGNET_OPENED_EVENT: &str = "torky-magnet-opened";

struct AppState {
    adapter: PubkyAdapter,
    engine: Arc<TorrentEngine>,
    discovery: PeerDiscovery,
    store: Store,
    gateway: StreamGateway,
    auth_flow: Mutex<Option<PubkyGrantAuthFlow>>,
    session: RwLock<Option<PubkySession>>,
    qbittorrent: RwLock<Option<Arc<QbittorrentClient>>>,
    catalog_api_keys: RwLock<HashMap<i64, String>>,
    tag_publish_lock: Mutex<()>,
    pending_magnet: std::sync::Mutex<Option<String>>,
    preferred_download_dir: RwLock<Option<PathBuf>>,
    /// Network settings the running engine was started with (for restart detection).
    session_network: SessionNetwork,
    catalog_url: Option<Url>,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionNetwork {
    dht_enabled: bool,
    upnp_enabled: bool,
    listen_port: Option<u16>,
}

#[derive(Debug, Serialize)]
struct AuthStart {
    authorization_url: String,
}

#[derive(Debug, Serialize)]
struct AuthStatus {
    authenticated: bool,
    user: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
struct ProfileResponse {
    name: String,
    bio: Option<String>,
    image: Option<String>,
    status: Option<String>,
}

impl ProfileResponse {
    fn validate(self) -> Result<Self, String> {
        let name_length = self.name.chars().count();
        if !(3..=50).contains(&name_length)
            || self.name == "[DELETED]"
            || self.name.chars().any(char::is_control)
        {
            return Err("publisher profile has an invalid name".to_owned());
        }
        if self
            .bio
            .as_ref()
            .is_some_and(|value| value.chars().count() > 160)
            || self
                .status
                .as_ref()
                .is_some_and(|value| value.chars().count() > 50)
            || self
                .image
                .as_ref()
                .is_some_and(|value| value.chars().count() > 300)
        {
            return Err("publisher profile exceeds supported field bounds".to_owned());
        }
        Ok(self)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateReleaseRequest {
    source_path: String,
    title: String,
    description: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportMagnetRequest {
    magnet: String,
    save_path: Option<String>,
    only_files: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportTorrentFileRequest {
    torrent_path: String,
    save_path: Option<String>,
    only_files: Option<Vec<usize>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QbittorrentConnectRequest {
    base_url: String,
    username: String,
    password: String,
    #[serde(default)]
    allow_remote: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct QbittorrentStatus {
    connected: bool,
    version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CatalogSearchResult {
    release: ReleaseV1,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCatalogSourceRequest {
    name: String,
    kind: SourceKind,
    endpoint: String,
    requires_api_key: bool,
    api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddRssFeedRequest {
    feed_url: String,
    name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RssPresetInfo {
    name: String,
    endpoint: String,
    enabled_by_default: bool,
    description: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSourceStatus {
    #[serde(flatten)]
    source: CatalogSource,
    has_api_key: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalCatalogSearchResponse {
    results: Vec<CatalogItem>,
    errors: Vec<CatalogSourceFailure>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogSourceFailure {
    source_id: i64,
    source_name: String,
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublishCatalogTagsRequest {
    info_hash: String,
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineStatus {
    download_dir: String,
    listen_port: Option<u16>,
    dht_enabled: bool,
    upnp_enabled: bool,
    download_limit_kbps: Option<u32>,
    upload_limit_kbps: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSettingsResponse {
    settings: ClientSettings,
    status: EngineStatus,
    restart_required: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SubjectTagClaim {
    issuer: String,
    tag: String,
    subject: String,
    info_hash: Option<String>,
    created_at: u64,
    revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SyncFollowedResponse {
    followed: Vec<String>,
    releases: Vec<ReleaseV1>,
    claim_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TorrentSummary {
    id: usize,
    info_hash: String,
    name: Option<String>,
    state: String,
    progress_bytes: u64,
    total_bytes: u64,
    uploaded_bytes: u64,
    download_mbps: f64,
    upload_mbps: f64,
    peers_connected: usize,
    peers_seen: usize,
    ratio: f64,
    eta: Option<u64>,
    finished: bool,
    error: Option<String>,
    files: Vec<TorrentFileSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TorrentFileSummary {
    index: usize,
    path: String,
    length: u64,
    included: bool,
}

#[tauri::command]
async fn start_auth(state: State<'_, AppState>) -> Result<AuthStart, String> {
    let capabilities = Capabilities::builder()
        .read_write(RELEASES_PATH)
        .map_err(display_error)?
        .read_write(TAG_CLAIMS_PATH)
        .map_err(display_error)?
        .finish();
    let client_id = ClientId::new("pubky.swarm").map_err(display_error)?;
    let flow = state
        .adapter
        .sdk()
        .start_grant_auth_flow(&capabilities, AuthFlowKind::signin(), client_id)
        .map_err(display_error)?;
    let authorization_url = flow.authorization_url().to_string();
    *state.auth_flow.lock().await = Some(flow);
    Ok(AuthStart { authorization_url })
}

#[tauri::command]
async fn poll_auth(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    let guard = state.auth_flow.lock().await;
    let Some(flow) = guard.as_ref() else {
        return auth_status(&state).await;
    };
    let Some(session) = flow.try_poll_once().await.map_err(display_error)? else {
        return Ok(AuthStatus {
            authenticated: false,
            user: None,
        });
    };
    let user = session.info().public_key().to_string();
    *state.session.write().await = Some(session);
    drop(guard);
    *state.auth_flow.lock().await = None;
    Ok(AuthStatus {
        authenticated: true,
        user: Some(user),
    })
}

#[tauri::command]
async fn get_auth_status(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    auth_status(&state).await
}

#[tauri::command]
async fn sign_out(state: State<'_, AppState>) -> Result<AuthStatus, String> {
    *state.auth_flow.lock().await = None;
    *state.session.write().await = None;
    auth_status(&state).await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command injection requires owned State.
fn take_pending_magnet(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state
        .pending_magnet
        .lock()
        .map_err(|_| "pending magnet lock poisoned".to_owned())
        .map(|mut pending| pending.take())
}

#[tauri::command]
async fn get_settings(state: State<'_, AppState>) -> Result<ClientSettings, String> {
    state.store.client_settings().map_err(display_error)
}

#[tauri::command]
async fn get_engine_status(state: State<'_, AppState>) -> Result<EngineStatus, String> {
    Ok(engine_status_from(&state))
}

#[tauri::command]
async fn update_settings(
    settings: ClientSettings,
    state: State<'_, AppState>,
) -> Result<UpdateSettingsResponse, String> {
    let settings = settings.normalized();
    settings.validate().map_err(display_error)?;
    let preferred = resolve_preferred_download_dir(settings.download_dir.as_deref())?;
    if let Some(path) = &preferred {
        std::fs::create_dir_all(path).map_err(display_error)?;
    }

    let running = state.session_network;
    let restart_required = settings.dht_enabled != running.dht_enabled
        || settings.upnp_enabled != running.upnp_enabled
        || settings.listen_port != running.listen_port;

    state
        .store
        .set_client_settings(&settings)
        .map_err(display_error)?;
    *state.preferred_download_dir.write().await = preferred;

    state
        .engine
        .set_download_bps(kbps_to_bps(settings.download_limit_kbps));
    state
        .engine
        .set_upload_bps(kbps_to_bps(settings.upload_limit_kbps));

    Ok(UpdateSettingsResponse {
        settings,
        status: engine_status_from(&state),
        restart_required,
    })
}

#[tauri::command]
async fn get_profile(user: String, state: State<'_, AppState>) -> Result<ProfileResponse, String> {
    let user = PublicKey::try_from(user.as_str()).map_err(display_error)?;
    let profile: ProfileResponse = state
        .adapter
        .get_public_json(&user, PROFILE_PATH)
        .await
        .map_err(display_error)?;
    profile.validate()
}

#[tauri::command]
async fn list_releases(user: String, state: State<'_, AppState>) -> Result<Vec<ReleaseV1>, String> {
    let user = PublicKey::try_from(user.as_str()).map_err(display_error)?;
    let publisher = PublisherId::new(user.clone());
    let Ok(resources) = state
        .adapter
        .list_public(&user, RELEASES_PATH, None, 1_000)
        .await
    else {
        return state.store.releases_for(&publisher).map_err(display_error);
    };
    let mut releases = Vec::with_capacity(resources.len());
    for resource in resources {
        let release: ReleaseV1 = state
            .adapter
            .get_public_json(&user, resource.path.as_str())
            .await
            .map_err(display_error)?;
        state.store.cache_release(&release).map_err(display_error)?;
        releases.push(release);
    }
    releases.sort_by_key(ReleaseV1::created_at);
    releases.reverse();
    Ok(releases)
}

#[tauri::command]
async fn search_catalog(
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<Vec<ReleaseV1>, String> {
    let query = query.trim();
    if query.chars().count() > 256 {
        return Err("catalog query exceeds 256 characters".to_owned());
    }
    let Some(catalog_url) = &state.catalog_url else {
        return Ok(Vec::new());
    };
    let endpoint = catalog_url.join("v1/search").map_err(display_error)?;
    let response = state
        .http
        .get(endpoint)
        .query(&[
            ("q", query.to_owned()),
            ("limit", limit.unwrap_or(25).clamp(1, 100).to_string()),
        ])
        .send()
        .await
        .map_err(display_error)?;
    if !response.status().is_success() {
        return Err(format!("catalog service returned {}", response.status()));
    }
    let results: Vec<CatalogSearchResult> = response.json().await.map_err(display_error)?;
    Ok(results.into_iter().map(|result| result.release).collect())
}

#[tauri::command]
async fn list_external_catalog_sources(
    state: State<'_, AppState>,
) -> Result<Vec<CatalogSourceStatus>, String> {
    let keys = state.catalog_api_keys.read().await;
    state
        .store
        .catalog_sources()
        .map_err(display_error)?
        .into_iter()
        .map(|source| {
            let has_api_key = keys.contains_key(&source.id);
            Ok(CatalogSourceStatus {
                source,
                has_api_key,
            })
        })
        .collect()
}

#[tauri::command]
fn list_rss_presets() -> Vec<RssPresetInfo> {
    BUILT_IN_RSS_PRESETS
        .iter()
        .map(|preset| RssPresetInfo {
            name: preset.name.to_owned(),
            endpoint: preset.endpoint.to_owned(),
            enabled_by_default: preset.enabled_by_default,
            description: preset.description.to_owned(),
        })
        .collect()
}

#[tauri::command]
async fn add_rss_feed(
    request: AddRssFeedRequest,
    state: State<'_, AppState>,
) -> Result<CatalogSourceStatus, String> {
    let source = state
        .store
        .add_rss_feed(
            &request.feed_url,
            request.name.as_deref(),
        )
        .map_err(display_error)?;
    Ok(CatalogSourceStatus {
        has_api_key: false,
        source,
    })
}

#[tauri::command]
async fn add_external_catalog_source(
    request: AddCatalogSourceRequest,
    state: State<'_, AppState>,
) -> Result<CatalogSourceStatus, String> {
    let api_key = normalize_api_key(request.api_key)?;
    if request.kind == SourceKind::Rss && api_key.is_some() {
        return Err("RSS sources do not accept API keys".to_owned());
    }
    let requires_api_key = request.requires_api_key || api_key.is_some();
    let source = state
        .store
        .add_catalog_source(
            &request.name,
            request.kind,
            &request.endpoint,
            requires_api_key,
        )
        .map_err(display_error)?;
    if let Some(api_key) = api_key {
        state
            .catalog_api_keys
            .write()
            .await
            .insert(source.id, api_key);
    }
    Ok(CatalogSourceStatus {
        has_api_key: state.catalog_api_keys.read().await.contains_key(&source.id),
        source,
    })
}

#[tauri::command]
async fn set_external_catalog_source_enabled(
    source_id: i64,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if state
        .store
        .set_catalog_source_enabled(source_id, enabled)
        .map_err(display_error)?
    {
        Ok(())
    } else {
        Err(format!("unknown catalog source {source_id}"))
    }
}

#[tauri::command]
async fn set_external_catalog_api_key(
    source_id: i64,
    api_key: Option<String>,
    state: State<'_, AppState>,
) -> Result<CatalogSourceStatus, String> {
    let source = state
        .store
        .catalog_source(source_id)
        .map_err(display_error)?
        .ok_or_else(|| format!("unknown catalog source {source_id}"))?;
    if source.kind != SourceKind::Torznab {
        return Err("only Torznab sources accept API keys".to_owned());
    }
    let api_key = normalize_api_key(api_key)?;
    let mut keys = state.catalog_api_keys.write().await;
    if let Some(api_key) = api_key {
        keys.insert(source_id, api_key);
    } else {
        keys.remove(&source_id);
    }
    Ok(CatalogSourceStatus {
        source,
        has_api_key: keys.contains_key(&source_id),
    })
}

#[tauri::command]
async fn remove_external_catalog_source(
    source_id: i64,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if !state
        .store
        .remove_catalog_source(source_id)
        .map_err(display_error)?
    {
        return Err(format!("unknown catalog source {source_id}"));
    }
    state.catalog_api_keys.write().await.remove(&source_id);
    Ok(())
}

#[tauri::command]
async fn search_external_catalogs(
    query: String,
    limit: Option<usize>,
    state: State<'_, AppState>,
) -> Result<ExternalCatalogSearchResponse, String> {
    let query = query.trim().to_owned();
    if query.chars().count() > 256 {
        return Err("catalog query exceeds 256 characters".to_owned());
    }
    let limit = limit.unwrap_or(50).clamp(1, catalog_client::MAX_RESULTS);
    let sources = state
        .store
        .catalog_sources()
        .map_err(display_error)?
        .into_iter()
        .filter(|source| source.enabled)
        .collect::<Vec<_>>();
    let keys = state.catalog_api_keys.read().await.clone();
    let responses = stream::iter(sources)
        .map(|source| {
            let query = query.clone();
            let api_key = keys.get(&source.id).cloned();
            let client = state.http.clone();
            async move {
                let result =
                    fetch_external_catalog(&client, &source, &query, limit, api_key.as_deref())
                        .await;
                (source, result)
            }
        })
        .buffer_unordered(EXTERNAL_CATALOG_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    let mut results = Vec::new();
    let mut errors = Vec::new();
    for (source, response) in responses {
        match response {
            Ok(items) => results.extend(items),
            Err(message) => errors.push(CatalogSourceFailure {
                source_id: source.id,
                source_name: source.name,
                message,
            }),
        }
    }
    results.sort_by(|left, right| {
        left.source_id
            .cmp(&right.source_id)
            .then_with(|| left.title.cmp(&right.title))
    });
    let mut seen = HashSet::new();
    results.retain(|item| {
        seen.insert(
            item.info_hash
                .clone()
                .unwrap_or_else(|| item.magnet.clone()),
        )
    });
    results.truncate(limit);
    errors.sort_by_key(|error| error.source_id);
    Ok(ExternalCatalogSearchResponse { results, errors })
}

#[tauri::command]
async fn publish_catalog_tags(
    request: PublishCatalogTagsRequest,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let _guard = state.tag_publish_lock.lock().await;
    let session = current_session(&state).await?;
    let issuer = PublisherId::new(session.info().public_key().clone());
    synchronize_tag_claims(&state, &issuer).await?;
    let info_hash: InfoHashV1 = request.info_hash.parse().map_err(display_error)?;
    let subject = SubjectRef::Torrent(TorrentRef::btih(info_hash));
    let tags = normalize_catalog_tags(request.tags)?;
    let mut revision = state
        .store
        .highest_tag_claim_revision(&issuer)
        .map_err(display_error)?
        .unwrap_or(0);
    let created_at = unix_millis()?;
    let mut claims = Vec::with_capacity(tags.len());
    for tag in &tags {
        revision = revision
            .checked_add(1)
            .ok_or_else(|| "tag-claim revision exhausted".to_owned())?;
        claims.push(
            TagClaimV1::new(
                issuer.clone(),
                subject.clone(),
                tag.clone(),
                TagOperation::Add,
                created_at,
                revision,
                SourceAttribution::Direct,
            )
            .map_err(display_error)?,
        );
    }
    for claim in claims {
        let path = format!("{TAG_CLAIMS_PATH}{}.json", claim.id());
        state
            .adapter
            .put_json(&session, &path, &claim)
            .await
            .map_err(display_error)?;
        state.store.cache_tag_claim(&claim).map_err(display_error)?;
    }
    Ok(tags)
}

#[tauri::command]
async fn create_release(
    request: CreateReleaseRequest,
    state: State<'_, AppState>,
) -> Result<ReleaseV1, String> {
    let session = current_session(&state).await?;
    let source = canonical_source(&request.source_path)?;
    let created = torrent_engine::create_torrent(
        &source,
        CreateOptions {
            name: None,
            piece_length: None,
        },
    )
    .await
    .map_err(display_error)?;
    let output_dir = if source.is_dir() {
        source.clone()
    } else {
        source
            .parent()
            .ok_or_else(|| "source file has no parent directory".to_owned())?
            .to_path_buf()
    };
    let torrent = state
        .engine
        .add_metainfo(
            created.metainfo_bytes(),
            AddOptions {
                output_dir: Some(output_dir),
                overwrite: true,
                disable_trackers: true,
                ..AddOptions::default()
            },
        )
        .await
        .map_err(display_error)?;
    torrent
        .wait_until_completed()
        .await
        .map_err(display_error)?;

    let listen_port = state
        .engine
        .listen_port()
        .filter(|port| *port != 0)
        .ok_or_else(|| "torrent listener has no publishable port".to_owned())?;
    let info_hash: InfoHashV1 = created.info_hash_hex().parse().map_err(display_error)?;
    tokio::time::timeout(
        Duration::from_secs(30),
        state.discovery.announce(info_hash, listen_port),
    )
    .await
    .map_err(|_| "Mainline peer announcement timed out".to_owned())?
    .map_err(display_error)?;

    let metadata = torrent.metadata().map_err(display_error)?;
    let mut tags = request.tags;
    tags.sort_unstable();
    tags.dedup();
    let release = ReleaseV1::new(
        PublisherId::new(session.info().public_key().clone()),
        unix_millis()?,
        request.title,
        request.description,
        TorrentV1 {
            info_hash,
            size: metadata.total_length,
            files: metadata
                .files
                .into_iter()
                .map(|file| {
                    let path = file
                        .path
                        .to_str()
                        .ok_or_else(|| "torrent contains a non-UTF-8 path".to_owned())?;
                    Ok(ReleaseFile {
                        path: path.replace('\\', "/"),
                        size: file.length,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?,
            trackers: Vec::new(),
        },
        tags,
    )
    .map_err(display_error)?;
    state
        .adapter
        .put_json(&session, &release.storage_path(), &release)
        .await
        .map_err(display_error)?;
    state.store.cache_release(&release).map_err(display_error)?;
    Ok(release)
}

#[tauri::command]
async fn follow_publisher(user: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let publisher = PublisherId::new(PublicKey::try_from(user.as_str()).map_err(display_error)?);
    state.store.follow(&publisher).map_err(display_error)?;
    followed(&state)
}

#[tauri::command]
async fn unfollow_publisher(user: String, state: State<'_, AppState>) -> Result<Vec<String>, String> {
    let publisher = PublisherId::new(PublicKey::try_from(user.as_str()).map_err(display_error)?);
    state.store.unfollow(&publisher).map_err(display_error)?;
    followed(&state)
}

#[tauri::command]
async fn list_followed(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    followed(&state)
}

#[tauri::command]
async fn sync_followed(state: State<'_, AppState>) -> Result<SyncFollowedResponse, String> {
    let mut publishers = state
        .store
        .followed_publishers()
        .map_err(display_error)?;
    if let Some(session) = state.session.read().await.as_ref() {
        let self_id = PublisherId::new(session.info().public_key().clone());
        if !publishers.iter().any(|publisher| publisher == &self_id) {
            publishers.push(self_id);
        }
    }
    for publisher in &publishers {
        let _ = sync_publisher_releases(&state, publisher).await?;
        synchronize_tag_claims(&state, publisher).await?;
    }
    let claim_count = state
        .store
        .recent_tag_claims(1_000)
        .map_err(display_error)?
        .len();
    let releases = state.store.all_releases(100).map_err(display_error)?;
    Ok(SyncFollowedResponse {
        followed: followed(&state)?,
        releases,
        claim_count,
    })
}

#[tauri::command]
async fn list_subject_tags(
    info_hash: String,
    state: State<'_, AppState>,
) -> Result<Vec<SubjectTagClaim>, String> {
    let info_hash: InfoHashV1 = info_hash.parse().map_err(display_error)?;
    let subject = SubjectRef::Torrent(TorrentRef::btih(info_hash));
    let claims = state
        .store
        .tag_claims_for(&subject)
        .map_err(display_error)?;
    Ok(claims
        .into_iter()
        .filter(|claim| matches!(claim.operation(), TagOperation::Add))
        .map(subject_tag_claim)
        .collect())
}

#[tauri::command]
async fn search_cached_tag_claims(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<SubjectTagClaim>, String> {
    let query = query.trim().to_lowercase();
    if query.chars().count() > 256 {
        return Err("tag query exceeds 256 characters".to_owned());
    }
    let claims = state
        .store
        .recent_tag_claims(500)
        .map_err(display_error)?;
    Ok(claims
        .into_iter()
        .filter(|claim| matches!(claim.operation(), TagOperation::Add))
        .filter(|claim| {
            query.is_empty()
                || claim.tag().contains(&query)
                || claim.issuer().to_string().to_lowercase().contains(&query)
                || claim.subject().to_string().to_lowercase().contains(&query)
        })
        .take(100)
        .map(subject_tag_claim)
        .collect())
}

#[tauri::command]
async fn download_release(
    release: ReleaseV1,
    only_files: Option<Vec<usize>>,
    state: State<'_, AppState>,
) -> Result<TorrentSummary, String> {
    let peers = state
        .discovery
        .wait_for_peers(release.torrent().info_hash, Duration::from_secs(60))
        .await
        .map_err(display_error)?;
    let magnet = format!("magnet:?xt=urn:btih:{}", release.torrent().info_hash);
    let output_dir = resolve_output_dir(None, &state).await?;
    let torrent = state
        .engine
        .add_magnet(
            &magnet,
            AddOptions {
                only_files,
                output_dir,
                initial_peers: Some(peers),
                disable_trackers: true,
                ..AddOptions::default()
            },
        )
        .await
        .map_err(display_error)?;
    torrent
        .wait_until_initialized()
        .await
        .map_err(display_error)?;
    Ok(torrent_summary(&torrent))
}

#[tauri::command]
async fn import_magnet(
    request: ImportMagnetRequest,
    state: State<'_, AppState>,
) -> Result<TorrentSummary, String> {
    let initial_peers =
        if let Some(hash) = magnet_v1_info_hash(&request.magnet).map_err(display_error)? {
            let info_hash: InfoHashV1 = hash.parse().map_err(display_error)?;
            tokio::time::timeout(
                MAINLINE_IMPORT_LOOKUP_TIMEOUT,
                state.discovery.lookup(info_hash),
            )
            .await
            .unwrap_or_default()
            .into_iter()
            .map(std::net::SocketAddr::V4)
            .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
    let output_dir = resolve_output_dir(request.save_path.as_deref(), &state).await?;
    let torrent = state
        .engine
        .add_magnet(
            &request.magnet,
            AddOptions {
                only_files: request.only_files,
                output_dir,
                initial_peers: (!initial_peers.is_empty()).then_some(initial_peers),
                ..AddOptions::default()
            },
        )
        .await
        .map_err(display_error)?;
    torrent
        .wait_until_initialized()
        .await
        .map_err(display_error)?;
    Ok(torrent_summary(&torrent))
}

#[tauri::command]
async fn import_torrent_file(
    request: ImportTorrentFileRequest,
    state: State<'_, AppState>,
) -> Result<TorrentSummary, String> {
    let torrent_path = canonical_file(&request.torrent_path, "torrent file")?;
    let metainfo = read_bounded_file(
        &torrent_path,
        state.engine.config().metainfo_limits.max_metainfo_bytes,
    )
    .await?;
    let output_dir = resolve_output_dir(request.save_path.as_deref(), &state).await?;
    let torrent = state
        .engine
        .add_metainfo(
            &metainfo,
            AddOptions {
                only_files: request.only_files,
                output_dir,
                ..AddOptions::default()
            },
        )
        .await
        .map_err(display_error)?;
    torrent
        .wait_until_initialized()
        .await
        .map_err(display_error)?;
    Ok(torrent_summary(&torrent))
}

#[tauri::command]
async fn connect_qbittorrent(
    request: QbittorrentConnectRequest,
    state: State<'_, AppState>,
) -> Result<QbittorrentStatus, String> {
    let client = if request.allow_remote {
        QbittorrentClient::approved_remote(&request.base_url)
    } else {
        QbittorrentClient::local(&request.base_url)
    }
    .map_err(display_error)?;
    client
        .login(&request.username, &request.password)
        .await
        .map_err(display_error)?;
    let version = client.version().await.map_err(display_error)?;
    *state.qbittorrent.write().await = Some(Arc::new(client));
    Ok(QbittorrentStatus {
        connected: true,
        version: Some(version),
    })
}

#[tauri::command]
async fn disconnect_qbittorrent(state: State<'_, AppState>) -> Result<QbittorrentStatus, String> {
    let client = state.qbittorrent.write().await.take();
    if let Some(client) = client {
        client.logout().await.map_err(display_error)?;
    }
    Ok(QbittorrentStatus {
        connected: false,
        version: None,
    })
}

#[tauri::command]
async fn get_qbittorrent_status(state: State<'_, AppState>) -> Result<QbittorrentStatus, String> {
    let client = state.qbittorrent.read().await.clone();
    let Some(client) = client else {
        return Ok(QbittorrentStatus {
            connected: false,
            version: None,
        });
    };
    let version = client.version().await.map_err(display_error)?;
    Ok(QbittorrentStatus {
        connected: true,
        version: Some(version),
    })
}

#[tauri::command]
async fn send_magnet_to_qbittorrent(
    magnet: String,
    save_path: Option<String>,
    tags: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let client = state
        .qbittorrent
        .read()
        .await
        .clone()
        .ok_or_else(|| "qBittorrent is not connected".to_owned())?;
    client
        .add_magnet(&magnet, save_path.as_deref(), None, &tags)
        .await
        .map_err(display_error)
}

#[tauri::command]
async fn list_qbittorrent_torrents(
    state: State<'_, AppState>,
) -> Result<Vec<QbittorrentTorrentInfo>, String> {
    let client = state
        .qbittorrent
        .read()
        .await
        .clone()
        .ok_or_else(|| "qBittorrent is not connected".to_owned())?;
    client.torrents().await.map_err(display_error)
}

#[tauri::command]
async fn import_completed_qbittorrent_torrent(
    hash: String,
    state: State<'_, AppState>,
) -> Result<TorrentSummary, String> {
    let client = state
        .qbittorrent
        .read()
        .await
        .clone()
        .ok_or_else(|| "qBittorrent is not connected".to_owned())?;
    let info = client
        .torrents()
        .await
        .map_err(display_error)?
        .into_iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&hash))
        .ok_or_else(|| format!("qBittorrent does not contain hash {hash}"))?;
    if info.progress < 1.0 {
        return Err(
            "only completed qBittorrent payloads can be imported safely; finish this torrent first"
                .to_owned(),
        );
    }
    let metainfo = client
        .export_torrent(
            &hash,
            state.engine.config().metainfo_limits.max_metainfo_bytes,
        )
        .await
        .map_err(display_error)?;
    if info.content_path.is_empty() {
        return Err(
            "qBittorrent did not report a local content path; remote or legacy layouts cannot be imported safely"
                .to_owned(),
        );
    }
    let content_path = Path::new(&info.content_path)
        .canonicalize()
        .map_err(|error| {
            format!(
                "cannot access qBittorrent content path {:?}: {error}",
                info.content_path
            )
        })?;
    let output_dir = if content_path.is_dir() {
        content_path
    } else if content_path.is_file() {
        content_path
            .parent()
            .ok_or_else(|| "qBittorrent content file has no parent directory".to_owned())?
            .to_path_buf()
    } else {
        return Err("qBittorrent content path is neither a file nor directory".to_owned());
    };
    let torrent = state
        .engine
        .add_metainfo(
            &metainfo,
            AddOptions {
                output_dir: Some(output_dir),
                overwrite: true,
                ..AddOptions::default()
            },
        )
        .await
        .map_err(display_error)?;
    torrent
        .wait_until_initialized()
        .await
        .map_err(display_error)?;
    Ok(torrent_summary(&torrent))
}

#[tauri::command]
async fn list_torrents(state: State<'_, AppState>) -> Result<Vec<TorrentSummary>, String> {
    Ok(state.engine.list().iter().map(torrent_summary).collect())
}

#[tauri::command]
async fn pause_torrent(torrent_id: usize, state: State<'_, AppState>) -> Result<(), String> {
    torrent_for_id(&state, torrent_id)?
        .pause()
        .await
        .map_err(display_error)
}

#[tauri::command]
async fn resume_torrent(torrent_id: usize, state: State<'_, AppState>) -> Result<(), String> {
    torrent_for_id(&state, torrent_id)?
        .resume()
        .await
        .map_err(display_error)
}

#[tauri::command]
async fn forget_torrent(
    torrent_id: usize,
    delete_files: bool,
    state: State<'_, AppState>,
) -> Result<(), String> {
    if delete_files {
        return Err(
            "payload deletion is disabled until the app persists proof that files were created by Torky; remove the transfer without deleting files"
                .to_owned(),
        );
    }
    torrent_for_id(&state, torrent_id)?
        .forget(false)
        .await
        .map_err(display_error)
}

#[tauri::command]
async fn update_torrent_files(
    torrent_id: usize,
    files: Vec<usize>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    torrent_for_id(&state, torrent_id)?
        .update_only_files(&files)
        .await
        .map_err(display_error)
}

#[tauri::command]
async fn get_stream_url(
    torrent_id: usize,
    file_index: usize,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let torrent = state
        .engine
        .get(torrent_id)
        .ok_or_else(|| format!("unknown torrent id {torrent_id}"))?;
    let metadata = torrent.metadata().map_err(display_error)?;
    if file_index >= metadata.files.len() {
        return Err(format!("unknown file index {file_index}"));
    }
    Ok(state.gateway.url(torrent_id, file_index))
}

async fn fetch_external_catalog(
    client: &reqwest::Client,
    source: &CatalogSource,
    query: &str,
    limit: usize,
    api_key: Option<&str>,
) -> Result<Vec<CatalogItem>, String> {
    if source.requires_api_key && api_key.is_none() {
        return Err("API key required for this session".to_owned());
    }
    let endpoint = validate_source_url(&source.endpoint).map_err(display_error)?;
    let request_url = match source.kind {
        SourceKind::Rss => endpoint,
        SourceKind::Torznab => {
            torznab_search_url(&endpoint, query, limit, api_key).map_err(display_error)?
        }
    };
    let mut response = client
        .get(request_url)
        .header(
            "accept",
            "application/rss+xml, application/xml, text/xml;q=0.9",
        )
        .send()
        .await
        .map_err(|error| format!("catalog request failed: {}", error.without_url()))?;
    if !response.status().is_success() {
        return Err(format!(
            "catalog returned HTTP status {}",
            response.status()
        ));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(format!(
            "catalog response exceeds {MAX_RESPONSE_BYTES} bytes"
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("catalog response failed: {}", error.without_url()))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(format!(
                "catalog response exceeds {MAX_RESPONSE_BYTES} bytes"
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let parse_limit = if source.kind == SourceKind::Rss {
        catalog_client::MAX_RESULTS
    } else {
        limit
    };
    let mut items = parse_catalog(source.id, &source.name, source.kind, &body, parse_limit)
        .map_err(display_error)?;
    if source.kind == SourceKind::Rss && !query.is_empty() {
        let query = query.to_lowercase();
        items.retain(|item| catalog_item_matches(item, &query));
    }
    items.truncate(limit);
    Ok(items)
}

fn catalog_item_matches(item: &CatalogItem, query: &str) -> bool {
    item.title.to_lowercase().contains(query)
        || item.description.to_lowercase().contains(query)
        || item
            .info_hash
            .as_ref()
            .is_some_and(|value| value.contains(query))
        || item.tags.iter().any(|tag| tag.contains(query))
}

fn normalize_api_key(value: Option<String>) -> Result<Option<String>, String> {
    let value = value.map(|value| value.trim().to_owned());
    if value
        .as_ref()
        .is_some_and(|value| value.len() > 512 || value.chars().any(char::is_control))
    {
        return Err("API key exceeds 512 characters or contains control characters".to_owned());
    }
    Ok(value.filter(|value| !value.is_empty()))
}

fn normalize_catalog_tags(values: Vec<String>) -> Result<Vec<String>, String> {
    if values.len() > 16 {
        return Err("at most 16 tags can be published at once".to_owned());
    }
    let mut tags = values
        .into_iter()
        .map(|value| value.trim().to_lowercase())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    tags.sort();
    tags.dedup();
    if tags.is_empty() {
        return Err("enter at least one tag".to_owned());
    }
    if tags.iter().any(|tag| {
        tag.len() > 32
            || tag.chars().any(char::is_control)
            || !tag.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            })
    }) {
        return Err("tags must use 1..=32 lowercase ASCII letters, digits, or hyphens".to_owned());
    }
    Ok(tags)
}

async fn synchronize_tag_claims(state: &AppState, issuer: &PublisherId) -> Result<(), String> {
    let public_key = issuer.public_key();
    let resources = state
        .adapter
        .list_public(public_key, TAG_CLAIMS_PATH, None, 1_000)
        .await
        .map_err(display_error)?;
    for resource in resources {
        let claim: TagClaimV1 = state
            .adapter
            .get_public_json(public_key, resource.path.as_str())
            .await
            .map_err(display_error)?;
        if claim.issuer() != issuer {
            return Err(format!(
                "tag claim {} does not match its Pubky authority",
                claim.id()
            ));
        }
        state.store.cache_tag_claim(&claim).map_err(display_error)?;
    }
    Ok(())
}

async fn sync_publisher_releases(
    state: &AppState,
    publisher: &PublisherId,
) -> Result<Vec<ReleaseV1>, String> {
    let user = publisher.public_key().clone();
    let Ok(resources) = state
        .adapter
        .list_public(&user, RELEASES_PATH, None, 1_000)
        .await
    else {
        return state.store.releases_for(publisher).map_err(display_error);
    };
    let mut releases = Vec::with_capacity(resources.len());
    for resource in resources {
        let release: ReleaseV1 = state
            .adapter
            .get_public_json(&user, resource.path.as_str())
            .await
            .map_err(display_error)?;
        state.store.cache_release(&release).map_err(display_error)?;
        releases.push(release);
    }
    releases.sort_by_key(ReleaseV1::created_at);
    releases.reverse();
    Ok(releases)
}

async fn auth_status(state: &AppState) -> Result<AuthStatus, String> {
    let session = state.session.read().await;
    Ok(AuthStatus {
        authenticated: session.is_some(),
        user: session
            .as_ref()
            .map(|value| value.info().public_key().to_string()),
    })
}

async fn current_session(state: &AppState) -> Result<PubkySession, String> {
    state
        .session
        .read()
        .await
        .clone()
        .ok_or_else(|| "authenticate with Pubky Ring before publishing".to_owned())
}

fn followed(state: &AppState) -> Result<Vec<String>, String> {
    state
        .store
        .followed_publishers()
        .map(|values| values.into_iter().map(|value| value.to_string()).collect())
        .map_err(display_error)
}

fn subject_tag_claim(claim: TagClaimV1) -> SubjectTagClaim {
    let info_hash = match claim.subject() {
        SubjectRef::Torrent(reference) => reference.v1().map(|hash| hash.to_string()),
        SubjectRef::Uri(_) => None,
    };
    SubjectTagClaim {
        issuer: claim.issuer().to_string(),
        tag: claim.tag().to_owned(),
        subject: claim.subject().to_string(),
        info_hash,
        created_at: claim.created_at(),
        revision: claim.revision(),
    }
}

fn torrent_for_id(state: &AppState, torrent_id: usize) -> Result<torrent_engine::Torrent, String> {
    state
        .engine
        .get(torrent_id)
        .ok_or_else(|| format!("unknown torrent id {torrent_id}"))
}

fn canonical_file(value: &str, description: &str) -> Result<PathBuf, String> {
    let path = Path::new(value)
        .canonicalize()
        .map_err(|error| format!("cannot open {description} {value:?}: {error}"))?;
    if !path.is_file() {
        return Err(format!(
            "{description} is not a regular file: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn canonical_output_dir(value: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let path = Path::new(value)
        .canonicalize()
        .map_err(|error| format!("cannot open save path {value:?}: {error}"))?;
    if !path.is_dir() {
        return Err(format!("save path is not a directory: {}", path.display()));
    }
    Ok(Some(path))
}

async fn resolve_output_dir(
    override_path: Option<&str>,
    state: &AppState,
) -> Result<Option<PathBuf>, String> {
    if let Some(path) = canonical_output_dir(override_path)? {
        return Ok(Some(path));
    }
    Ok(state.preferred_download_dir.read().await.clone())
}

fn resolve_preferred_download_dir(value: Option<&str>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    if path.exists() {
        let path = path
            .canonicalize()
            .map_err(|error| format!("cannot open download directory {value:?}: {error}"))?;
        if !path.is_dir() {
            return Err(format!(
                "download directory is not a directory: {}",
                path.display()
            ));
        }
        return Ok(Some(path));
    }
    std::fs::create_dir_all(&path)
        .map_err(|error| format!("cannot create download directory {value:?}: {error}"))?;
    path.canonicalize()
        .map(Some)
        .map_err(|error| format!("cannot open download directory {value:?}: {error}"))
}

fn kbps_to_bps(kbps: Option<u32>) -> Option<u32> {
    kbps.and_then(|value| value.checked_mul(1024))
}

fn listen_port_range_from_settings(settings: &ClientSettings) -> Range<u16> {
    if let Some(port) = settings.listen_port {
        if port == u16::MAX {
            return (port - 1)..port;
        }
        return port..(port + 1);
    }
    available_port_range()
}

fn engine_status_from(state: &AppState) -> EngineStatus {
    let config = state.engine.config();
    let settings = state.store.client_settings().unwrap_or_default();
    EngineStatus {
        download_dir: state
            .preferred_download_dir
            .try_read()
            .ok()
            .and_then(|guard| guard.clone())
            .unwrap_or_else(|| config.download_dir.clone())
            .display()
            .to_string(),
        listen_port: state.engine.listen_port(),
        dht_enabled: state.session_network.dht_enabled,
        upnp_enabled: state.session_network.upnp_enabled,
        download_limit_kbps: settings.download_limit_kbps,
        upload_limit_kbps: settings.upload_limit_kbps,
    }
}

fn engine_config_from_settings(
    default_download_dir: PathBuf,
    persistence_dir: PathBuf,
    settings: &ClientSettings,
) -> Result<(EngineConfig, Option<PathBuf>), String> {
    let preferred = resolve_preferred_download_dir(settings.download_dir.as_deref())?;
    let download_dir = preferred
        .clone()
        .unwrap_or(default_download_dir);
    std::fs::create_dir_all(&download_dir).map_err(display_error)?;
    let mut engine_config = EngineConfig::new(download_dir);
    engine_config.persistence_dir = Some(persistence_dir);
    engine_config.fastresume = true;
    engine_config.dht_mode = if settings.dht_enabled {
        DhtMode::Persistent
    } else {
        DhtMode::Disabled
    };
    engine_config.enable_upnp_port_forwarding = settings.upnp_enabled;
    engine_config.listen_port_range = Some(listen_port_range_from_settings(settings));
    engine_config.download_bps = kbps_to_bps(settings.download_limit_kbps);
    engine_config.upload_bps = kbps_to_bps(settings.upload_limit_kbps);
    Ok((engine_config, preferred))
}

async fn read_bounded_file(path: &Path, max_bytes: usize) -> Result<Vec<u8>, String> {
    let file = tokio::fs::File::open(path).await.map_err(display_error)?;
    let declared_len = file.metadata().await.map_err(display_error)?.len();
    let max_bytes_u64 = u64::try_from(max_bytes).map_err(display_error)?;
    if declared_len > max_bytes_u64 {
        return Err(format!(
            "torrent file is {declared_len} bytes, maximum allowed is {max_bytes}"
        ));
    }

    let capacity = usize::try_from(declared_len).map_err(display_error)?;
    let mut bytes = Vec::with_capacity(capacity);
    file.take(max_bytes_u64.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(display_error)?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "torrent file exceeds the maximum allowed size of {max_bytes} bytes"
        ));
    }
    Ok(bytes)
}

fn canonical_source(value: &str) -> Result<PathBuf, String> {
    let path = Path::new(value);
    if !path.exists() {
        return Err(format!("source does not exist: {}", path.display()));
    }
    path.canonicalize().map_err(display_error)
}

fn unix_millis() -> Result<u64, String> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(display_error)?
        .as_millis();
    u64::try_from(millis).map_err(display_error)
}

fn torrent_summary(torrent: &torrent_engine::Torrent) -> TorrentSummary {
    let progress = torrent.progress();
    let files = torrent
        .metadata()
        .map(|metadata| {
            metadata
                .files
                .into_iter()
                .map(|file| TorrentFileSummary {
                    index: file.index,
                    path: file.path.to_string_lossy().into_owned(),
                    length: file.length,
                    included: file.included,
                })
                .collect()
        })
        .unwrap_or_default();
    TorrentSummary {
        id: torrent.id(),
        info_hash: torrent.info_hash(),
        name: torrent.name(),
        state: format!("{:?}", progress.state).to_lowercase(),
        progress_bytes: progress.progress_bytes,
        total_bytes: progress.total_bytes,
        uploaded_bytes: progress.uploaded_bytes,
        download_mbps: progress.download_mbps,
        upload_mbps: progress.upload_mbps,
        peers_connected: progress.peers_connected,
        peers_seen: progress.peers_seen,
        ratio: progress.ratio,
        eta: progress.eta,
        finished: progress.finished,
        error: progress.error,
        files,
    }
}

fn available_port_range() -> Range<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("operating system must allocate a local TCP port");
    let start = u32::from(
        listener
            .local_addr()
            .expect("reserved listener address")
            .port(),
    );
    drop(listener);
    let end = (start + 64).min(u32::from(u16::MAX));
    let (start, end) = if end > start {
        (start, end)
    } else {
        (start - 1, start)
    };
    u16::try_from(start).expect("valid start port")..u16::try_from(end).expect("valid end port")
}

fn display_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn configured_catalog_url() -> Result<Option<Url>, Box<dyn std::error::Error>> {
    let value = match std::env::var("PUBKY_SWARM_DISCOVERY_URL") {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let mut url = Url::parse(&value)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "PUBKY_SWARM_DISCOVERY_URL must be a credential-free HTTP(S) base URL",
        )
        .into());
    }
    if !url.path().ends_with('/') {
        url.set_path(&format!("{}/", url.path()));
    }
    Ok(Some(url))
}

fn first_valid_magnet(urls: Vec<Url>) -> Option<String> {
    urls.into_iter().find_map(|url| {
        (url.scheme() == "magnet" && magnet_v1_info_hash(url.as_str()).ok().flatten().is_some())
            .then(|| url.to_string())
    })
}

fn queue_opened_magnets(app: &tauri::AppHandle, urls: Vec<Url>) {
    let Some(magnet) = first_valid_magnet(urls) else {
        return;
    };
    let state = app.state::<AppState>();
    if let Ok(mut pending) = state.pending_magnet.lock() {
        *pending = Some(magnet);
        let _ = app.emit(MAGNET_OPENED_EVENT, ());
        focus_main_window(app);
    }
}

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn magnets_from_argv(argv: &[String]) -> Vec<Url> {
    argv.iter()
        .filter_map(|arg| Url::parse(arg).ok())
        .filter(|url| url.scheme() == "magnet")
        .collect()
}

fn application_builder() -> tauri::Builder<tauri::Wry> {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, argv, _working_directory| {
            // Browser magnet clicks often launch a second process while Swarm is
            // already open. The deep-link / Opened paths may not fire in that case;
            // argv is the handoff that does.
            queue_opened_magnets(app, magnets_from_argv(&argv));
            focus_main_window(app);
        },
    ));
    builder
}

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Torky/0.1")
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Build and run the native Torky application.
///
/// # Panics
///
/// Panics if Tauri cannot build its runtime or generated application context.
pub fn run() {
    let application = application_builder()
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let store = Store::open(data_dir.join("swarm.sqlite3"))?;
            let settings = store.client_settings()?;
            let (engine_config, preferred_download_dir) = engine_config_from_settings(
                data_dir.join("downloads"),
                data_dir.join("torrent-state"),
                &settings,
            )
            .map_err(|error| std::io::Error::other(error))?;
            let engine = Arc::new(tauri::async_runtime::block_on(TorrentEngine::new(
                engine_config,
            ))?);
            let gateway = tauri::async_runtime::block_on(StreamGateway::start(engine.clone()))?;
            let discovery = PeerDiscovery::new(Dht::client()?.as_async());
            let adapter = PubkyAdapter::mainnet()?;
            let catalog_url = configured_catalog_url()?;
            app.manage(AppState {
                adapter,
                engine,
                discovery,
                store,
                gateway,
                auth_flow: Mutex::new(None),
                session: RwLock::new(None),
                qbittorrent: RwLock::new(None),
                catalog_api_keys: RwLock::new(HashMap::new()),
                tag_publish_lock: Mutex::new(()),
                pending_magnet: std::sync::Mutex::new(None),
                preferred_download_dir: RwLock::new(preferred_download_dir),
                session_network: SessionNetwork {
                    dht_enabled: settings.dht_enabled,
                    upnp_enabled: settings.upnp_enabled,
                    listen_port: settings.listen_port,
                },
                catalog_url,
                http: http_client()?,
            });
            let handle = app.handle().clone();
            app.deep_link()
                .on_open_url(move |event| queue_opened_magnets(&handle, event.urls()));
            if let Some(urls) = app.deep_link().get_current()? {
                queue_opened_magnets(app.handle(), urls);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_auth,
            poll_auth,
            get_auth_status,
            sign_out,
            take_pending_magnet,
            get_settings,
            get_engine_status,
            update_settings,
            get_profile,
            list_releases,
            search_catalog,
            list_external_catalog_sources,
            list_rss_presets,
            add_rss_feed,
            add_external_catalog_source,
            set_external_catalog_source_enabled,
            set_external_catalog_api_key,
            remove_external_catalog_source,
            search_external_catalogs,
            publish_catalog_tags,
            list_subject_tags,
            search_cached_tag_claims,
            create_release,
            follow_publisher,
            unfollow_publisher,
            list_followed,
            sync_followed,
            download_release,
            import_magnet,
            import_torrent_file,
            connect_qbittorrent,
            disconnect_qbittorrent,
            get_qbittorrent_status,
            send_magnet_to_qbittorrent,
            list_qbittorrent_torrents,
            import_completed_qbittorrent_torrent,
            list_torrents,
            pause_torrent,
            resume_torrent,
            forget_torrent,
            update_torrent_files,
            get_stream_url,
        ])
        .build(tauri::generate_context!())
        .expect("build Torky application");

    application.run(|handle, event| match event {
        tauri::RunEvent::Exit => {
            let state = handle.state::<AppState>();
            let _ = state.gateway.shutdown();
            let engine = state.engine.clone();
            tauri::async_runtime::block_on(engine.shutdown());
        }
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        tauri::RunEvent::Opened { urls } => queue_opened_magnets(handle, urls),
        _ => {}
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_tags_are_normalized_before_any_publication() {
        assert_eq!(
            normalize_catalog_tags(vec![
                " Research ".to_owned(),
                "research".to_owned(),
                "public-domain".to_owned(),
            ])
            .unwrap(),
            vec!["public-domain", "research"]
        );
        assert!(normalize_catalog_tags(vec!["not valid".to_owned()]).is_err());
        assert!(normalize_catalog_tags(vec!["not_valid".to_owned()]).is_err());
        assert!(normalize_catalog_tags(vec!["x".repeat(33)]).is_err());
        assert!(normalize_catalog_tags(Vec::new()).is_err());
    }

    #[test]
    fn catalog_api_keys_are_bounded_and_empty_values_clear_the_key() {
        assert_eq!(
            normalize_api_key(Some(" secret ".to_owned())).unwrap(),
            Some("secret".to_owned())
        );
        assert_eq!(normalize_api_key(Some(" ".to_owned())).unwrap(), None);
        assert!(normalize_api_key(Some("x".repeat(513))).is_err());
    }

    #[test]
    fn single_instance_argv_extracts_magnet_urls() {
        let hash = "3CA6678F769E5D37076F56EE935B84D3C28BF14E";
        let magnet = format!("magnet:?xt=urn:btih:{hash}&dn=Shared");
        let urls = magnets_from_argv(&[
            "/Applications/Torky.app/Contents/MacOS/Torky".to_owned(),
            magnet.clone(),
            "--flag".to_owned(),
        ]);
        assert_eq!(urls.len(), 1);
        assert_eq!(first_valid_magnet(urls), Some(magnet));
    }

    #[test]
    fn native_deep_link_boundary_accepts_only_valid_btih_magnets() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            first_valid_magnet(vec![
                Url::parse("https://example.com/not-a-magnet").unwrap(),
                Url::parse(&format!("magnet:?xt=urn:btih:{hash}&dn=Shared")).unwrap(),
            ]),
            Some(format!("magnet:?xt=urn:btih:{hash}&dn=Shared"))
        );
        assert!(
            first_valid_magnet(vec![Url::parse("magnet:?dn=missing-infohash").unwrap()]).is_none()
        );
        let browser_magnet = concat!(
            "magnet:?xt=urn:btih:3CA6678F769E5D37076F56EE935B84D3C28BF14E",
            "&dn=Raspberry%20Pi%20Pico%20Tips%20and%20Tricks%20",
            "(2024-01-28)%20by%20Malcolm%20Maclean%20EPUB",
            "&tr=udp%3A%2F%2Ftracker.opentrackr.org%3A1337",
            "&tr=udp%3A%2F%2Fopen.stealth.si%3A80%2Fannounce",
            "&tr=udp%3A%2F%2Ftracker.torrent.eu.org%3A451%2Fannounce",
            "&tr=udp%3A%2F%2Ftracker.bittor.pw%3A1337%2Fannounce",
            "&tr=udp%3A%2F%2Fpublic.popcorn-tracker.org%3A6969%2Fannounce",
            "&tr=udp%3A%2F%2Ftracker.dler.org%3A6969%2Fannounce",
            "&tr=udp%3A%2F%2Fexodus.desync.com%3A6969",
            "&tr=udp%3A%2F%2Fopen.demonii.com%3A1337%2Fannounce",
            "&tr=udp%3A%2F%2Fglotorrents.pw%3A6969%2Fannounce",
            "&tr=udp%3A%2F%2Ftracker.coppersurfer.tk%3A6969",
            "&tr=udp%3A%2F%2Ftorrent.gresille.org%3A80%2Fannounce",
            "&tr=udp%3A%2F%2Fp4p.arenabg.com%3A1337",
            "&tr=udp%3A%2F%2Ftracker.internetwarriors.net%3A1337",
        );
        let queued = first_valid_magnet(vec![Url::parse(browser_magnet).unwrap()])
            .expect("browser magnet must reach the native queue");
        assert_eq!(
            magnet_v1_info_hash(&queued).unwrap().as_deref(),
            Some("3ca6678f769e5d37076f56ee935b84d3c28bf14e")
        );
    }

    #[test]
    fn rss_search_matches_normalized_metadata() {
        let item = CatalogItem {
            source_id: 1,
            source_name: "Academic Torrents".to_owned(),
            title: "Open Research Corpus".to_owned(),
            description: "Reproducible dataset".to_owned(),
            magnet: "magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567".to_owned(),
            info_hash: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
            size: Some(42),
            tags: vec!["dataset".to_owned()],
            details_url: None,
            non_authoritative: true,
            client_validation_required: true,
            provenance: catalog_client::CatalogProvenance::RssHint,
        };
        assert!(catalog_item_matches(&item, "research"));
        assert!(catalog_item_matches(&item, "dataset"));
        assert!(!catalog_item_matches(&item, "film"));
    }
}
