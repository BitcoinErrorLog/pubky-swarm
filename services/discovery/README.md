# Torky discovery service

This optional service indexes only publishers explicitly refreshed through
`POST /v1/publishers/{publisher}/refresh`. It searches the validated
`swarm-store` release cache; it does not crawl the DHT and is not an authority
for publisher state. Clients must validate results against the publisher.

Set `PUBKY_SWARM_DISCOVERY_PUBLIC_URL` to the credential-free HTTP(S) origin
used in feed and details links. Without it, the service derives an origin from
the request `Host` header and an optional `X-Forwarded-Proto` value of `http`
or `https`.

## Endpoints

- `GET /health`
- `POST /v1/publishers/{publisher}/refresh`
- `GET /v1/publishers/{publisher}/releases`
- `GET /v1/publishers/{publisher}/releases/{release}`
- `GET /v1/search?q={query}&limit={1..100}`
- `GET /v1/publishers/{publisher}/releases.rss?limit={1..100}`
- `GET /v1/search.rss?q={query}&limit={1..100}`
- `GET /api?t=caps`
- `GET /api?t=search&q={query}&cat=8000&tag={tag}&limit={1..100}&offset={offset}`
- `GET /torznab/api` (alias for `/api`)
- `GET /v1/torznab/caps`
- `GET /v1/torznab/search`
- `GET /opensearch.xml`
- `GET /plugins/pubky_swarm.py`

RSS enclosures use magnet URLs with the v1 infohash, display name, tracker
hints, MIME type `application/x-bittorrent`, and payload length. Torznab
results include category 8000, tags, infohash, magnet, size, publisher,
release ID, and a native details link.

The current released `swarm-store` API exposes validated release records but
does not expose seed or peer observations. Compatibility responses therefore
omit Torznab seed/peer attributes instead of reporting invented zeroes. The
Python plugin reports qBittorrent's unknown value (`-1`) for those fields.
When a future released store API provides observations, this service can map
them without changing the feed contracts.

Install the served Python artifact in qBittorrent's search plugins. The
checked-in `pubky_swarm.py` defaults to `http://127.0.0.1:7780`; the served
copy is generated with the service's current public origin.
