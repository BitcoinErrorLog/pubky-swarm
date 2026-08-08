# Pubky Swarm

Experimental desktop and protocol stack for publishing, discovering, downloading,
and validating torrents through Pubky identities.

The project separates:

- publisher identity and authority (Pubky),
- mutable torrent discovery (BEP 44/BEP 46 over Mainline DHT),
- byte availability (BitTorrent peers and seeders), and
- optional search/index services, which are never authoritative.

## Implemented stack

- Tauri desktop publisher, direct/follow discovery, download progress, and
  seekable media playback.
- Pubky 0.10 grant-authenticated release writes, public reads, listings, and
  event cursors.
- Mainline 8 peer discovery feeding a constrained librqbit 8.1.1 transfer
  engine.
- BEP 46 mutable dataset heads signed by the same Ed25519 identity as Pubky.
- Deterministic manifests and interchangeable Homeserver/torrent dataset
  adapters.
- Migrated offline cache, optional non-authoritative search service, bounded
  seeder, and machine-readable benchmark harness.
- Persistent opt-in RSS/Torznab catalogs with an Academic Torrents starter
  feed, session-only indexer credentials, and Pubky-attributed tag claims.
- Automated publisher-shutdown proof: a fresh client retrieves authenticated
  metadata from a separate seeder.

V0 intentionally emits v1 `btih` torrents. Production dataset-head signing
remains blocked on a safe live-signing flow from Pubky Ring; the working proof
uses isolated test identities.

## Development

On the external macOS workspace, mount the native build cache first:

```bash
./scripts/mount-build-volume.sh
```

Then verify the workspace and start the desktop application:

```bash
cargo test --workspace
cd apps/desktop
npm install
npm run tauri dev
```

The real Pubky testnet integration requires PostgreSQL and uses
`TEST_PUBKY_CONNECTION_STRING` when provided.

See:

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)
- [`docs/SECURITY.md`](docs/SECURITY.md)
- [`docs/BENCHMARKS.md`](docs/BENCHMARKS.md)
- [`docs/DEMO.md`](docs/DEMO.md)

## Safety

Private keys and session credentials remain in Rust backend processes. Never add
recovery files, session secrets, API keys, downloaded payloads, or local data
directories to version control.

## License

MIT
