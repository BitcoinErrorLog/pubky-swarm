# ADR 0004: Owned libtorrent 2.0.13 Lifecycle Bridge

## Status

Accepted as an isolated lifecycle bridge. This does not replace the engine
selected by ADR 0001.

## Source pin

The crate builds the official `arvidn/libtorrent` `v2.0.13` tag:

- tag commit: `7d7fc38fac61177fa5e02148f791b2f65250b09d`;
- tag commit tree: `c6989134e6bdc796ef85d1c93afa8029f9cd0501`;
- official `libtorrent-rasterbar-2.0.13.tar.gz` release asset SHA-256:
  `892cb75c06318e2420de0faf9f63a908069d3d237676e2459fd30abe0cb3b1bf`;
- embedded upstream revision reported by the release header: `4a0dcf5cf`;
- runtime version: `2.0.13.0`.

The verified source is committed under `vendor/libtorrent`. To reproduce it,
run:

```sh
bash crates/libtorrent-engine/scripts/fetch-libtorrent.sh
```

The script downloads by immutable commit, verifies the archive before replacing
the vendor directory, and verifies the version header after extraction.

## Bridge boundary

`libtorrent-engine` uses `cxx` and `cxx-build` 1.0.199 with a project-owned
facade. The public Rust crate forbids unsafe code and exposes only owned Rust
strings, vectors, records, and an owning `Session`. No libtorrent, Boost, or STL
container type appears in the public Rust API. The private generated CXX layer
uses `UniquePtr` solely to own the project-defined opaque session handle.

Every callable facade entry point is `noexcept`, catches `std::exception` and
unknown exceptions, and copies failures into an explicit Rust error. Calling
`Session::close` performs abort, worker synchronization, and destruction with a
reported result. Dropping without `close` remains safe; the `noexcept` native
destructor catches failures because Rust `Drop` has no error channel.

The bridge provides:

- runtime version, embedded revision, ABI, and compile switches from the linked
  native library;
- libtorrent parsing of official v1, v2, and hybrid magnet vectors into owned
  hash, name, and tracker values;
- construction of a loopback-only session with explicit discovery, port
  mapping, TCP, and uTP switches;
- owned session status and drained alert snapshots;
- synchronous magnet, validated `.torrent`, and trusted fast-resume additions
  with an explicit UTF-8 save path, initial flags, priorities, and rate limits;
- facade-owned monotonically increasing torrent IDs, with owned v1/v2 hashes,
  names, status, counters, current flags, limits, errors, and file snapshots;
- pause, resume, removal without payload deletion, file priority/selection,
  force recheck, force reannounce, and per-torrent rate-limit commands;
- session-global upload and download limits through `settings_pack`;
- alert-driven fast-resume serialization, explicit polling, and restoration
  from owned bencoded bytes with the caller's save path and flags overriding
  the persisted values;
- explicit session shutdown and native worker synchronization.

## Lifecycle parity and synchronization

The first lifecycle slice supports the following libtorrent 2.0.13 behavior
without exposing a `torrent_handle`:

- `session::add_torrent` is used synchronously for magnets, v1, v2, hybrid
  metainfo, and restored resume data. Duplicate handling follows the explicit
  `duplicate_is_error` flag. IDs are stable only within one `Session` and are
  never reused by that session.
- Pause, resume, file-priority changes, force recheck, force reannounce, and
  global settings are native asynchronous commands. Successful return means
  libtorrent accepted the command, not that disk or tracker work completed.
  Callers poll owned snapshots and drained owned alerts. In particular,
  `torrent_paused`, `torrent_resumed`, `file_prio`, `torrent_checked`, and
  tracker alerts expose native completion where libtorrent emits one.
- Removal passes no `remove_flags_t`; neither `delete_files` nor
  `delete_partfile` can be selected through this API. The torrent disappears
  from active snapshots synchronously, while `torrent_removed` remains
  observable as an owned alert.
- Resume serialization calls asynchronous `save_resume_data(save_info_dict)`.
  A facade request token is polled until the corresponding
  `save_resume_data_alert` is converted with `write_resume_data_buf`, or the
  corresponding failure alert is returned as an error. A terminal poll
  consumes the request.
- Per-torrent limits use `torrent_handle` and report `None` for libtorrent's
  `-1` unlimited value. Global limits use `settings_pack`, translating
  libtorrent's global `0` unlimited value to the same Rust `None`.
- File priorities are the native inclusive `0..=7` values. Selection is the
  exact convenience mapping `false -> 0`, `true -> 4`. Priority application is
  asynchronous and is observed through file snapshots or `file_prio_alert`.
