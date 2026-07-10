# Plan: Single‑Port SOCKS5 Multiplexer

## Context

Currently each SOCKS account gets its own port (e.g., 10801→Singapore, 10802→US). The user wants **a single external port (9999)** where the SOCKS5 **username** determines which proxy node to use.

Mihomo does **not** support per‑user routing inside one listener (`proxy` is scalar). So we add a thin Rust ‑side SOCKS5 auth proxy that reads the username/password, looks up the account, and forwards to the corresponding internal Mihomo listener.

```
Client → :9999 (Rust SOCKS5 proxy) → 127.0.0.1:50001 → Mihomo → Node A
                                    → 127.0.0.1:50002 → Mihomo → Node B
```

## Files Changed

| File | Change |
|------|--------|
| `src/socks_proxy.rs` | **New** – SOCKS5 auth‑multiplexer |
| `src/db.rs` | Add `find_account_by_username`, `auto_assign_port`; remove `listen_port` param from create/update |
| `src/main.rs` | Remove `listen_port` from request; add username‑uniq validation; spawn proxy server; expose `socks_port` in status |
| `src/config.rs` | Bind internal listeners to `127.0.0.1` (no longer exposed externally) |
| `src/lib.rs` | Register `pub mod socks_proxy` |
| `src/static/index.html` | Remove port input/column; display single SOCKS5 port in header |
| `Dockerfile` | `EXPOSE 3000 9999` |
| `docker-compose.yml` | Map `9999:9999` |
| `.env.example` | Add `SOCKS_PORT=9999` |

## Task Breakdown (execution order)

### 1. `src/db.rs` – foundation
- `find_account_by_username(username) → Option<SocksAccount>` — used by proxy for auth lookup
- `auto_assign_port() → u16` — scans `MAX(listen_port)` where ≥ 50001, returns max+1 or 50001 as base
- `add_socks_account` — call `auto_assign_port()` internally, remove param
- `update_socks_account` — remove `listen_port` param (port is immutable after creation)
- Update `#[cfg(test)]` block to match new signatures

### 2. `src/socks_proxy.rs` – the core (new file)
- `pub async fn serve(addr: SocketAddr, db: Database)` – main loop
- Per‑connection (tokio::spawn):
  1. **Method negotiation**: require `0x02` (USERNAME/PASSWORD), reply `[0x05, 0x02]`
  2. **Auth** (RFC 1929): read username/password, look up via `db.find_account_by_username`, verify password
  3. **SOCKS5 CONNECT**: read full request (buf 512 B to cover domain ATYP up to 262 B), forward verbatim to `127.0.0.1:<account.listen_port>`
  4. **Relay**: `into_split` + `tokio::io::copy` with `tokio::select!`

### 3. `src/main.rs` – API & server spawn
- `SocksAccountRequest`: drop `listen_port` field
- `validate_socks_account`: accept `(db, exclude_id)`; check username uniq
- Spawn SOCKS5 server: read `SOCKS_PORT` env (default 9999)
- Status endpoint: add `"socks_port"` field

### 4. `src/config.rs` – internal binding
- `listener_yaml`: change `"listen"` from `"0.0.0.0"` → `"127.0.0.1"`

### 5. `src/lib.rs` – register module
- Add `pub mod socks_proxy;`

### 6. `src/static/index.html` – UI
- Remove `listen_port` input, port column, and related i18n
- Add SOCKS port display in header, fed from status API

### 7. Docker / env
- `Dockerfile`: `EXPOSE 3000 9999`
- `docker-compose.yml`: map `9999:9999`
- `.env.example`: `SOCKS_PORT=9999`

### 8. `cargo build --release` + `docker compose up` – end‑to‑end verification
- Create two accounts (different usernames/nodes), start Mihomo
- `curl --socks5 user1:pass1@localhost:9999 …` → node‑A IP
- `curl --socks5 user2:pass2@localhost:9999 …` → node‑B IP
- `curl --socks5 bad:pass@localhost:9999 …` → auth failure

## Verification

1. `cargo test` — all unit tests pass
2. Manual: create two accounts → start mihomo → curl tests confirm per‑username routing
3. Docker: `docker compose build && docker compose up` → same curl tests
