# Acceptance testing

Pubky Swarm tests user outcomes at more than one layer. A frontend mock alone is
not accepted as evidence that transfer or publication works.

## Catalog magnet to downloaded bytes

`torrent-engine/tests/local_swarm.rs` creates generated files, builds and seeds a
real torrent, places its infohash in RSS, parses the RSS through
`catalog-client`, adds the resulting magnet to an independent engine, downloads
over loopback, and compares the retrieved bytes with the originals.

```bash
cargo test -p torrent-engine explicit_peer_seed_leech_selective_stream
```

This test also covers selective download, streaming, pause/resume, safe removal,
and observed upload traffic from the sharing engine.

## File and tag sharing through Pubky

The PostgreSQL-backed `pubky-adapter` acceptance test publishes a validated
`ReleaseV1` and `TagClaimV1` under one Pubky identity. A separate adapter then
lists and retrieves both public objects and validates exact equality.

```bash
TEST_PUBKY_CONNECTION_STRING='postgres://USER@127.0.0.1:5432/postgres?pubky-test=true' \
  cargo test -p pubky-adapter real_testnet_crud_listing_and_event_cursor
```

The test uses generated metadata only. It proves public sharing with another
reader identity boundary, not merely insertion into the local SQLite cache.

## Desktop interaction contract

The React acceptance test searches an external catalog, clicks **Add magnet**,
verifies the exact magnet reaches Library, submits it through the Tauri API
boundary, and exercises tag publication. It also covers encoded and literal
BTIH query values.

```bash
cd apps/desktop
npm test
```

These tests mock the Tauri invocation boundary; the Rust tests above exercise
the real parser, torrent engine, network transfer, Pubky homeserver, and
persistence paths behind that boundary.

## Full gate

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cd apps/desktop
npm test
npm run build
```
