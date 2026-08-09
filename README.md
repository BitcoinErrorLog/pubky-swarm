<p align="center">
  <img src="apps/desktop/app-icon.png" width="112" alt="Pubky Swarm icon">
</p>

<h1 align="center">Pubky Swarm</h1>

<p align="center">
  A Pubky-native desktop torrent client and an experiment in
  origin-independent public datasets.
</p>

<p align="center">
  <a href="https://github.com/BitcoinErrorLog/pubky-swarm/actions/workflows/ci.yml">
    <img src="https://github.com/BitcoinErrorLog/pubky-swarm/actions/workflows/ci.yml/badge.svg" alt="CI">
  </a>
  <a href="LICENSE">
    <img src="https://img.shields.io/badge/license-MIT-f03b20" alt="MIT license">
  </a>
</p>

> **Status:** working alpha. The desktop client, transfer path, catalog
> integrations, Pubky release publication, and public tag claims are
> implemented and tested. See [Current limits](#current-limits) before using it
> for important data.

## What is Pubky Swarm?

A publisher should be able to expose a changing public dataset without keeping
one canonical origin server online forever. Pubky Swarm tests that proposition
with torrents:

- **Pubky identifies the publisher and authorizes public metadata.**
- **BEP 44/BEP 46 identifies the current dataset snapshot.**
- **BitTorrent peers provide the bytes.**
- **Optional catalogs help find candidates but never become authorities.**

These roles stay separate. A search index can suggest a torrent; it cannot
silently become the publisher or redefine authenticated data.

## What works

### Desktop torrent client

- Add arbitrary magnet links and `.torrent` files.
- Register the installed app as a `magnet:` handler.
- Choose files before or after adding a multi-file torrent.
- Pause, resume, remove, and inspect transfers.
- View download/upload rates, peers, ETA, progress, and ratio.
- Stream supported media through a capability-protected loopback HTTP gateway.
- Hand magnets to another client or connect to qBittorrent's WebUI.
- Import completed qBittorrent payloads without duplicating their data layout.

### Pubky publishing and social metadata

- Authorize through a capability-scoped Pubky grant; the root key stays on the
  signing device.
- Publish validated `ReleaseV1` records under a Pubky identity.
- Resolve publishers, follow them, and retain validated releases offline.
- Publish issuer-attributed `TagClaimV1` records without rewriting a release's
  publisher tags.
- Let another Pubky reader list and retrieve public release and tag records.

### Discovery without surrendering authority

- Search the optional Pubky discovery cache.
- Search persistent, user-approved RSS and Torznab sources.
- Start with the official Academic Torrents recent RSS feed.
- Connect local Jackett or Prowlarr endpoints with session-only API keys.
- Preserve RSS/Torznab results as explicit, non-authoritative hints.
- Require confirmation before a catalog result starts downloading.

### Dataset research stack

- Publish and resolve BEP 46 mutable dataset heads.
- Detect stale sequences, rollback attempts, and concurrent updates.
- Build deterministic manifests with BLAKE3 object digests.
- Read the same authenticated dataset through Homeserver and torrent backends.
- Pin and reannounce bounded datasets through the optional seeder service.
- Prove that a fresh reader can retrieve a publisher's authenticated snapshot
  from a separate seeder after the publisher shuts down.

## How a release moves through the system

```text
Publisher
  │
  ├─ Pubky grant ──> public release + tag records
  │
  ├─ BEP 46 ───────> signed pointer to the current dataset torrent
  │
  └─ BitTorrent ───> verified payload pieces
                         │
Catalogs ──> hints only  │
                         ▼
Reader ──> validates identity, metadata, torrent, manifest, and object bytes
```

Publication writes immutable data before advancing mutable authority. A failed
upload cannot silently replace the previous valid dataset head.

## Quick start

### Prerequisites

- Rust `1.93`
- Node.js `24` with npm
- Platform dependencies required by
  [Tauri](https://v2.tauri.app/start/prerequisites/) and libtorrent

Clone and run:

```bash
git clone https://github.com/BitcoinErrorLog/pubky-swarm.git
cd pubky-swarm

cargo test --workspace --exclude pubky-adapter --exclude dataset-homeserver

cd apps/desktop
npm ci
npm test
npm run tauri dev
```

On the project's external macOS development volume, mount the native build cache
before running Cargo:

```bash
./scripts/mount-build-volume.sh
```

This workaround is only needed when the underlying filesystem cannot support
Cargo's normal hard-link and incremental-cache behavior.

## First run

1. Open **Library** to paste a magnet or select a `.torrent` file.
2. Open **Discover** and search the enabled Academic Torrents feed.
3. Select **Add magnet**, review the populated Library form, then confirm.
4. Optionally add a local Torznab endpoint from Jackett or Prowlarr.
5. Authorize with Pubky Ring to publish releases or public tag claims.

External catalog descriptions and categories are untrusted hints. Torrent hashes
prove byte consistency, not authorship, legality, or safety.

## Acceptance evidence

The test suite checks outcomes across the real boundaries:

- RSS result → generated magnet → independent torrent engine → downloaded bytes
  exactly match the shared files.
- Seeder upload traffic is observed during the transfer.
- A publisher writes a validated release and tag claim to a real local Pubky
  testnet; an independent reader lists and retrieves both.
- The React interaction test clicks **Add magnet**, verifies the exact value
  reaches Library, and submits it through the Tauri command boundary.
- Literal and percent-encoded BTIH magnet targets are accepted.

Run the complete gate:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace

cd apps/desktop
npm test
npm run build
```

PostgreSQL-backed Pubky integration tests use
`TEST_PUBKY_CONNECTION_STRING`. See [Acceptance testing](docs/TESTING.md) for
the exact commands and what each layer proves.

## Trust and safety

- Root keys and recovery material are never imported by the desktop app.
- Torznab API keys remain in memory and are cleared when the app exits.
- External XML is size/depth bounded; DTDs, unknown entities, redirect chains,
  credential-bearing URLs, and Torznab error documents are rejected.
- Stream URLs bind only to loopback and contain a random 256-bit capability.
- Imported filesystem paths and torrent metadata are validated before use.
- Removing a transfer does not delete payload files until ownership provenance
  can be proven safely.

Read the complete [security model](docs/SECURITY.md) before exposing services or
adding third-party catalogs.

## Project map

```text
apps/desktop/              Tauri + React desktop application
crates/catalog-client/     Bounded RSS and Torznab client
crates/swarm-protocol/     Releases, manifests, tags, collections, moderation
crates/torrent-engine/     Constrained librqbit transfer engine
crates/libtorrent-engine/  Narrow libtorrent 2.0 bridge and parity spike
crates/swarm-head/         BEP 46 signing, publication, CAS, rollback checks
crates/swarm-store/        Migrated SQLite cache and source configuration
crates/dataset-*/          Transport-neutral, Homeserver, and torrent datasets
crates/stream-gateway/     Capability-protected seekable playback
services/discovery/        Optional RSS, Torznab, OpenSearch, and JSON index
services/seeder/           Bounded pinning and BEP 46 reannouncement
```

## Current limits

- The production transfer engine emits BitTorrent v1 `btih` torrents. The
  libtorrent bridge is implemented as a constrained migration/parity spike, not
  yet the default engine.
- Web seeds are not supported by the current librqbit backend.
- Desktop and seeder sessions enable Mainline DHT by default so completed
  torrents can announce and discover peers without relying only on trackers.
- Production dataset-head signing still needs a trusted live-signing flow from
  Pubky Ring. Current BEP 46 proofs use isolated test identities.
- Search availability does not imply payload availability. If every peer holding
  the pieces disappears, signed metadata remains valid but the bytes are gone.
- External catalogs are opt-in hints. Pubky Swarm does not bundle piracy-site
  scrapers or treat third-party listings as publisher claims.
- Production signing, notarization, and release publication require maintainer
  credentials documented in [Releasing](docs/RELEASING.md).

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Security model](docs/SECURITY.md)
- [Acceptance testing](docs/TESTING.md)
- [Demo guide](docs/DEMO.md)
- [Benchmarks](docs/BENCHMARKS.md)
- [Release process](docs/RELEASING.md)
- [Architecture decisions](docs/decisions/)

## License

[MIT](LICENSE)
