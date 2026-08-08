# Demonstration

## Desktop application

```bash
cd apps/desktop
npm install
npm run tauri dev
```

1. Authorize the release namespace with Pubky Ring.
2. Enter an absolute local file or directory path.
3. Create and publish the release.
4. On another client, enter the publisher Pubky.
5. Load its profile and validated release records.
6. Resolve payload peers through Mainline and download.
7. Play supported media through the seekable loopback Range server.

## External catalogs and tagging

1. Open Discover. The built-in Academic Torrents recent feed is already
   enabled.
2. Optionally add a credential-free RSS endpoint, or a Torznab endpoint from a
   local Jackett/Prowlarr instance. Enter its API key separately; it is held only
   for the current app session.
3. Search enabled catalogs. One unavailable source does not discard successful
   results from other sources.
4. Select **Add magnet**. Confirm the populated magnet in Library to begin the
   transfer.
5. Authorize Pubky and enter normalized tags on a result with a v1 infohash.
   **Publish tags** writes issuer-attributed tag claims without treating source
   categories as authenticated facts.

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
