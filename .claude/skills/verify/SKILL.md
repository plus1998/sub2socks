---
name: verify
summary: Runtime verification recipe for the embedded web UI and API
---

# Verify sub2socks

1. Build the Vite assets with `npm --prefix frontend run build`, then copy `frontend/dist/index.html` and `frontend/dist/assets/app.{css,js}` into `src/static/`.
2. Launch with an isolated temporary database/data directory and unused ports:
   `RUST_PROXY_MANAGER_DB="$tmp/app.db" RUST_PROXY_MANAGER_DATA_DIR="$tmp/data" PORT=<web> SOCKS_PORT=<socks> cargo run`.
3. Drive the HTTP surface:
   - `GET /api/status` should report uninitialized/unauthenticated on a fresh DB.
   - `GET /` should reference `/assets/app.css` and `/assets/app.js` and contain the React root.
   - Both asset URLs should return 200 with CSS/JavaScript content types.
   - `GET /api/subscriptions` without a session should return 401.
   - Probe malformed `POST /api/init` and confirm a clear 4xx response.
4. Stop the server and remove the temporary runtime directory.
