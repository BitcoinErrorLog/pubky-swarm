import { invoke } from "@tauri-apps/api/core";
import type {
  AuthStart,
  AuthStatus,
  AddExternalCatalogSourceRequest,
  AddRssFeedRequest,
  CreateReleaseRequest,
  ExternalCatalogSearchResponse,
  ExternalCatalogSource,
  ImportMagnetRequest,
  ImportTorrentFileRequest,
  Profile,
  QbittorrentConnectRequest,
  QbittorrentStatus,
  QbittorrentTorrent,
  ReleaseV1,
  RssPresetInfo,
  TorrentSummary,
  ClientSettings,
  EngineStatus,
  UpdateSettingsResponse,
  SubjectTagClaim,
  SyncFollowedResponse,
} from "./types";

export const api = {
  startAuth: () => invoke<AuthStart>("start_auth"),
  pollAuth: () => invoke<AuthStatus>("poll_auth"),
  authStatus: () => invoke<AuthStatus>("get_auth_status"),
  signOut: () => invoke<AuthStatus>("sign_out"),
  takePendingMagnet: () => invoke<string | null>("take_pending_magnet"),
  profile: (user: string) => invoke<Profile>("get_profile", { user }),
  releases: (user: string) =>
    invoke<ReleaseV1[]>("list_releases", { user }),
  searchCatalog: (query: string, limit = 25) =>
    invoke<ReleaseV1[]>("search_catalog", { query, limit }),
  externalCatalogSources: () =>
    invoke<ExternalCatalogSource[]>("list_external_catalog_sources"),
  rssPresets: () => invoke<RssPresetInfo[]>("list_rss_presets"),
  addRssFeed: (request: AddRssFeedRequest) =>
    invoke<ExternalCatalogSource>("add_rss_feed", { request }),
  addExternalCatalogSource: (request: AddExternalCatalogSourceRequest) =>
    invoke<ExternalCatalogSource>("add_external_catalog_source", { request }),
  setExternalCatalogSourceEnabled: (sourceId: number, enabled: boolean) =>
    invoke<void>("set_external_catalog_source_enabled", { sourceId, enabled }),
  setExternalCatalogApiKey: (sourceId: number, apiKey?: string) =>
    invoke<ExternalCatalogSource>("set_external_catalog_api_key", {
      sourceId,
      apiKey: apiKey ?? null,
    }),
  removeExternalCatalogSource: (sourceId: number) =>
    invoke<void>("remove_external_catalog_source", { sourceId }),
  searchExternalCatalogs: (query: string, limit = 50) =>
    invoke<ExternalCatalogSearchResponse>("search_external_catalogs", {
      query,
      limit,
    }),
  publishCatalogTags: (infoHash: string, tags: string[]) =>
    invoke<string[]>("publish_catalog_tags", {
      request: { infoHash, tags },
    }),
  follow: (user: string) =>
    invoke<string[]>("follow_publisher", { user }),
  unfollow: (user: string) =>
    invoke<string[]>("unfollow_publisher", { user }),
  followed: () => invoke<string[]>("list_followed"),
  syncFollowed: () => invoke<SyncFollowedResponse>("sync_followed"),
  listSubjectTags: (infoHash: string) =>
    invoke<SubjectTagClaim[]>("list_subject_tags", { infoHash }),
  searchCachedTagClaims: (query: string) =>
    invoke<SubjectTagClaim[]>("search_cached_tag_claims", { query }),
  createRelease: (request: CreateReleaseRequest) =>
    invoke<ReleaseV1>("create_release", { request }),
  downloadRelease: (release: ReleaseV1, onlyFiles?: number[]) =>
    invoke<TorrentSummary>("download_release", {
      release,
      onlyFiles: onlyFiles ?? null,
    }),
  importMagnet: (request: ImportMagnetRequest) =>
    invoke<TorrentSummary>("import_magnet", { request }),
  importTorrentFile: (request: ImportTorrentFileRequest) =>
    invoke<TorrentSummary>("import_torrent_file", { request }),
  torrents: () => invoke<TorrentSummary[]>("list_torrents"),
  pauseTorrent: (torrentId: number) =>
    invoke<void>("pause_torrent", { torrentId }),
  resumeTorrent: (torrentId: number) =>
    invoke<void>("resume_torrent", { torrentId }),
  forgetTorrent: (torrentId: number, deleteFiles: boolean) =>
    invoke<void>("forget_torrent", { torrentId, deleteFiles }),
  updateTorrentFiles: (torrentId: number, files: number[]) =>
    invoke<void>("update_torrent_files", { torrentId, files }),
  connectQbittorrent: (request: QbittorrentConnectRequest) =>
    invoke<QbittorrentStatus>("connect_qbittorrent", { request }),
  disconnectQbittorrent: () =>
    invoke<QbittorrentStatus>("disconnect_qbittorrent"),
  qbittorrentStatus: () =>
    invoke<QbittorrentStatus>("get_qbittorrent_status"),
  sendToQbittorrent: (
    magnet: string,
    savePath: string | undefined,
    tags: string[],
  ) =>
    invoke<void>("send_magnet_to_qbittorrent", {
      magnet,
      savePath: savePath ?? null,
      tags,
    }),
  qbittorrentTorrents: () =>
    invoke<QbittorrentTorrent[]>("list_qbittorrent_torrents"),
  importQbittorrentTorrent: (hash: string) =>
    invoke<TorrentSummary>("import_completed_qbittorrent_torrent", { hash }),
  streamUrl: (torrentId: number, fileIndex: number) =>
    invoke<string>("get_stream_url", { torrentId, fileIndex }),
  settings: () => invoke<ClientSettings>("get_settings"),
  engineStatus: () => invoke<EngineStatus>("get_engine_status"),
  updateSettings: (settings: ClientSettings) =>
    invoke<UpdateSettingsResponse>("update_settings", { settings }),
};
