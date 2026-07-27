# io-workbench Mobile

This folder contains the first remote mobile client package as a static PWA.

The mobile client does not run agent CLIs locally. It connects to a running
`io-workbench` server over HTTP(S) and WS(S), stores the server URL and bearer
token in browser storage, and shows remote projects plus live WebSocket status.

Open `www/index.html` directly for local testing, or package the `www/` folder
with any native WebView wrapper.

Initial files:

- `www/index.html` mobile shell
- `www/app.js` REST/WebSocket client
- `www/manifest.webmanifest` PWA manifest
- `www/sw.js` small offline shell cache

