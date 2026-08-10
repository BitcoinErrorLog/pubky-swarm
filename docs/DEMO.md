# Demonstration

## Desktop application

```bash
cd apps/desktop
npm install
npm run tauri dev
```

1. Connect from the sidebar identity card. Scan the QR (or copy the
   `pubkyauth://` URL) in Pubky Ring and approve the release + tag-claim grant.
2. Enter an absolute local file or directory path on Publish.
3. Create and publish the release. Discover opens on the contact feed preview.
4. On another client, follow the publisher Pubky, then **Sync now**.
5. Load profile/releases, resolve payload peers through Mainline, and download.
6. Play supported media through the seekable loopback Range server.

## Two-human Ring smoke (required for phone approval)

Use two machines (or two Torky builds) and two Ring identities.

1. **Alice** opens Torky → sidebar **Connect** → Ring scans QR / pastes URL →
   approve. Confirm sidebar shows connected and “Session ends when you quit”.
2. **Alice** publishes a release with publisher tags, then tags the Library
   torrent with a public claim (e.g. `public-domain`).
3. **Bob** connects the same way, pastes Alice’s Pubky, **Follow publisher**,
   **Sync now**. Confirm Alice’s release appears under Contacts and Alice’s
   claim chip shows on the card.
4. **Bob** publishes a claim on Alice’s infohash from the release card.
5. **Alice** clicks **Sync now** and confirms Bob’s claim chip on the same
   torrent/release without re-pasting keys.
6. Sign out on either side; restarting the app requires a new Ring approval
   (session is process-lifetime; grant secrets are not stored in SQLite).
   Pending grant flows are also not restored across relaunch for the same
   reason—restart Connect if the app quits mid-approval.

Phone Ring is only required for this checklist. CI uses two testnet identities
instead (see `docs/TESTING.md`).

## External catalogs and tagging

1. Open Discover. Curated Academic Torrents feeds are listed; Recent is enabled
   by default. Toggle other collections or paste an HTTPS RSS feed URL.
2. Optionally open advanced Torznab and connect a local Jackett/Prowlarr
   endpoint. Enter its API key separately; it is held only for the current app
   session.
3. Search enabled catalogs. Cached claim matches for the query appear when
   contacts’ tags are already synced. One unavailable source does not discard
   successful results from other sources.
4. Select **Add magnet**. Confirm the populated magnet in Library to begin the
   transfer.
5. Authorize Pubky and enter normalized tags on a result with a v1 infohash.
   **Publish tags** writes issuer-attributed tag claims without treating source
   categories as authenticated facts. Publisher `ReleaseV1.tags` remain separate
   metadata chips.

## Origin-independent dataset proof

The automated proof uses only generated local content:

```bash
cargo test -p dataset-torrent fresh_reader_survives_publisher_shutdown_via_seeder -- --nocapture
```

It performs:

1. Publisher builds and seeds an authenticated metadata snapshot.
2. Publisher signs the current torrent through BEP 46.
3. Seeder resolves, downloads, verifies, pins, announces, and reannounces.
4. Publisher torrent engine shuts down.
5. A fresh reader resolves the same Pubky head.
6. The fresh reader obtains and validates release metadata from the seeder.

## Failure boundary

Stop every engine that holds the generated snapshot and start a fresh reader.
The signed head remains meaningful, but retrieval fails because no peer has the
bytes. This is the expected distinction between authenticated authority and
availability.

## Optional discovery service

```bash
cargo run -p discovery
curl -X POST \
  http://127.0.0.1:7780/v1/publishers/PUBKY/refresh
curl 'http://127.0.0.1:7780/v1/search?q=open'
```

Results explicitly require client validation and never replace publisher
authority.

## Optional bounded seeder

```bash
PUBKY_SWARM_PUBLISHERS=PUBKY1,PUBKY2 \
PUBKY_SWARM_MAX_DISK_BYTES=21474836480 \
cargo run -p seeder
```

The service pins current snapshots, refreshes signed BEP 46 items without root
keys, announces dataset torrents, and stops adding data at the disk budget.
