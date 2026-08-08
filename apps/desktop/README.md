# Pubky Swarm Desktop

Tauri/React interface for Pubky authorization, release publication, direct
publisher discovery, torrent download progress, and seekable media playback.
Installed desktop bundles register as a `magnet:` URL handler. Opening a magnet
focuses the existing app and fills the import form; it does not begin a download
until the user confirms it.

```bash
npm install
npm run build
npm run tauri dev
```

Private keys, Pubky credentials, filesystem access, torrent networking, and
stream capabilities stay in the Rust backend. The webview receives only typed
commands and public data.