- Reannounce requests all configured trackers immediately with
  `ignore_min_interval | high_priority`. There is no fabricated completion:
  tracker announce/reply/error alerts are the only network outcome.
- Resume completions are processed before generic alert retention. Generic
  owned alerts are bounded to the newest 4,096 entries to prevent an
  unobserved session from growing memory without limit.

The session remains loopback-only and disables DHT, LSD, UPnP, NAT-PMP,
incoming/outgoing TCP and uTP, and per-torrent PEX. Tests therefore exercise
real native lifecycle and disk-thread behavior without contacting public
swarms.

## Boundary and input limits

The CXX calls transfer owned Rust strings, byte vectors, and shared value
records. No libtorrent, Boost, STL, native handle, pointer, span, string view,
or borrowed output crosses into public Rust. The public crate continues to
forbid unsafe code.

Rust validates inputs before copying them to C++, and the C++ facade repeats
the security-critical checks:

- magnet URI: 32 KiB;
- `.torrent` metainfo: 8 MiB;
- fast-resume input/output: 16 MiB;
- UTF-8 save path: 4 KiB;
- file and priority count: 100,000;
- metainfo decode: 1,048,576 pieces, depth 100, and 2,097,152 tokens;
- file priorities: `0..=7`;
- rate limits: positive `i32` bytes/second or unlimited.

Every CXX entry point is `noexcept` and catches both `std::exception` and
unknown exceptions. Metainfo uses `load_torrent_buffer` with explicit
`load_torrent_limits`. Resume data is security-sensitive trusted input;
restoration always overrides its save path and lifecycle flags, but persisted
trackers and web seeds remain part of libtorrent's resume model.

## Deliberately unsupported parity

This slice does not expose peer lists or peer connection commands, piece
priorities/deadlines, tracker or web-seed mutation, DHT/LSD/PEX control, port
mapping, proxy/I2P settings, IP filters, queue positioning, storage moves,
file renames, piece reads, torrent creation, session-state persistence,
payload deletion, plugin installation, encryption settings, or mutable
torrents. It also does not claim tracker completion for force reannounce or
disk completion for commands where libtorrent provides only asynchronous
state/alerts.

Native tests generate structurally legal local v1, v2, and hybrid fixtures,
exercise add/snapshot/control/save/restore/remove behavior, verify payload
removal is not requested, reject malformed and oversized inputs, and use no
tracker URLs or public swarm connectivity.

## Native build and link

Cargo always configures the vendored source directly with CMake. It never calls
`find_package(libtorrent)` and cannot silently select a system libtorrent 2.1.
The native configuration is:

- static `libtorrent-rasterbar`;
- release mode, position-independent code, ABI 3, deprecated API disabled;
- DHT, encryption, extensions, mutable torrents, streaming, and C++ exceptions
  enabled;
- I2P and libtorrent logging disabled;
- tests, examples, tools, and Python bindings disabled;
- OpenSSL enabled; Boost is used through its headers.

The project facade is compiled as C++17 with the same ABI-relevant preprocessor
definitions. Libtorrent exceptions remain enabled internally but do not cross
the facade.

On the verified Apple Silicon macOS setup, CMake and Boost are found through
Homebrew, OpenSSL is found at `brew --prefix openssl@3`, and the link consists
of the vendored static `libtorrent-rasterbar` archive plus dynamic Homebrew
OpenSSL 3 and the macOS CoreFoundation and SystemConfiguration frameworks. This
is not a fully static executable and is not a universal binary.

## Other platform requirements

Linux requires CMake 3.20 or newer, a C++17 compiler, Boost development headers,
OpenSSL 3 development headers and libraries, and a native build backend
supported by CMake. Set `BOOST_ROOT`, `OPENSSL_ROOT_DIR`, or `OPENSSL_LIB_DIR`
when they are outside standard search paths. The output statically embeds
libtorrent but ordinarily links the platform C++ runtime, pthreads, OpenSSL, and
system libraries dynamically.

Windows requires 64-bit Visual Studio 2022 C++ build tools, CMake, Boost, and
OpenSSL 3 built for the same target and MSVC runtime. Set `BOOST_ROOT`,
`OPENSSL_ROOT_DIR`, and `OPENSSL_LIB_DIR`; a vcpkg installation may be selected
with `CMAKE_TOOLCHAIN_FILE`. The script links the required Windows networking
and cryptography system libraries. Static MSVC runtime or fully static OpenSSL
is not selected by default.

Cross-compilation is limited by the native Boost and OpenSSL installations.
Each Cargo target needs matching native dependencies and a CMake toolchain.
