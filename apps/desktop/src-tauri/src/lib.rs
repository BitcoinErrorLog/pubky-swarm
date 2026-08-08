//! Native application boundary for Pubky Swarm.

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
use swarm_store::{CatalogSource, Store};
use tauri::{Manager, State};
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, RwLock};
use torrent_engine::{
    AddOptions, CreateOptions, DhtMode, EngineConfig, TorrentEngine, magnet_v1_info_hash,
};
use url::Url;

const MAINLINE_IMPORT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);
const EXTERNAL_CATALOG_CONCURRENCY: usize = 4;

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
    catalog_url: Url,
    http: reqwest::Client,
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
    let endpoint = state.catalog_url.join("v1/search").map_err(display_error)?;
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
async fn list_followed(state: State<'_, AppState>) -> Result<Vec<String>, String> {
    followed(&state)
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
    let torrent = state
        .engine
        .add_magnet(
            &magnet,
            AddOptions {
                only_files,
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
    let output_dir = canonical_output_dir(request.save_path.as_deref())?;
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
    let output_dir = canonical_output_dir(request.save_path.as_deref())?;
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
            "payload deletion is disabled until the app persists proof that files were created by Pubky Swarm; remove the transfer without deleting files"
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

#[cfg(desktop)]
fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn application_builder() -> tauri::Builder<tauri::Wry> {
    let builder = tauri::Builder::default();
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(
        |app, _argv, _working_directory| focus_main_window(app),
    ));
    builder
}

fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Pubky-Swarm/0.1")
        .build()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Build and run the native Pubky Swarm application.
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
            let mut engine_config = EngineConfig::new(data_dir.join("downloads"));
            engine_config.persistence_dir = Some(data_dir.join("torrent-state"));
            engine_config.fastresume = true;
            engine_config.dht_mode = DhtMode::Disabled;
            engine_config.listen_port_range = Some(available_port_range());
            engine_config.enable_upnp_port_forwarding = true;
            let engine = Arc::new(tauri::async_runtime::block_on(TorrentEngine::new(
                engine_config,
            ))?);
            let gateway = tauri::async_runtime::block_on(StreamGateway::start(engine.clone()))?;
            let discovery = PeerDiscovery::new(Dht::client()?.as_async());
            let adapter = PubkyAdapter::mainnet()?;
            let store = Store::open(data_dir.join("swarm.sqlite3"))?;
            let mut catalog_url = Url::parse(
                &std::env::var("PUBKY_SWARM_DISCOVERY_URL")
                    .unwrap_or_else(|_| "http://127.0.0.1:7780/".to_owned()),
            )?;
            if !matches!(catalog_url.scheme(), "http" | "https")
                || !catalog_url.username().is_empty()
                || catalog_url.password().is_some()
                || catalog_url.query().is_some()
                || catalog_url.fragment().is_some()
            {
                return Err(
                    "PUBKY_SWARM_DISCOVERY_URL must be a credential-free HTTP(S) base URL".into(),
                );
            }
            if !catalog_url.path().ends_with('/') {
                catalog_url.set_path(&format!("{}/", catalog_url.path()));
            }
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
                catalog_url,
                http: http_client()?,
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            start_auth,
            poll_auth,
            get_auth_status,
            get_profile,
            list_releases,
            search_catalog,
            list_external_catalog_sources,
            add_external_catalog_source,
            set_external_catalog_source_enabled,
            set_external_catalog_api_key,
            remove_external_catalog_source,
            search_external_catalogs,
            publish_catalog_tags,
            create_release,
            follow_publisher,
            list_followed,
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
        .expect("build Pubky Swarm application");

    application.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let state = handle.state::<AppState>();
            let _ = state.gateway.shutdown();
            let engine = state.engine.clone();
            tauri::async_runtime::block_on(engine.shutdown());
        }
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
