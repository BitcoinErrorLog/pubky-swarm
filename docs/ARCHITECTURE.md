# Architecture

Pubky Swarm keeps authority, discovery, and availability separate.

```text
Pubky identity
├── PKARR _pubky record ──> Homeserver metadata baseline
└── BEP 46 salted item ───> current dataset torrent

Mainline DHT ──> payload/dataset torrent peers
BitTorrent ────> verified pieces and seekable files
Manifest ──────> logical path, size, and BLAKE3 object digest
Application ───> validated profile and release objects
```

## Components

- `swarm-protocol`: release and dataset manifest wire types.
- `swarm-head`: BEP 46 publication, CAS, rollback checks, and reannouncement.
- `mainline-discovery`: peer announce/lookup independent from the transfer engine.
- `torrent-engine`: constrained v1 librqbit adapter.
- `dataset-core`: transport-neutral reader/publisher/watcher contracts.
- `dataset-homeserver`: control backend using immutable snapshot paths and a
  mutable Homeserver pointer.
- `dataset-torrent`: BEP 46-authorized snapshot torrents.
- `stream-gateway`: capability-protected loopback HTTP Range playback.
- `swarm-store`: migrated local cache; no root keys or grant secrets.
- `apps/desktop`: Tauri/React publisher and reader.
- `services/discovery`: optional non-authoritative validated search cache.
- `services/seeder`: bounded dataset pinning and BEP 46 reannouncement.

## Publication order

1. Validate logical objects and build the canonical manifest.
2. Materialize an immutable snapshot directory.
3. Create and verify the v1 dataset torrent.
4. Seed and announce the torrent.
5. Publish the signed BEP 46 head with CAS.

A failure before step 5 leaves the previous authority unchanged. It may leave
unreferenced immutable bytes, which can be garbage-collected later without
affecting correctness.

## Read validation

1. Resolve and verify the signed BEP 46 item.
2. Resolve peers from Mainline.
3. Resolve and structurally validate torrent metainfo.
4. Fetch and parse the canonical manifest.
5. Verify publisher identity and manifest digest.
6. Fetch requested object pieces.
7. Verify object length and BLAKE3 digest.
8. Deserialize the application schema.

Homeserver and torrent readers expose the same dataset interface and must not
skip manifest/object verification.
