// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App, { isMagnet } from "./App";

const apiMock = vi.hoisted(() => ({
  authStatus: vi.fn(),
  startAuth: vi.fn(),
  pollAuth: vi.fn(),
  signOut: vi.fn(),
  takePendingMagnet: vi.fn(),
  followed: vi.fn(),
  unfollow: vi.fn(),
  syncFollowed: vi.fn(),
  listSubjectTags: vi.fn(),
  searchCachedTagClaims: vi.fn(),
  qbittorrentStatus: vi.fn(),
  torrents: vi.fn(),
  externalCatalogSources: vi.fn(),
  searchCatalog: vi.fn(),
  searchExternalCatalogs: vi.fn(),
  importMagnet: vi.fn(),
  publishCatalogTags: vi.fn(),
  settings: vi.fn(),
  engineStatus: vi.fn(),
  updateSettings: vi.fn(),
  rssPresets: vi.fn(),
  addRssFeed: vi.fn(),
  follow: vi.fn(),
  profile: vi.fn(),
  releases: vi.fn(),
}));

const openUrlMock = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));
const clipboardWriteMock = vi.hoisted(() => vi.fn().mockResolvedValue(undefined));

const eventMock = vi.hoisted(() => ({
  callbacks: [] as Array<() => void>,
  listen: vi.fn((_event: string, callback: () => void) => {
    eventMock.callbacks.push(callback);
    return Promise.resolve(() => undefined);
  }),
}));

vi.mock("./api", () => ({ api: apiMock }));
vi.mock("@tauri-apps/api/event", () => ({ listen: eventMock.listen }));
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: vi.fn().mockResolvedValue(null),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: openUrlMock,
}));
vi.mock("qrcode.react", () => ({
  QRCodeSVG: ({ value }: { value: string }) => (
    <svg data-testid="qr-code" data-value={value} />
  ),
}));

const infoHash = "0123456789abcdef0123456789abcdef01234567";
const magnet = `magnet:?xt=urn:btih:${infoHash}&dn=Open+Research`;

function externalResult() {
  return {
    sourceId: 1,
    sourceName: "Academic Torrents — Recent",
    title: "Open Research",
    description: "Public research dataset",
    magnet,
    infoHash,
    size: 42,
    tags: ["dataset"],
    detailsUrl: `https://academictorrents.com/details/${infoHash}`,
    nonAuthoritative: true,
    clientValidationRequired: true,
    provenance: "rss_hint" as const,
  };
}

