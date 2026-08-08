# Benchmarks

The benchmark harness emits versioned JSON to standard output.

```bash
cargo run -p benchmark > report.json
```

Default matrix:

- object counts: `1,10,100`
- object sizes: `500,2048,10240`

Override with bounded comma-separated values:

```bash
PUBKY_SWARM_BENCH_COUNTS=1,10,100,1000 \
PUBKY_SWARM_BENCH_SIZES=500,2048,10240,102400 \
cargo run --release -p benchmark > report.json
```

Per case the report records:

- dataset bytes;
- canonical manifest construction time and bytes;
- manifest digest;
- v1 torrent construction time, piece length, and metainfo bytes; and
- one-object mutation manifest/torrent regeneration costs.

Safety limits prevent more than 100,000 objects, objects above 1 MiB in the
configured matrix, or a single case above 2 GiB.

Transport acceptance is covered by integration tests:

```bash
cargo test -p torrent-engine --test local_swarm
cargo test -p dataset-homeserver
cargo test -p dataset-torrent
```

These measure and verify selective piece spill, persistence, Mainline-discovered
transfer, real Pubky Homeserver CRUD/events, BEP 46 publication, seeder pinning,
publisher shutdown, and fresh-client retrieval.
