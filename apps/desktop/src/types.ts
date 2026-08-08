export type AuthStatus = {
  authenticated: boolean;
  user: string | null;
};

export type AuthStart = {
  authorization_url: string;
};

export type Profile = {
  name: string;
  bio?: string | null;
  image?: string | null;
  status?: string | null;
};

export type ReleaseFile = {
  path: string;
  size: number;
};

export type TorrentV1 = {
  info_hash: string;
  size: number;
  files: ReleaseFile[];
  trackers?: string[];
};

export type ReleaseV1 = {
  schema: "pubky.swarm/release";
  version: 1;
  id: string;
  publisher: string;
  created_at: number;
  title: string;
  description: string;
  torrent: TorrentV1;
  tags: string[];
};

export type CreateReleaseRequest = {
  sourcePath: string;
  title: string;
  description: string;
  tags: string[];
};

export type TorrentSummary = {
  id: number;
  infoHash: string;
  name: string | null;
  state: string;
  progressBytes: number;
  totalBytes: number;
  uploadedBytes: number;
  finished: boolean;
  error: string | null;
  downloadMbps: number;
  uploadMbps: number;
  peersConnected: number;
  peersSeen: number;
  ratio: number;
  eta: number | null;
  files: TorrentFileSummary[];
};

export type TorrentFileSummary = {
  index: number;
  path: string;
  length: number;
  included: boolean;
};

export type ImportTorrentRequest = {
  savePath?: string;
  onlyFiles?: number[];
};

export type ImportMagnetRequest = ImportTorrentRequest & {
  magnet: string;
};

export type ImportTorrentFileRequest = ImportTorrentRequest & {
  torrentPath: string;
};

export type QbittorrentStatus = {
  connected: boolean;
  version: string | null;
};

export type QbittorrentConnectRequest = {
  baseUrl: string;
  username: string;
  password: string;
  allowRemote: boolean;
};

export type QbittorrentTorrent = {
  hash: string;
  name: string;
  progress: number;
  dlspeed: number;
  upspeed: number;
  num_seeds: number;
  num_leechs: number;
  state: string;
  tags: string;
  category: string;
  save_path: string;
  content_path: string;
  size: number;
  ratio: number;
  eta: number;
};