beforeEach(() => {
  window.focus = vi.fn();
  clipboardWriteMock.mockReset();
  clipboardWriteMock.mockResolvedValue(undefined);
  Object.defineProperty(Navigator.prototype, "clipboard", {
    configurable: true,
    get: () => ({ writeText: clipboardWriteMock }),
  });
  apiMock.authStatus.mockResolvedValue({ authenticated: false, user: null });
  apiMock.startAuth.mockResolvedValue({
    authorization_url: "pubkyauth://approve?secret=test",
  });
  apiMock.pollAuth.mockResolvedValue({ authenticated: false, user: null });
  apiMock.signOut.mockResolvedValue({ authenticated: false, user: null });
  apiMock.takePendingMagnet.mockResolvedValue(null);
  apiMock.followed.mockResolvedValue([]);
  apiMock.unfollow.mockResolvedValue([]);
  apiMock.syncFollowed.mockResolvedValue({
    followed: [],
    releases: [],
    claimCount: 0,
  });
  apiMock.listSubjectTags.mockResolvedValue([]);
  apiMock.searchCachedTagClaims.mockResolvedValue([]);
  apiMock.follow.mockResolvedValue(["pubky1alice"]);
  apiMock.profile.mockResolvedValue({ name: "Alice", bio: "Publisher" });
  apiMock.releases.mockResolvedValue([]);
  apiMock.qbittorrentStatus.mockResolvedValue({ connected: false, version: null });
  apiMock.torrents.mockResolvedValue([]);
  apiMock.externalCatalogSources.mockResolvedValue([{
    id: 1,
    name: "Academic Torrents — Recent",
    kind: "rss",
    endpoint: "https://academictorrents.com/rss.xml",
    enabled: true,
    builtIn: true,
    requiresApiKey: false,
    addedAt: 1,
    hasApiKey: false,
  }]);
  apiMock.rssPresets.mockResolvedValue([{
    name: "Academic Torrents — Recent",
    endpoint: "https://academictorrents.com/rss.xml",
    enabledByDefault: true,
    description: "Latest public research datasets and papers on Academic Torrents.",
  }]);
  apiMock.addRssFeed.mockResolvedValue({
    id: 2,
    name: "example.org — feed",
    kind: "rss",
    endpoint: "https://example.org/feed.xml",
    enabled: true,
    builtIn: false,
    requiresApiKey: false,
    addedAt: 2,
    hasApiKey: false,
  });
  apiMock.searchCatalog.mockRejectedValue(new Error("optional discovery unavailable"));
  apiMock.searchExternalCatalogs.mockResolvedValue({
    results: [externalResult()],
    errors: [],
  });
  apiMock.importMagnet.mockResolvedValue({
    id: 7,
    infoHash,
    name: "Open Research",
    state: "initializing",
    progressBytes: 0,
    totalBytes: 42,
    uploadedBytes: 0,
    downloadMbps: 0,
    uploadMbps: 0,
    peersConnected: 0,
    peersSeen: 0,
    ratio: 0,
    eta: null,
    finished: false,
    error: null,
    files: [],
  });
  apiMock.publishCatalogTags.mockResolvedValue(["public-domain", "research"]);
  apiMock.settings.mockResolvedValue({
    downloadDir: null,
    dhtEnabled: true,
    upnpEnabled: true,
    downloadLimitKbps: null,
    uploadLimitKbps: null,
    listenPort: null,
  });
  apiMock.engineStatus.mockResolvedValue({
    downloadDir: "/tmp/downloads",
    listenPort: 51413,
    dhtEnabled: true,
    upnpEnabled: true,
    downloadLimitKbps: null,
    uploadLimitKbps: null,
  });
  apiMock.updateSettings.mockImplementation(async (settings) => ({
    settings,
    status: {
      downloadDir: settings.downloadDir || "/tmp/downloads",
      listenPort: 51413,
      dhtEnabled: settings.dhtEnabled,
      upnpEnabled: settings.upnpEnabled,
      downloadLimitKbps: settings.downloadLimitKbps,
      uploadLimitKbps: settings.uploadLimitKbps,
    },
    restartRequired: false,
  }));
  eventMock.callbacks.length = 0;
  openUrlMock.mockClear();
  clipboardWriteMock.mockClear();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("magnet validation", () => {
  it("accepts literal and percent-encoded btih targets", () => {
    expect(isMagnet(magnet)).toBe(true);
    expect(isMagnet(`magnet:?xt=urn%3Abtih%3A${infoHash}`)).toBe(true);
    expect(isMagnet("magnet:?dn=missing-hash")).toBe(false);
    expect(isMagnet("https://example.com/file.torrent")).toBe(false);
  });
});

describe("external catalog workflow", () => {
  it("consumes a cold-start browser magnet from the Rust handoff", async () => {
    apiMock.takePendingMagnet.mockResolvedValueOnce(magnet);
    render(<App />);

    expect(await screen.findByLabelText("Magnet link")).toHaveValue(magnet);
    expect(apiMock.takePendingMagnet).toHaveBeenCalled();
  });

  it("consumes a browser magnet while the app is already running", async () => {
    render(<App />);
    await waitFor(() => expect(eventMock.callbacks).toHaveLength(1));
    apiMock.takePendingMagnet.mockResolvedValueOnce(magnet);

    await act(async () => {
      eventMock.callbacks[0]();
    });

    expect(await screen.findByLabelText("Magnet link")).toHaveValue(magnet);
  });

  it("moves a catalog magnet into Library and submits it to the backend", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Discover" }));
    await user.type(screen.getByPlaceholderText("title, tag, publisher, or infohash"), "research");
    await user.click(screen.getByRole("button", { name: "Search" }));
    await user.click(await screen.findByRole("button", { name: "Add magnet" }));

    const magnetInput = await screen.findByLabelText("Magnet link");
    expect(magnetInput).toHaveValue(magnet);
    await user.click(screen.getByRole("button", { name: "Add magnet" }));

    await waitFor(() => {
      expect(apiMock.importMagnet).toHaveBeenCalledWith({ magnet });
    });
  });

  it("publishes normalized user tags for the selected infohash", async () => {
    apiMock.authStatus.mockResolvedValue({
      authenticated: true,
      user: "pubky1testpublisher",
    });
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Discover" }));
    await user.click(screen.getByRole("button", { name: "Search" }));
    const tags = await screen.findByPlaceholderText("public-domain, research");
    await user.type(tags, "Research, public-domain");
    await user.click(screen.getByRole("button", { name: "Publish tags" }));

    await waitFor(() => {
      expect(apiMock.publishCatalogTags).toHaveBeenCalledWith(
        infoHash,
        ["Research", "public-domain"],
      );
    });
  });
});

describe("settings", () => {
  it("loads defaults and saves an upload limit", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Settings" }));
    expect(await screen.findByText("Transfers and network")).toBeInTheDocument();
    await waitFor(() => expect(apiMock.settings).toHaveBeenCalled());

    const upload = screen.getByLabelText("Upload limit (KB/s)");
    await user.clear(upload);
    await user.type(upload, "256");
    await user.click(screen.getByRole("button", { name: "Save settings" }));

    await waitFor(() => {
      expect(apiMock.updateSettings).toHaveBeenCalledWith(expect.objectContaining({
        uploadLimitKbps: 256,
        dhtEnabled: true,
        upnpEnabled: true,
      }));
    });
  });
});

