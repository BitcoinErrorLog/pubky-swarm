// @vitest-environment jsdom

import "@testing-library/jest-dom/vitest";
import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App, { isMagnet } from "./App";

const apiMock = vi.hoisted(() => ({
  authStatus: vi.fn(),
  takePendingMagnet: vi.fn(),
  followed: vi.fn(),
  qbittorrentStatus: vi.fn(),
  torrents: vi.fn(),
  externalCatalogSources: vi.fn(),
  searchCatalog: vi.fn(),
  searchExternalCatalogs: vi.fn(),
  importMagnet: vi.fn(),
  publishCatalogTags: vi.fn(),
}));

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
  openUrl: vi.fn().mockResolvedValue(undefined),
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
  apiMock.authStatus.mockResolvedValue({ authenticated: false, user: null });
  apiMock.takePendingMagnet.mockResolvedValue(null);
  apiMock.followed.mockResolvedValue([]);
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
  eventMock.callbacks.length = 0;
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
