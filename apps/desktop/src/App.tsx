import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { getCurrent, onOpenUrl } from "@tauri-apps/plugin-deep-link";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "./api";
import appIcon from "../app-icon.png";
import type {
  AuthStatus,
  Profile,
  QbittorrentStatus,
  QbittorrentTorrent,
  ReleaseV1,
  TorrentSummary,
} from "./types";
import "./App.css";

type View = "library" | "discover" | "publish";
type Player = { url: string; name: string };
type FileEditor = { torrent: TorrentSummary; selected: Set<number> };
type ReleaseImport = { release: ReleaseV1; selected: Set<number> };
type Removal = { torrent: TorrentSummary };

const viewCopy: Record<View, { eyebrow: string; title: string; detail: string }> = {
  library: {
    eyebrow: "Local swarm",
    title: "Library",
    detail: "Add, inspect, stream, and control every transfer from one place.",
  },
  discover: {
    eyebrow: "Pubky network",
    title: "Discover",
    detail: "Resolve publishers and retrieve releases with authenticated metadata.",
  },
  publish: {
    eyebrow: "Your releases",
    title: "Publish",
    detail: "Create a torrent, seed it locally, and announce it under your Pubky.",
  },
};

function App() {
  const [view, setView] = useState<View>("library");
  const [auth, setAuth] = useState<AuthStatus>({ authenticated: false, user: null });
  const [authUrl, setAuthUrl] = useState<string | null>(null);
  const [publisher, setPublisher] = useState("");
  const [catalogQuery, setCatalogQuery] = useState("");
  const [catalogResults, setCatalogResults] = useState<ReleaseV1[]>([]);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [releases, setReleases] = useState<ReleaseV1[]>([]);
  const [followed, setFollowed] = useState<string[]>([]);
  const [torrents, setTorrents] = useState<TorrentSummary[]>([]);
  const [player, setPlayer] = useState<Player | null>(null);
  const [magnet, setMagnet] = useState("");
  const [torrentPath, setTorrentPath] = useState("");
  const [savePath, setSavePath] = useState("");
  const [sourcePath, setSourcePath] = useState("");
  const [title, setTitle] = useState("");
  const [description, setDescription] = useState("");
  const [tags, setTags] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [releaseImport, setReleaseImport] = useState<ReleaseImport | null>(null);
  const [fileEditor, setFileEditor] = useState<FileEditor | null>(null);
  const [removal, setRemoval] = useState<Removal | null>(null);
  const [qbittorrent, setQbittorrent] = useState<QbittorrentStatus>({
    connected: false,
    version: null,
  });
  const [qbitUrl, setQbitUrl] = useState("http://127.0.0.1:8080");
  const [qbitUsername, setQbitUsername] = useState("admin");
  const [qbitPassword, setQbitPassword] = useState("");
  const [qbitAllowRemote, setQbitAllowRemote] = useState(false);
  const [qbitLibrary, setQbitLibrary] = useState<QbittorrentTorrent[]>([]);

  const refreshTorrents = useCallback(async (surfaceError = true) => {
    try {
      setTorrents(await api.torrents());
    } catch (reason) {
      if (surfaceError) setError(errorMessage(reason));
    }
  }, []);

  useEffect(() => {
    api.authStatus().then(setAuth).catch((reason) => setError(errorMessage(reason)));
    api.followed().then(setFollowed).catch((reason) => setError(errorMessage(reason)));
    api.qbittorrentStatus().then(setQbittorrent).catch(() => {
      setQbittorrent({ connected: false, version: null });
    });
    void refreshTorrents();
    const interval = window.setInterval(() => void refreshTorrents(false), 2_000);
    return () => window.clearInterval(interval);
  }, [refreshTorrents]);

  useEffect(() => {
    let active = true;
    let stopListening: (() => void) | undefined;
    const receiveMagnet = (urls: string[]) => {
      const value = urls.find(isMagnet);
      if (!value) return;
      setMagnet(value);
      setView("library");
      setError(null);
      window.focus();
    };

    void getCurrent()
      .then((urls) => {
        if (active && urls) receiveMagnet(urls);
      })
      .catch((reason) => {
        if (active) setError(`Could not read the incoming magnet link: ${errorMessage(reason)}`);
      });
    void onOpenUrl(receiveMagnet)
      .then((unlisten) => {
        if (active) {
          stopListening = unlisten;
        } else {
          unlisten();
        }
      })
      .catch((reason) => {
        if (active) setError(`Could not listen for magnet links: ${errorMessage(reason)}`);
      });

    return () => {
      active = false;
      stopListening?.();
    };
  }, []);

  useEffect(() => {
    if (!authUrl || auth.authenticated) return;
    const interval = window.setInterval(async () => {
      try {
        const status = await api.pollAuth();
        setAuth(status);
        if (status.authenticated) setAuthUrl(null);
      } catch (reason) {
        setError(errorMessage(reason));
        setAuthUrl(null);
      }
    }, 2_000);
    return () => window.clearInterval(interval);
  }, [auth.authenticated, authUrl]);

  useEffect(() => {
    if (!releaseImport && !fileEditor && !removal) return;
    const close = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setReleaseImport(null);
      setFileEditor(null);
      setRemoval(null);
    };
    window.addEventListener("keydown", close);
    return () => window.removeEventListener("keydown", close);
  }, [releaseImport, fileEditor, removal]);

  const totals = useMemo(() => {
    const active = torrents.filter((torrent) => !torrent.finished && !isPaused(torrent));
    return {
      active: active.length,
      download: torrents.reduce((sum, torrent) => sum + safeNumber(torrent.downloadMbps), 0),
      upload: torrents.reduce((sum, torrent) => sum + safeNumber(torrent.uploadMbps), 0),
      peers: torrents.reduce((sum, torrent) => sum + safeNumber(torrent.peersConnected), 0),
    };
  }, [torrents]);

  async function run(label: string, action: () => Promise<void>) {
    setBusy(label);
    setError(null);
    try {
      await action();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(null);
    }
  }

  async function beginAuth() {
    await run("Starting Pubky authorization", async () => {
      const result = await api.startAuth();
      setAuthUrl(result.authorization_url);
      await openUrl(result.authorization_url);
    });
  }

  async function discover(event: FormEvent) {
    event.preventDefault();
    const target = publisher.trim();
    if (!target) return;
    await run("Resolving publisher", async () => {
      const [nextProfile, nextReleases] = await Promise.all([
        api.profile(target),
        api.releases(target),
      ]);
      setProfile(nextProfile);
      setReleases(nextReleases);
    });
  }

  async function followResolvedPublisher() {
    const target = publisher.trim();
    if (!target) return;
    await run("Following publisher", async () => {
      setFollowed(await api.follow(target));
    });
  }

  async function searchCatalog(event: FormEvent) {
    event.preventDefault();
    await run("Searching opt-in Pubky catalogs", async () => {
      setCatalogResults(await api.searchCatalog(catalogQuery));
    });
  }

  async function publish(event: FormEvent) {
    event.preventDefault();
    await run("Hashing, seeding, and publishing release", async () => {
      const release = await api.createRelease({
        sourcePath,
        title,
        description,
        tags: tags.split(",").map((tag) => tag.trim().toLowerCase()).filter(Boolean),
      });
      setSourcePath("");
      setTitle("");
      setDescription("");
      setTags("");
      if (publisher === auth.user) setReleases((current) => [release, ...current]);
      await refreshTorrents();
      setView("library");
    });
  }

  async function importMagnet(event: FormEvent) {
    event.preventDefault();
    if (!isMagnet(magnet)) {
      setError("Enter a valid magnet link beginning with magnet:?");
      return;
    }
    await run("Adding magnet to your library", async () => {
      await api.importMagnet({
        magnet: magnet.trim(),
        ...(savePath ? { savePath } : {}),
      });
      setMagnet("");
      await refreshTorrents();
    });
  }

  async function importTorrentFile() {
    if (!torrentPath) return;
    await run("Reading torrent metadata", async () => {
      const summary = await api.importTorrentFile({
        torrentPath,
        ...(savePath ? { savePath } : {}),
      });
      setTorrentPath("");
      await refreshTorrents();
      if (summary.files.length > 1) {
        setFileEditor({
          torrent: summary,
          selected: new Set(summary.files.filter((file) => file.included).map((file) => file.index)),
        });
      }
    });
  }

  async function pickTorrent() {
    const path = await openDialog({
      multiple: false,
      directory: false,
      filters: [{ name: "Torrent metadata", extensions: ["torrent"] }],
    });
    if (typeof path === "string") setTorrentPath(path);
  }

  async function pickSavePath() {
    const path = await openDialog({ multiple: false, directory: true });
    if (typeof path === "string") setSavePath(path);
  }

  async function pickSource(directory: boolean) {
    const path = await openDialog({ multiple: false, directory });
    if (typeof path === "string") {
      setSourcePath(path);
      if (!title) setTitle(fileName(path));
    }
  }

  function prepareRelease(release: ReleaseV1) {
    if (release.torrent.files.length <= 1) {
      void downloadRelease(release, release.torrent.files.map((_, index) => index));
      return;
    }
    setReleaseImport({
      release,
      selected: new Set(release.torrent.files.map((_, index) => index)),
    });
  }

  async function downloadRelease(release: ReleaseV1, onlyFiles: number[]) {
    await run(`Finding peers for ${release.title}`, async () => {
      await api.downloadRelease(release, onlyFiles);
      setReleaseImport(null);
      await refreshTorrents();
      setView("library");
    });
  }

  async function toggleTransfer(torrent: TorrentSummary) {
    await run(`${isPaused(torrent) ? "Resuming" : "Pausing"} ${displayName(torrent)}`, async () => {
      if (isPaused(torrent)) await api.resumeTorrent(torrent.id);
      else await api.pauseTorrent(torrent.id);
      await refreshTorrents();
    });
  }

  async function removeTorrent() {
    if (!removal) return;
    await run(`Removing ${displayName(removal.torrent)}`, async () => {
      await api.forgetTorrent(removal.torrent.id, false);
      if (player && removal.torrent.files.some((file) => file.path === player.name)) {
        setPlayer(null);
      }
      setRemoval(null);
      await refreshTorrents();
    });
  }

  async function updateFiles() {
    if (!fileEditor || fileEditor.selected.size === 0) return;
    await run(`Updating files for ${displayName(fileEditor.torrent)}`, async () => {
      await api.updateTorrentFiles(fileEditor.torrent.id, [...fileEditor.selected]);
      setFileEditor(null);
      await refreshTorrents();
    });
  }

  async function play(torrent: TorrentSummary, fileIndex: number, name: string) {
    await run(`Preparing ${fileName(name)}`, async () => {
      setPlayer({ url: await api.streamUrl(torrent.id, fileIndex), name });
    });
  }

  async function handoffMagnet(value: string) {
    await run("Opening magnet in your default client", () => openUrl(value));
  }

  async function connectQbittorrent(event: FormEvent) {
    event.preventDefault();
    await run("Connecting to qBittorrent", async () => {
      setQbittorrent(
        await api.connectQbittorrent({
          baseUrl: qbitUrl,
          username: qbitUsername,
          password: qbitPassword,
          allowRemote: qbitAllowRemote,
        }),
      );
      setQbitPassword("");
    });
  }

  async function disconnectQbittorrent() {
    await run("Disconnecting qBittorrent", async () => {
      setQbittorrent(await api.disconnectQbittorrent());
      setQbitLibrary([]);
    });
  }

  async function sendToQbittorrent(
    magnetValue: string,
    releaseTags: string[] = [],
  ) {
    await run("Sending magnet to qBittorrent", async () => {
      await api.sendToQbittorrent(
        magnetValue,
        savePath || undefined,
        releaseTags,
      );
    });
  }

  async function loadQbittorrentLibrary() {
    await run("Loading qBittorrent library", async () => {
      setQbitLibrary(await api.qbittorrentTorrents());
    });
  }

  async function importQbittorrentTorrent(hash: string) {
    await run("Importing completed qBittorrent payload", async () => {
      await api.importQbittorrentTorrent(hash);
      await refreshTorrents();
    });
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <img className="brand-mark" src={appIcon} alt="" />
          <div><strong>Pubky Swarm</strong><span>Desktop alpha</span></div>
        </div>
        <nav aria-label="Primary navigation">
          {(["library", "discover", "publish"] as View[]).map((item) => (
            <button
              key={item}
              className={view === item ? "active" : ""}
              aria-current={view === item ? "page" : undefined}
              onClick={() => setView(item)}
            >
              <NavIcon name={item} />
              <span>{capitalize(item)}</span>
              {item === "library" && torrents.length > 0 && <b>{torrents.length}</b>}
            </button>
          ))}
        </nav>
        <div className={`identity-card ${auth.authenticated ? "online" : ""}`}>
          <span className="status-dot" />
          <div>
            <strong>{auth.authenticated ? "Publisher connected" : "Pubky not connected"}</strong>
            <span>{auth.user ? shortKey(auth.user) : "Connect to publish releases"}</span>
          </div>
        </div>
      </aside>

      <div className="workspace">
        <header className="page-header">
          <div>
            <p className="eyebrow">{viewCopy[view].eyebrow}</p>
            <h1>{viewCopy[view].title}</h1>
            <p>{viewCopy[view].detail}</p>
          </div>
          <div className="engine-status" title="Local torrent engine">
            <span className="status-dot" />
            Engine online
          </div>
        </header>

        <main>
          {error && (
            <div className="notice error" role="alert">
              <AlertIcon />
              <div><strong>Couldn’t complete that action</strong><span>{error}</span></div>
              <button onClick={() => setError(null)} aria-label="Dismiss error">×</button>
            </div>
          )}
          {busy && (
            <div className="notice working" role="status" aria-live="polite">
              <span className="spinner" aria-hidden="true" />
              {busy}
            </div>
          )}

          {view === "library" && (
            <Library
              torrents={torrents}
              totals={totals}
              magnet={magnet}
              torrentPath={torrentPath}
              savePath={savePath}
              busy={Boolean(busy)}
              player={player}
              onMagnetChange={setMagnet}
              onMagnetSubmit={importMagnet}
              onPickTorrent={pickTorrent}
              onImportTorrent={importTorrentFile}
              onPickSavePath={pickSavePath}
              onClearSavePath={() => setSavePath("")}
              onHandoff={() => void handoffMagnet(magnet.trim())}
              onRefresh={() => void refreshTorrents()}
              onToggle={(torrent) => void toggleTransfer(torrent)}
              onRemove={(torrent) => setRemoval({ torrent })}
              onEditFiles={(torrent) => setFileEditor({
                torrent,
                selected: new Set(torrent.files.filter((file) => file.included).map((file) => file.index)),
              })}
              onPlay={(torrent, index, name) => void play(torrent, index, name)}
              onClosePlayer={() => setPlayer(null)}
              qbittorrent={qbittorrent}
              qbitUrl={qbitUrl}
              qbitUsername={qbitUsername}
              qbitPassword={qbitPassword}
              qbitAllowRemote={qbitAllowRemote}
              onQbitUrlChange={setQbitUrl}
              onQbitUsernameChange={setQbitUsername}
              onQbitPasswordChange={setQbitPassword}
              onQbitAllowRemoteChange={setQbitAllowRemote}
              onConnectQbittorrent={connectQbittorrent}
              onDisconnectQbittorrent={() => void disconnectQbittorrent()}
              onSendQbittorrent={() =>
                void sendToQbittorrent(magnet.trim())
              }
              qbitLibrary={qbitLibrary}
              onLoadQbittorrentLibrary={() =>
                void loadQbittorrentLibrary()
              }
              onImportQbittorrentTorrent={(hash) =>
                void importQbittorrentTorrent(hash)
              }
            />
          )}

          {view === "discover" && (
            <Discover
              publisher={publisher}
              profile={profile}
              releases={releases}
              followed={followed}
              busy={Boolean(busy)}
              onPublisherChange={setPublisher}
              onDiscover={discover}
              onFollowed={(value) => setPublisher(value)}
              onDownload={prepareRelease}
              onHandoff={(release) => void handoffMagnet(releaseMagnet(release))}
              qbittorrentConnected={qbittorrent.connected}
              onSendQbittorrent={(release) =>
                void sendToQbittorrent(
                  releaseMagnet(release),
                  release.tags,
                )
              }
              catalogQuery={catalogQuery}
              catalogResults={catalogResults}
              onCatalogQueryChange={setCatalogQuery}
              onCatalogSearch={searchCatalog}
              onFollowPublisher={() => void followResolvedPublisher()}
            />
          )}

          {view === "publish" && (
            <Publish
              auth={auth}
              authUrl={authUrl}
              sourcePath={sourcePath}
              title={title}
              description={description}
              tags={tags}
              busy={Boolean(busy)}
              onAuth={() => void beginAuth()}
              onReopenAuth={() => authUrl && void openUrl(authUrl)}
              onSourceChange={setSourcePath}
              onPickSource={(directory) => void pickSource(directory)}
              onTitleChange={setTitle}
              onDescriptionChange={setDescription}
              onTagsChange={setTags}
              onPublish={publish}
            />
          )}
        </main>
      </div>

      {releaseImport && (
        <FileSelectionDialog
          title={releaseImport.release.title}
          description="Choose what to download before adding this release to your library."
          files={releaseImport.release.torrent.files.map((file, index) => ({
            index, path: file.path, length: file.size,
          }))}
          selected={releaseImport.selected}
          busy={Boolean(busy)}
          confirmLabel="Add selected files"
          onChange={(selected) => setReleaseImport({ ...releaseImport, selected })}
          onCancel={() => setReleaseImport(null)}
          onConfirm={() => void downloadRelease(releaseImport.release, [...releaseImport.selected])}
        />
      )}

      {fileEditor && (
        <FileSelectionDialog
          title={displayName(fileEditor.torrent)}
          description="Included files are downloaded and kept available to stream."
          files={fileEditor.torrent.files}
          selected={fileEditor.selected}
          busy={Boolean(busy)}
          confirmLabel="Update selection"
          onChange={(selected) => setFileEditor({ ...fileEditor, selected })}
          onCancel={() => setFileEditor(null)}
          onConfirm={() => void updateFiles()}
        />
      )}

      {removal && (
        <div className="modal-backdrop" role="presentation">
          <section className="dialog remove-dialog" role="dialog" aria-modal="true" aria-labelledby="remove-title">
            <div className="danger-icon"><TrashIcon /></div>
            <h2 id="remove-title">Remove “{displayName(removal.torrent)}”?</h2>
            <p>
              The transfer is removed from this library. Payload files remain
              on disk so publisher originals and shared qBittorrent data cannot
              be deleted accidentally.
            </p>
            <div className="dialog-actions">
              <button className="secondary" onClick={() => setRemoval(null)}>Cancel</button>
              <button className="danger" disabled={Boolean(busy)} onClick={() => void removeTorrent()}>
                Remove torrent
              </button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

type LibraryProps = {
  torrents: TorrentSummary[];
  totals: { active: number; download: number; upload: number; peers: number };
  magnet: string;
  torrentPath: string;
  savePath: string;
  busy: boolean;
  player: Player | null;
  onMagnetChange: (value: string) => void;
  onMagnetSubmit: (event: FormEvent) => void;
  onPickTorrent: () => void;
  onImportTorrent: () => void;
  onPickSavePath: () => void;
  onClearSavePath: () => void;
  onHandoff: () => void;
  onRefresh: () => void;
  onToggle: (torrent: TorrentSummary) => void;
  onRemove: (torrent: TorrentSummary) => void;
  onEditFiles: (torrent: TorrentSummary) => void;
  onPlay: (torrent: TorrentSummary, index: number, name: string) => void;
  onClosePlayer: () => void;
  qbittorrent: QbittorrentStatus;
  qbitUrl: string;
  qbitUsername: string;
  qbitPassword: string;
  qbitAllowRemote: boolean;
  onQbitUrlChange: (value: string) => void;
  onQbitUsernameChange: (value: string) => void;
  onQbitPasswordChange: (value: string) => void;
  onQbitAllowRemoteChange: (value: boolean) => void;
  onConnectQbittorrent: (event: FormEvent) => void;
  onDisconnectQbittorrent: () => void;
  onSendQbittorrent: () => void;
  qbitLibrary: QbittorrentTorrent[];
  onLoadQbittorrentLibrary: () => void;
  onImportQbittorrentTorrent: (hash: string) => void;
};

function Library(props: LibraryProps) {
  return (
    <>
      <section className="stat-grid" aria-label="Transfer totals">
        <Stat label="Active" value={String(props.totals.active)} detail={`${props.torrents.length} in library`} />
        <Stat label="Download" value={formatSpeed(props.totals.download)} detail="current aggregate" />
        <Stat label="Upload" value={formatSpeed(props.totals.upload)} detail="current aggregate" />
        <Stat label="Peers" value={String(props.totals.peers)} detail="connected now" />
      </section>

      <section className="panel add-panel">
        <div className="section-heading">
          <div><p className="eyebrow">Add transfer</p><h2>Bring content into your library</h2></div>
        </div>
        <form className="magnet-form" onSubmit={props.onMagnetSubmit}>
          <label htmlFor="magnet">Magnet link</label>
          <div className="input-action">
            <input
              id="magnet"
              value={props.magnet}
              onChange={(event) => props.onMagnetChange(event.target.value)}
              placeholder="magnet:?xt=urn:btih:…"
              spellCheck={false}
              autoComplete="off"
              required
            />
            <button className="primary" type="submit" disabled={props.busy}>Add magnet</button>
          </div>
          <button
            className="text-button external-link"
            type="button"
            disabled={!isMagnet(props.magnet) || props.busy}
            onClick={props.onHandoff}
          >
            Open magnet in another torrent client ↗
          </button>
          {props.qbittorrent.connected && (
            <button
              className="text-button external-link"
              type="button"
              disabled={!isMagnet(props.magnet) || props.busy}
              onClick={props.onSendQbittorrent}
            >
              Send magnet to qBittorrent
            </button>
          )}
        </form>
        <div className="import-divider"><span>or use a local file</span></div>
        <div className="file-import-row">
          <button className="file-picker" type="button" onClick={props.onPickTorrent}>
            <FileIcon />
            <span><strong>{props.torrentPath ? fileName(props.torrentPath) : "Choose a .torrent file"}</strong>
              <small>{props.torrentPath || "Open native file picker"}</small></span>
          </button>
          <button className="secondary" disabled={!props.torrentPath || props.busy} onClick={props.onImportTorrent}>
            Add torrent
          </button>
        </div>
        <div className="save-path-row">
          <div><FolderIcon /><span><strong>Save location</strong><small>{props.savePath || "App default"}</small></span></div>
          {props.savePath && <button className="text-button" onClick={props.onClearSavePath}>Use default</button>}
          <button className="secondary compact" onClick={props.onPickSavePath}>Choose folder</button>
        </div>
      </section>

      <section className="panel qbit-panel">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Existing-client bridge</p>
            <h2>qBittorrent WebUI</h2>
          </div>
          {props.qbittorrent.connected && (
            <span>Connected {props.qbittorrent.version}</span>
          )}
        </div>
        {props.qbittorrent.connected ? (
          <div className="qbit-connected">
            <p>
              Pubky releases and arbitrary magnets can be sent directly to
              your existing qBittorrent library.
            </p>
            <button
              className="secondary"
              type="button"
              onClick={props.onDisconnectQbittorrent}
            >
              Disconnect
            </button>
            <button
              className="secondary"
              type="button"
              onClick={props.onLoadQbittorrentLibrary}
            >
              Load migration list
            </button>
          </div>
        ) : (
          <form className="qbit-form" onSubmit={props.onConnectQbittorrent}>
            <label>
              WebUI URL
              <input
                value={props.qbitUrl}
                onChange={(event) => props.onQbitUrlChange(event.target.value)}
                placeholder="http://127.0.0.1:8080"
                required
              />
            </label>
            <label>
              Username
              <input
                value={props.qbitUsername}
                onChange={(event) =>
                  props.onQbitUsernameChange(event.target.value)
                }
                autoComplete="username"
                required
              />
            </label>
            <label>
              Password
              <input
                type="password"
                value={props.qbitPassword}
                onChange={(event) =>
                  props.onQbitPasswordChange(event.target.value)
                }
                autoComplete="current-password"
                required
              />
            </label>
            <label className="remote-check">
              <input
                type="checkbox"
                checked={props.qbitAllowRemote}
                onChange={(event) =>
                  props.onQbitAllowRemoteChange(event.target.checked)
                }
              />
              Explicitly allow a non-loopback WebUI host
            </label>
            <button className="primary" type="submit" disabled={props.busy}>
              Connect
            </button>
          </form>
        )}
        {props.qbitLibrary.length > 0 && (
          <div className="qbit-library">
            {props.qbitLibrary.map((torrent) => (
              <div key={torrent.hash}>
                <span>
                  <strong>{torrent.name}</strong>
                  <small>
                    {formatBytes(torrent.size)} · {torrent.category || "No category"}
                  </small>
                </span>
                <button
                  className="secondary compact"
                  disabled={torrent.progress < 1 || props.busy}
                  title={
                    torrent.progress < 1
                      ? "Complete this torrent in qBittorrent before importing"
                      : "Recheck existing payload in Pubky Swarm"
                  }
                  onClick={() =>
                    props.onImportQbittorrentTorrent(torrent.hash)
                  }
                >
                  {torrent.progress < 1 ? "Incomplete" : "Import"}
                </button>
              </div>
            ))}
          </div>
        )}
      </section>

      {props.player && (
        <section className="panel player-panel">
          <div className="section-heading">
            <div><p className="eyebrow">Streaming now</p><h2>{fileName(props.player.name)}</h2></div>
            <button className="icon-button" aria-label="Close player" onClick={props.onClosePlayer}>×</button>
          </div>
          <video key={props.player.url} src={props.player.url} controls autoPlay />
        </section>
      )}

      <section className="transfers-section">
        <div className="section-heading">
          <div><p className="eyebrow">Transfers</p><h2>Your torrents</h2></div>
          <button className="secondary compact" onClick={props.onRefresh}>Refresh</button>
        </div>
        {props.torrents.length === 0 ? (
          <div className="empty-state">
            <DownloadIcon />
            <h3>Your library is empty</h3>
            <p>Add a magnet, open a .torrent file, or download a publisher release.</p>
          </div>
        ) : (
          <div className="torrent-list">
            {props.torrents.map((torrent) => (
              <TorrentCard
                key={torrent.id}
                torrent={torrent}
                busy={props.busy}
                onToggle={() => props.onToggle(torrent)}
                onRemove={() => props.onRemove(torrent)}
                onEditFiles={() => props.onEditFiles(torrent)}
                onPlay={(index, name) => props.onPlay(torrent, index, name)}
              />
            ))}
          </div>
        )}
      </section>
    </>
  );
}

function TorrentCard({ torrent, busy, onToggle, onRemove, onEditFiles, onPlay }: {
  torrent: TorrentSummary;
  busy: boolean;
  onToggle: () => void;
  onRemove: () => void;
  onEditFiles: () => void;
  onPlay: (index: number, name: string) => void;
}) {
  const percent = torrent.totalBytes > 0
    ? Math.min(100, (torrent.progressBytes / torrent.totalBytes) * 100)
    : 0;
  const included = torrent.files.filter((file) => file.included);
  const playable = included.filter((file) => isPlayable(file.path));
  return (
    <article className="torrent-card">
      <div className="torrent-card-header">
        <div className={`state-icon ${torrent.finished ? "complete" : isPaused(torrent) ? "paused" : ""}`}>
          {torrent.finished ? <CheckIcon /> : isPaused(torrent) ? <PauseIcon /> : <DownloadIcon />}
        </div>
        <div className="torrent-title">
          <h3>{displayName(torrent)}</h3>
          <span>{torrent.state} · {formatBytes(torrent.progressBytes)} of {formatBytes(torrent.totalBytes)}</span>
        </div>
        <div className="torrent-actions">
          {!torrent.finished && (
            <button className="secondary compact" disabled={busy} onClick={onToggle}>
              {isPaused(torrent) ? <PlayIcon /> : <PauseIcon />}
              {isPaused(torrent) ? "Resume" : "Pause"}
            </button>
          )}
          <button className="icon-button" aria-label={`Remove ${displayName(torrent)}`} onClick={onRemove}>
            <TrashIcon />
          </button>
        </div>
      </div>
      {torrent.error && <div className="torrent-error" role="alert">{torrent.error}</div>}
      <div className="progress-line">
        <div className="progress" aria-label={`${percent.toFixed(0)} percent complete`}>
          <span style={{ width: `${percent}%` }} />
        </div>
        <strong>{percent.toFixed(0)}%</strong>
      </div>
      <div className="metric-grid">
        <Metric label="Down" value={formatSpeed(torrent.downloadMbps)} />
        <Metric label="Up" value={formatSpeed(torrent.uploadMbps)} />
        <Metric label="Peers" value={`${safeNumber(torrent.peersConnected)} / ${safeNumber(torrent.peersSeen)}`} />
        <Metric label="ETA" value={torrent.finished ? "Complete" : formatEta(torrent.eta)} />
        <Metric label="Ratio" value={safeNumber(torrent.ratio).toFixed(2)} />
      </div>
      <div className="torrent-footer">
        <code title={torrent.infoHash}>{shortHash(torrent.infoHash)}</code>
        <span>{included.length} of {torrent.files.length} files</span>
        {torrent.files.length > 0 && <button className="text-button" onClick={onEditFiles}>Manage files</button>}
      </div>
      {playable.length > 0 && (
        <div className="playable-files">
          {playable.map((file) => (
            <button key={file.index} onClick={() => onPlay(file.index, file.path)}>
              <PlayIcon /><span>Play {fileName(file.path)}</span><small>{formatBytes(file.length)}</small>
            </button>
          ))}
        </div>
      )}
    </article>
  );
}

function Discover({ publisher, profile, releases, followed, busy, onPublisherChange, onDiscover, onFollowed, onDownload, onHandoff, qbittorrentConnected, onSendQbittorrent, catalogQuery, catalogResults, onCatalogQueryChange, onCatalogSearch, onFollowPublisher }: {
  publisher: string;
  profile: Profile | null;
  releases: ReleaseV1[];
  followed: string[];
  busy: boolean;
  onPublisherChange: (value: string) => void;
  onDiscover: (event: FormEvent) => void;
  onFollowed: (value: string) => void;
  onDownload: (release: ReleaseV1) => void;
  onHandoff: (release: ReleaseV1) => void;
  qbittorrentConnected: boolean;
  onSendQbittorrent: (release: ReleaseV1) => void;
  catalogQuery: string;
  catalogResults: ReleaseV1[];
  onCatalogQueryChange: (value: string) => void;
  onCatalogSearch: (event: FormEvent) => void;
  onFollowPublisher: () => void;
}) {
  return (
    <>
      <section className="panel catalog-search">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Opt-in federation</p>
            <h2>Search publisher catalogs</h2>
          </div>
          <span>Client validation required</span>
        </div>
        <form onSubmit={onCatalogSearch}>
          <div className="input-action">
            <input
              value={catalogQuery}
              onChange={(event) => onCatalogQueryChange(event.target.value)}
              placeholder="title, tag, publisher, or infohash"
              maxLength={256}
            />
            <button className="primary" type="submit" disabled={busy}>
              Search
            </button>
          </div>
        </form>
        {catalogResults.length > 0 && (
          <div className="catalog-results">
            {catalogResults.map((release) => (
              <div key={`${release.publisher}:${release.id}`}>
                <span>
                  <strong>{release.title}</strong>
                  <small>
                    {shortKey(release.publisher)} · {formatBytes(release.torrent.size)}
                  </small>
                </span>
                <div>
                  {qbittorrentConnected && (
                    <button
                      className="secondary compact"
                      onClick={() => onSendQbittorrent(release)}
                    >
                      qBittorrent
                    </button>
                  )}
                  <button className="primary" onClick={() => onDownload(release)}>
                    Choose files
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
      <section className="panel discover-search">
        <form onSubmit={onDiscover}>
          <label htmlFor="publisher">Publisher Pubky</label>
          <div className="input-action">
            <input
              id="publisher"
              value={publisher}
              onChange={(event) => onPublisherChange(event.target.value)}
              placeholder="pubky… or z-base32 public key"
              spellCheck={false}
              required
            />
            <button className="primary" type="submit" disabled={busy}>Resolve publisher</button>
          </div>
        </form>
        {followed.length > 0 && (
          <div className="followed">
            <span>Recent</span>
            {followed.map((value) => <button key={value} onClick={() => onFollowed(value)}>{shortKey(value)}</button>)}
          </div>
        )}
      </section>
      {profile && (
        <section className="profile-hero">
          <div className="avatar">{profile.name.slice(0, 2)}</div>
          <div><p className="eyebrow">Resolved publisher</p><h2>{profile.name}</h2>
            <p>{profile.bio || profile.status || "Pubky publisher"}</p>
            <button className="secondary compact" onClick={onFollowPublisher}>
              Follow publisher
            </button>
          </div>
          <code>{publisher}</code>
        </section>
      )}
      <section className="releases-section">
        <div className="section-heading">
          <div><p className="eyebrow">Authenticated metadata</p><h2>Publisher releases</h2></div>
          <span>{releases.length} {releases.length === 1 ? "release" : "releases"}</span>
        </div>
        {releases.length === 0 ? (
          <div className="empty-state"><SearchIcon /><h3>No releases loaded</h3>
            <p>Resolve a Pubky to inspect its signed release records.</p></div>
        ) : (
          <div className="release-grid">
            {releases.map((release) => (
              <article className="release-card" key={release.id}>
                <div className="release-meta">
                  <span>{formatBytes(release.torrent.size)}</span>
                  <span>{release.torrent.files.length} {release.torrent.files.length === 1 ? "file" : "files"}</span>
                </div>
                <h3>{release.title}</h3>
                <p>{release.description || "No description provided."}</p>
                {release.tags.length > 0 && <div className="tags">{release.tags.map((tag) => <span key={tag}>#{tag}</span>)}</div>}
                <code className="hash">btih:{release.torrent.info_hash}</code>
                <footer>
                  <span>{shortKey(release.publisher)}</span>
                  <div>
                    <button className="icon-button" title="Open in another torrent client" aria-label={`Open ${release.title} externally`} onClick={() => onHandoff(release)}>↗</button>
                    {qbittorrentConnected && (
                      <button
                        className="secondary compact"
                        onClick={() => onSendQbittorrent(release)}
                      >
                        qBittorrent
                      </button>
                    )}
                    <button className="primary" onClick={() => onDownload(release)}>Choose files</button>
                  </div>
                </footer>
              </article>
            ))}
          </div>
        )}
      </section>
    </>
  );
}

function Publish({ auth, authUrl, sourcePath, title, description, tags, busy, onAuth, onReopenAuth, onSourceChange, onPickSource, onTitleChange, onDescriptionChange, onTagsChange, onPublish }: {
  auth: AuthStatus;
  authUrl: string | null;
  sourcePath: string;
  title: string;
  description: string;
  tags: string;
  busy: boolean;
  onAuth: () => void;
  onReopenAuth: () => void;
  onSourceChange: (value: string) => void;
  onPickSource: (directory: boolean) => void;
  onTitleChange: (value: string) => void;
  onDescriptionChange: (value: string) => void;
  onTagsChange: (value: string) => void;
  onPublish: (event: FormEvent) => void;
}) {
  return (
    <>
      <section className={`panel auth-banner ${auth.authenticated ? "connected" : ""}`}>
        <div className="auth-mark"><KeyIcon /></div>
        <div>
          <p className="eyebrow">{auth.authenticated ? "Ready to publish" : "Publisher access required"}</p>
          <h2>{auth.authenticated ? "Pubky authorization active" : "Connect your Pubky"}</h2>
          <p>{auth.authenticated
            ? "Publishing uses a scoped grant. Your root key remains on your signing device."
            : "Approve a capability-scoped grant for the Swarm release namespace."}</p>
          {auth.user && <code>{auth.user}</code>}
        </div>
        {!auth.authenticated && (
          <div className="auth-actions">
            <button className="primary" disabled={busy} onClick={onAuth}>
              {authUrl ? "Waiting for approval…" : "Authorize with Pubky"}
            </button>
            {authUrl && <button className="text-button" onClick={onReopenAuth}>Reopen link ↗</button>}
          </div>
        )}
      </section>

      <section className="panel publish-panel">
        <div className="section-heading">
          <div><p className="eyebrow">New release</p><h2>Create and announce</h2></div>
        </div>
        <form className="publish-form" onSubmit={onPublish}>
          <label className="wide">
            Local file or directory
            <div className="source-field">
              <input
                value={sourcePath}
                onChange={(event) => onSourceChange(event.target.value)}
                placeholder="/absolute/path/to/content"
                required
                disabled={!auth.authenticated}
              />
              <button className="secondary" type="button" disabled={!auth.authenticated} onClick={() => onPickSource(false)}>Choose file</button>
              <button className="secondary" type="button" disabled={!auth.authenticated} onClick={() => onPickSource(true)}>Choose folder</button>
            </div>
          </label>
          <label>
            Title
            <input value={title} onChange={(event) => onTitleChange(event.target.value)} placeholder="Release title" required maxLength={200} disabled={!auth.authenticated} />
          </label>
          <label>
            Tags
            <input value={tags} onChange={(event) => onTagsChange(event.target.value)} placeholder="film, open-media" disabled={!auth.authenticated} />
          </label>
          <label className="wide">
            Description
            <textarea value={description} onChange={(event) => onDescriptionChange(event.target.value)} placeholder="Describe what this release contains" maxLength={4000} disabled={!auth.authenticated} />
            <small className="character-count">{description.length} / 4000</small>
          </label>
          <div className="wide publish-submit">
            <p>The local engine starts seeding after the release record is published.</p>
            <button className="primary" type="submit" disabled={!auth.authenticated || busy}>Create torrent and publish</button>
          </div>
        </form>
      </section>
    </>
  );
}

function FileSelectionDialog({ title, description, files, selected, busy, confirmLabel, onChange, onCancel, onConfirm }: {
  title: string;
  description: string;
  files: { index: number; path: string; length: number }[];
  selected: Set<number>;
  busy: boolean;
  confirmLabel: string;
  onChange: (selected: Set<number>) => void;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const allSelected = files.length > 0 && selected.size === files.length;
  function toggle(index: number) {
    const next = new Set(selected);
    if (next.has(index)) next.delete(index);
    else next.add(index);
    onChange(next);
  }
  return (
    <div className="modal-backdrop" role="presentation">
      <section className="dialog file-dialog" role="dialog" aria-modal="true" aria-labelledby="files-title">
        <div className="dialog-heading">
          <div><p className="eyebrow">File selection</p><h2 id="files-title">{title}</h2><p>{description}</p></div>
          <button className="icon-button" aria-label="Close file selection" onClick={onCancel}>×</button>
        </div>
        <div className="select-toolbar">
          <label>
            <input
              type="checkbox"
              checked={allSelected}
              onChange={() => onChange(allSelected ? new Set() : new Set(files.map((file) => file.index)))}
            />
            Select all
          </label>
          <span>{selected.size} of {files.length} selected · {formatBytes(files.filter((file) => selected.has(file.index)).reduce((sum, file) => sum + file.length, 0))}</span>
        </div>
        <div className="file-list">
          {files.map((file) => (
            <label key={file.index} className="file-option">
              <input type="checkbox" checked={selected.has(file.index)} onChange={() => toggle(file.index)} />
              <FileIcon />
              <span><strong>{fileName(file.path)}</strong><small>{file.path}</small></span>
              <b>{formatBytes(file.length)}</b>
            </label>
          ))}
        </div>
        {selected.size === 0 && <p className="selection-warning" role="alert">Select at least one file to continue.</p>}
        <div className="dialog-actions">
          <button className="secondary" onClick={onCancel}>Cancel</button>
          <button className="primary" disabled={selected.size === 0 || busy} onClick={onConfirm}>{confirmLabel}</button>
        </div>
      </section>
    </div>
  );
}

function Stat({ label, value, detail }: { label: string; value: string; detail: string }) {
  return <div className="stat"><span>{label}</span><strong>{value}</strong><small>{detail}</small></div>;
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div className="metric"><span>{label}</span><strong>{value}</strong></div>;
}

function shortKey(value: string) {
  return value.length > 22 ? `${value.slice(0, 12)}…${value.slice(-8)}` : value;
}

function shortHash(value: string) {
  return value.length > 20 ? `${value.slice(0, 10)}…${value.slice(-8)}` : value;
}

function displayName(torrent: TorrentSummary) {
  return torrent.name || shortHash(torrent.infoHash);
}

function formatBytes(value: number) {
  if (!Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const exponent = Math.min(units.length - 1, Math.floor(Math.log(value) / Math.log(1024)));
  return `${(value / 1024 ** exponent).toFixed(exponent === 0 ? 0 : 1)} ${units[exponent]}`;
}

function formatSpeed(value: number) {
  return `${safeNumber(value).toFixed(value >= 10 ? 1 : 2)} MB/s`;
}

function formatEta(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value) || value < 0) return "—";
  if (value < 60) return `${Math.ceil(value)}s`;
  if (value < 3600) return `${Math.ceil(value / 60)}m`;
  const hours = Math.floor(value / 3600);
  const minutes = Math.ceil((value % 3600) / 60);
  return `${hours}h ${minutes}m`;
}

function errorMessage(reason: unknown) {
  if (reason instanceof Error) return reason.message;
  if (typeof reason === "string") return reason;
  try {
    return JSON.stringify(reason);
  } catch {
    return "An unknown error occurred.";
  }
}

function isPlayable(path: string) {
  return /\.(mp4|m4v|webm|mov|mp3|m4a|ogg|wav)$/i.test(path);
}

function isPaused(torrent: TorrentSummary) {
  return /paused|stopped|idle/i.test(torrent.state);
}

function isMagnet(value: string) {
  return /^magnet:\?.*xt=urn:btih:/i.test(value.trim());
}

function releaseMagnet(release: ReleaseV1) {
  return `magnet:?xt=urn:btih:${encodeURIComponent(release.torrent.info_hash)}&dn=${encodeURIComponent(release.title)}`;
}

function fileName(path: string) {
  const clean = path.replace(/[\\/]+$/, "");
  return clean.split(/[\\/]/).pop() || clean;
}

function safeNumber(value: number | null | undefined) {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function capitalize(value: string) {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

type IconProps = { name?: View };
function NavIcon({ name }: IconProps) {
  if (name === "discover") return <svg viewBox="0 0 24 24"><circle cx="11" cy="11" r="6" /><path d="m16 16 4 4" /></svg>;
  if (name === "publish") return <svg viewBox="0 0 24 24"><path d="M12 16V4m0 0L7 9m5-5 5 5" /><path d="M5 15v4h14v-4" /></svg>;
  return <svg viewBox="0 0 24 24"><path d="M4 6h16v13H4z" /><path d="M8 6V4h8v2M8 11h8" /></svg>;
}
function AlertIcon() { return <svg viewBox="0 0 24 24"><path d="M12 4 3 20h18L12 4Z" /><path d="M12 9v5m0 3v.01" /></svg>; }
function FileIcon() { return <svg viewBox="0 0 24 24"><path d="M6 3h8l4 4v14H6z" /><path d="M14 3v5h5" /></svg>; }
function FolderIcon() { return <svg viewBox="0 0 24 24"><path d="M3 6h7l2 2h9v11H3z" /></svg>; }
function DownloadIcon() { return <svg viewBox="0 0 24 24"><path d="M12 4v11m0 0 5-5m-5 5-5-5M5 20h14" /></svg>; }
function SearchIcon() { return <svg viewBox="0 0 24 24"><circle cx="10" cy="10" r="6" /><path d="m15 15 5 5" /></svg>; }
function CheckIcon() { return <svg viewBox="0 0 24 24"><path d="m5 12 4 4L19 6" /></svg>; }
function PauseIcon() { return <svg viewBox="0 0 24 24"><path d="M8 5v14m8-14v14" /></svg>; }
function PlayIcon() { return <svg viewBox="0 0 24 24"><path d="m8 5 11 7-11 7z" /></svg>; }
function TrashIcon() { return <svg viewBox="0 0 24 24"><path d="M4 7h16M9 7V4h6v3m3 0-1 14H7L6 7m4 4v6m4-6v6" /></svg>; }
function KeyIcon() { return <svg viewBox="0 0 24 24"><circle cx="8" cy="12" r="4" /><path d="M12 12h9m-3 0v3m-3-3v2" /></svg>; }

export default App;