describe("rss feeds", () => {
  it("adds a pasted RSS feed URL from Discover", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Discover" }));
    expect(await screen.findByText("RSS catalogs")).toBeInTheDocument();
    await user.type(
      screen.getByPlaceholderText("https://example.org/feed.xml"),
      "https://example.org/open-research.xml",
    );
    await user.click(screen.getByRole("button", { name: "Add feed" }));

    await waitFor(() => {
      expect(apiMock.addRssFeed).toHaveBeenCalledWith({
        feedUrl: "https://example.org/open-research.xml",
      });
    });
  });
});

describe("auth handoff", () => {
  it("shows QR and copyable pubkyauth URL, then clears wait state on poll success", async () => {
    const user = userEvent.setup();
    const authUrl = "pubkyauth://approve?secret=test";
    apiMock.pollAuth.mockResolvedValue({ authenticated: true, user: "pubky1tester" });

    render(<App />);
    const connect = screen.getAllByRole("button", { name: "Connect" })
      .find((button) => button.className.includes("compact"));
    expect(connect).toBeTruthy();
    await user.click(connect as HTMLElement);

    expect(await screen.findByTestId("auth-qr-panel")).toBeInTheDocument();
    expect(screen.getByTestId("qr-code")).toHaveAttribute("data-value", authUrl);
    expect(screen.getByLabelText("Authorization URL")).toHaveValue(authUrl);
    expect(openUrlMock).toHaveBeenCalledWith(authUrl);

    await user.click(screen.getByTestId("open-auth-url"));
    expect(openUrlMock).toHaveBeenCalledTimes(2);

    await user.click(screen.getByTestId("copy-auth-url"));
    expect(await screen.findByTestId("auth-copied")).toBeInTheDocument();
    await waitFor(() => {
      expect(apiMock.pollAuth).toHaveBeenCalled();
    }, { timeout: 5_000 });
    await waitFor(() => {
      expect(screen.queryByTestId("auth-qr-panel")).not.toBeInTheDocument();
    }, { timeout: 5_000 });
    expect(await screen.findByText("Publisher connected")).toBeInTheDocument();
  }, 15_000);
});

describe("social loop surfaces", () => {
  const alice = "pubky1alicepublisherkeyxxxxxxxxxxxxxxxx";
  const bob = "pubky1bobpublisherkeyxxxxxxxxxxxxxxxxxx";
  const aliceRelease = {
    schema: "pubky.swarm/release",
    version: 1,
    id: "release-1",
    publisher: alice,
    created_at: 1,
    title: "Alice Dataset",
    description: "Shared research",
    torrent: {
      info_hash: infoHash,
      size: 42,
      files: [{ path: "data.bin", size: 42 }],
      trackers: [],
    },
    tags: ["research"],
  };

  it("syncs followed contacts and renders claim chips on the feed", async () => {
    apiMock.authStatus.mockResolvedValue({ authenticated: true, user: bob });
    apiMock.followed.mockResolvedValue([alice]);
    apiMock.syncFollowed.mockResolvedValue({
      followed: [alice],
      releases: [aliceRelease],
      claimCount: 1,
    });
    apiMock.listSubjectTags.mockResolvedValue([
      {
        issuer: alice,
        tag: "public-domain",
        subject: `torrent:btih:${infoHash}`,
        infoHash,
        createdAt: 1,
        revision: 1,
      },
    ]);
    const user = userEvent.setup();
    render(<App />);

    await user.click(await screen.findByRole("button", { name: "Discover" }));
    expect(await screen.findByText("Contacts")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Sync now" }));

    await waitFor(() => {
      expect(apiMock.syncFollowed).toHaveBeenCalled();
    });
    expect(await screen.findByText("Alice Dataset")).toBeInTheDocument();
    expect(await screen.findByText(/#public-domain/)).toBeInTheDocument();
    expect(screen.getByText(/#research/)).toBeInTheDocument();
  });

  it("publishes tags from a library torrent with the correct infohash", async () => {
    apiMock.authStatus.mockResolvedValue({ authenticated: true, user: bob });
    apiMock.torrents.mockResolvedValue([
      {
        id: 3,
        infoHash,
        name: "Library seed",
        state: "seeding",
        progressBytes: 42,
        totalBytes: 42,
        uploadedBytes: 1,
        downloadMbps: 0,
        uploadMbps: 0.1,
        peersConnected: 1,
        peersSeen: 2,
        ratio: 1,
        eta: null,
        finished: true,
        error: null,
        files: [{ index: 0, path: "data.bin", length: 42, included: true }],
      },
    ]);
    const user = userEvent.setup();
    render(<App />);

    const tags = await screen.findByLabelText("Public tag claims");
    await user.type(tags, "verified");
    await user.click(screen.getByRole("button", { name: "Publish tags" }));

    await waitFor(() => {
      expect(apiMock.publishCatalogTags).toHaveBeenCalledWith(infoHash, ["verified"]);
    });
  });
});
