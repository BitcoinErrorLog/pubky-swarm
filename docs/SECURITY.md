# Security Model

## Trust boundaries

- The Pubky root key is authoritative for PKARR and BEP 46.
- Grant/PoP sessions authorize only their declared Homeserver capability.
- Homeservers, peers, seeders, discovery services, trackers, and cached bytes
  are untrusted.
- Search results are candidates. Clients verify publisher, head, torrent,
  manifest, object digest, and schema before rendering.

## Key custody

The desktop application never imports a user's primary root key. Normal
publication uses capabilities scoped to `/pub/pubky.swarm/v1/releases/` and
`/pub/pubky.swarm/v1/tag-claims/`.

Dataset-head publication is currently lab-only with an isolated test identity.
Production publication remains blocked until a trusted signer such as Pubky Ring
can approve exact BEP 46 bytes. Pubky Noise may transport a future signing
request but does not itself provide delegation.

## Rollback and concurrency

- BEP 44 sequence and CAS order dataset heads.
- Clients persist the highest accepted authority sequence.
- Lower sequences are rejected; equal sequence with a different manifest is a
  conflict.
- First-contact freshness cannot be proven globally.
- Homeserver control-backend publication is serialized in-process but Pubky
  0.10 does not expose HTTP conditional writes, so it is not the root-authority
  path.

## Network input limits

The torrent adapter rejects:

- metainfo above 4 MiB by default;
- more than 65,536 files;
- more than 1 TiB declared content;
- unsafe, non-UTF-8, reserved, duplicate, or file/directory-colliding paths;
- excessive path depth/component/path bytes;
- symlink entries; and
- empty or malformed torrents.

librqbit allocates BEP 9 metadata before the adapter's lower cap and applies its
own 32 MiB ceiling. This remains an upstream limitation.

Dataset manifests allow at most 100,000 objects. In-memory dataset object reads
are capped at 100 MiB and manifest reads at 16 MiB.

External catalog responses are capped at 4 MiB and 100 actionable entries.
Remote sources require HTTPS; loopback HTTP is allowed for local Prowlarr or
Jackett. URL credentials, fragments, sensitive credential query parameters,
explicit non-loopback IP literals, and HTTP redirects are rejected. At most four
sources are fetched concurrently. XML document types, unknown entities, nesting
beyond 32 elements, and Torznab error documents are rejected. Request failures
remove request URLs before they reach the UI so Torznab API keys are not
disclosed in errors.

## Playback

Media is exposed only on an operating-system-assigned loopback port. Every URL
contains a random 256-bit capability. The server compares a digest of the token,
supports one byte range per request, validates torrent/file IDs, and never binds
to a non-loopback interface.

## Tracker and URL policy

Release schemas accept only credential-free HTTP, HTTPS, or UDP tracker URLs
without fragments. Pubky release downloads use Mainline peer discovery and do
not require publisher tracker hints. Explicit user imports of arbitrary magnets
or `.torrent` files preserve and contact their tracker tiers as normal client
behavior; remote tracker URLs should therefore be treated as user-approved
network destinations. Web seeds remain unsupported in the librqbit backend.

## Local persistence

SQLite stores followed publishers, validated public release records, event
cursors, and credential-free external catalog configuration. It does not store
root keys, recovery material, grant credentials, or Torznab API keys. Indexer
keys remain in memory and are cleared when the app exits. Torrent persistence
contains public metainfo and resume state.

External feed categories remain untrusted metadata. The app never downloads a
catalog result until the user confirms the populated magnet form. Publishing a
tag creates a separate issuer-attributed Pubky claim; it does not rewrite the
source release or imply that Torky verified the source's description.

## Availability is not permanence

If every publisher, seeder, and client holding pieces goes offline, the bytes
are unavailable. Signed metadata proves authority and integrity, not permanent
storage.
