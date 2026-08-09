# Pubky Swarm Desktop

Tauri/React interface for Pubky authorization, release publication, direct
publisher discovery, torrent download progress, and seekable media playback.
Installed desktop bundles register as a `magnet:` URL handler. Opening a magnet
focuses the existing app and fills the import form; it does not begin a download
until the user confirms it.

The Discover view persists user-approved RSS and Torznab endpoints. Academic
Torrents ships curated RSS presets (Recent enabled by default; other collections
opt-in). Paste any HTTPS RSS feed URL to add more. Torznab API keys remain in
memory for the current process and must be entered again after restart.
External results are hints: adding a result only fills the magnet import form.
Authenticated users can publish their own tags under the scoped Pubky
tag-claims namespace.

The optional Pubky discovery service is queried only when
`PUBKY_SWARM_DISCOVERY_URL` is set. Leaving it unset uses the built-in and
user-configured external catalogs without producing a localhost connection
error.

```bash
npm install
npm run build
npm run tauri dev
```

Private keys, Pubky credentials, filesystem access, torrent networking, and
stream capabilities stay in the Rust backend. The webview receives only typed
commands and public data.
