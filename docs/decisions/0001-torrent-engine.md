# ADR 0001: Torrent Engine and Peer Discovery

## Status

Accepted for the v1 experiment.

## Decision

Use `librqbit 8.1.1` for v1 torrent creation, peer-wire transfer, selection,
seeding, persistence, and seekable streaming. Disable its built-in DHT and use
`mainline 8.0.0` through `mainline-discovery` for peer announce and lookup.

Keep the transfer engine behind `torrent-engine`; do not expose librqbit handles
to application code.

## Evidence

The local acceptance suite proves:

- deterministic v1 metainfo creation;
- two-stage magnet metadata resolution before storage;
- defensive metainfo/path validation;
- explicit-peer and Mainline-discovered transfer;
- selective file retrieval with measured v1 piece-boundary spill;
- seek/read streaming;
- pause, resume, forget, upload, and JSON fast-resume persistence.

The combined Mainline test uses a five-node isolated DHT, announces the generated
infohash and seeder port, resolves peers from another node, and transfers bytes
through librqbit. No public content or external bootstrap node is used.

A separate live-DHT probe reached public bootstrap infrastructure but did not
rediscover the same-host seeder within 180 seconds. This is consistent with NAT
hairpin/routability limits and is not used as proof of engine correctness.

## Why split discovery from transfer

librqbit 8.1.1 does not expose bootstrap addresses or accept an injected DHT in
`SessionOptions`, so its built-in DHT cannot be isolated in tests. Its persisted
DHT also restores a UDP listen address, causing concurrent sessions to contend.
`mainline` exposes both local testnets and public DHT configuration and produces
peer addresses that librqbit accepts through `initial_peers`.

The split makes discovery testable and keeps the stack aligned with Pubky's
existing Mainline implementation.

## Limits

- V0 emits v1 `btih` torrents only.
- Small files can share pieces; selecting one file may transfer adjacent bytes.
- BEP 9 allocates metadata inside librqbit before the adapter can enforce its
  lower configured cap; librqbit itself caps this allocation at 32 MiB.
- Public availability still requires a routable seeder, NAT traversal, tracker,
  or explicit peer.

## Revisit trigger

Evaluate libtorrent 2.0.x behind the same adapter if v2/hybrid torrents, web
seeds, piece deadlines, or production NAT behavior become mandatory.
