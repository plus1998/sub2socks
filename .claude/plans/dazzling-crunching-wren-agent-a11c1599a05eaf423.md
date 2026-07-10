# Implementation Plan: SOCKS5 Auth-Based Multiplexer

## Overview

Replace the current "one-port-per-account" SOCKS architecture with a single external port (default 9999) that routes connections to different proxy nodes based on SOCKS5 username/password authentication.

**Current flow:** User -> port 10801 -> Node A, User -> port 10802 -> Node B
**New flow:** User -> port 9999 (username "proxy") -> Rust SOCKS5 proxy -> 127.0.0.1:50001 -> Mihomo -> Node A

## Architecture Diagram

```
External Clients
      |
      v
  SOCKS_PORT (9999)  <-- Rust SOCKS5 Multiplexer (new socks_proxy.rs)
      |                         |
      |              username lookup via Database
      |                         |
      v                         v
  127.0.0.1:50001     127.0.0.1:50002   ...   (internal Mihomo SOCKS listeners)
      |                         |
      v                         v
   Node A                   Node B           (upstream proxy nodes)
```

## Task Breakdown

### Task 1: Add `find_account_by_username` and port auto-assignment to `db.rs`

**File:** `src/db.rs`

**Changes:**

1. **Add new method `find_account_by_username`:**
```rust
pub fn find_account_by_username(&self, username: &str) -> Result<Option<SocksAccount>> {
    let conn = self.conn()?;
    conn.query_row(
        "SELECT id, name, username, password, node_id, listen_port, enabled, created_at
         FROM socks_accounts WHERE username = ? AND enabled = 1",
        [username],
        Self::map_account,
    )
    .optional()
}
```
Used by the SOCKS5 multiplexer to look up which internal port to forward to.

2. **Add new method `auto_assign_port`:**
```rust
pub fn auto_assign_port(&self) -> Result<u16> {
    const BASE_PORT: u16 = 50001;
    let conn = self.conn()?;
    let max_port: Option<i64> = conn
        .query_row(
            "SELECT MAX(listen_port) FROM socks_accounts WHERE listen_port >= ?",
            [BASE_PORT as i64],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    match max_port {
        Some(max) if max >= BASE_PORT as i64 && max < 65535 => Ok((max as u16) + 1),
        _ => Ok(BASE_PORT),
    }
}
```
Returns the next available port starting from 50001. Finds the current max assigned port, then returns max+1. If no ports are assigned yet, returns 50001.

3. **Modify `add_socks_account` signature** to remove `listen_port` parameter -- it will call `auto_assign_port` internally:
```rust
pub fn add_socks_account(
    &self,
    name: &str,
    username: &str,
    password: &str,
    node_id: i64,
) -> Result<i64> {
    let port = self.auto_assign_port()?;
    let conn = self.conn()?;
    conn.execute(
        "INSERT INTO socks_accounts (name, username, password, node_id, listen_port)
         VALUES (?, ?, ?, ?, ?)",
        params![name, username, password, node_id, port as i64],
    )?;
    Ok(conn.last_insert_rowid())
}
```

4. **Modify `update_socks_account`** similarly to remove `listen_port` parameter. Updates on an account should NOT change the internal port (it stays stable for that account):
```rust
pub fn update_socks_account(
    &self,
    id: i64,
    name: &str,
    username: &str,
    password: &str,
    node_id: i64,
) -> Result<bool> {
    let conn = self.conn()?;
    let changed = conn.execute(
        "UPDATE socks_accounts
         SET name = ?, username = ?, password = ?, node_id = ?
         WHERE id = ?",
        params![name, username, password, node_id, id],
    )?;
    Ok(changed > 0)
}
```

5. **Update tests** in the `#[cfg(test)] mod tests` block to match the new signatures.

**Dependencies:** None (can be done independently)

---

### Task 2: Update `SocksAccountRequest` and API handlers in `main.rs`

**File:** `src/main.rs`

**Changes:**

1. **Remove `listen_port` field from `SocksAccountRequest`:**
```rust
#[derive(Debug, Deserialize)]
struct SocksAccountRequest {
    name: String,
    username: String,
    password: String,
    node_id: i64,
    enabled: Option<bool>,
}
```

2. **Add `SOCKS_PORT` env var and start the SOCKS5 proxy server.** At the top of `main()`:
```rust
let socks_port = std::env::var("SOCKS_PORT")
    .ok()
    .and_then(|value| value.parse::<u16>().ok())
    .unwrap_or(9999);
```

3. **Spawn the SOCKS5 proxy server** as a background tokio task after the database is initialized:
```rust
let socks_db = state.db.clone();
let socks_addr = SocketAddr::from(([0, 0, 0, 0], socks_port));
tokio::spawn(async move {
    println!("SOCKS5 multiplexer listening on {socks_addr}");
    if let Err(e) = rust_proxy_manager::socks_proxy::serve(socks_addr, socks_db).await {
        eprintln!("SOCKS5 multiplexer error: {e}");
    }
});
```

4. **Update `validate_socks_account`** to:
   - Remove the `listen_port == 0` check
   - Add a username uniqueness check (call `db.find_account_by_username`). Note: for updates (PUT), the username should be unique excluding the current account being edited. This means `validate_socks_account` needs an optional `exclude_id: Option<i64>` parameter.
   
```rust
fn validate_socks_account(
    db: &Database,
    payload: &SocksAccountRequest,
    exclude_id: Option<i64>,
) -> Result<(), ApiError> {
    if payload.name.trim().is_empty()
        || payload.username.trim().is_empty()
        || payload.password.is_empty()
    {
        return Err(ApiError::bad_request(
            "name, username, and password are required",
        ));
    }
    // Check username uniqueness
    if let Some(existing) = db.find_account_by_username(&payload.username)
        .map_err(ApiError::db)?
    {
        if Some(existing.id) != exclude_id {
            return Err(ApiError::bad_request("username already in use"));
        }
    }
    Ok(())
}
```

5. **Update `add_socks_account` handler:**
   - Remove `payload.listen_port` from the `db.add_socks_account(...)` call
   - Pass `db` reference to `validate_socks_account`

6. **Update `update_socks_account` handler:**
   - Remove `payload.listen_port` from the `db.update_socks_account(...)` call
   - Pass `Some(id)` as `exclude_id` to `validate_socks_account`

7. **Add a new API endpoint or extend the status endpoint** to expose `SOCKS_PORT` so the UI can display it:
   - Extend the `status` handler or add a new field to the status response:
```rust
Ok(Json(json!({
    "initialized": state.db.is_initialized().map_err(ApiError::db)?,
    "mihomo": mihomo.status(),
    "socks_port": std::env::var("SOCKS_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(9999u16),
})))
```

**Dependencies:** Task 1 (db.rs changes must be done first)

---

### Task 3: Create `src/socks_proxy.rs` -- The SOCKS5 Multiplexer Module

**File:** `src/socks_proxy.rs` (new file)

This is the core of the feature. It implements a partial SOCKS5 server that handles authentication and relays connections.

**SOCKS5 Protocol Details (RFC 1928 + RFC 1929):**

#### Phase 1: Method Negotiation (RFC 1928 Section 3)

```
Client -> Server: [0x05, nmethods, method_1, method_2, ...]
Server -> Client: [0x05, chosen_method]
```

The client sends SOCKS version (0x05), number of supported auth methods, and the method list. The server picks one.

Our server:
- Only supports `0x02` (USERNAME/PASSWORD) and `0x00` (NO AUTHENTICATION REQUIRED is NOT supported here -- we always require auth).
- If the client does not offer `0x02`, respond with `[0x05, 0xFF]` (no acceptable method) and close.
- If the client offers `0x02`, respond with `[0x05, 0x02]`.

#### Phase 2: Username/Password Authentication (RFC 1929)

```
Client -> Server: [0x01, username_len, username..., password_len, password...]
Server -> Client: [0x01, 0x00]  (success) or [0x01, 0x01]  (failure)
```

- VER: 0x01
- ULEN: 1 byte username length
- UNAME: username bytes
- PLEN: 1 byte password length
- PASSWD: password bytes

On failure (wrong password or username not found), return `[0x01, 0x01]` and close connection.

#### Phase 3: SOCKS5 Request (RFC 1928 Section 4)

```
Client -> Server: [0x05, CMD, RSV(0x00), ATYP, DST.ADDR, DST.PORT]
```

- CMD: 0x01=CONNECT (we only support CONNECT)
- ATYP: 0x01=IPv4 (4 bytes), 0x03=Domain (1 byte length + domain), 0x04=IPv6 (16 bytes)
- DST.ADDR: variable length
- DST.PORT: 2 bytes network order

We need to read the full address from the client, then forward the entire request verbatim to the internal Mihomo listener.

We reply:
```
Server -> Client: [0x05, REP, 0x00, ATYP, BND.ADDR, BND.PORT]
```
Even though we don't actually bind to the destination (Mihomo does), we respond with a successful reply indicating the address Mihomo is connecting to.

For simplicity, we can reply with `[0x05, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]` (success, IPv4, 0.0.0.0:0) -- this is common practice for relay proxies.

#### Phase 4: Bidirectional Data Relay

After the SOCKS5 handshake completes:
1. The client and the internal Mihomo listener are now connected.
2. We do `tokio::io::copy_bidirectional` between the two TCP streams.
3. On any error or EOF, close both streams.

**Module Structure:**

```rust
use crate::db::Database;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Start the SOCKS5 multiplexer server. Runs forever.
pub async fn serve(addr: SocketAddr, db: Database) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, client_addr) = listener.accept().await?;
        let db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, db).await {
                eprintln!("SOCKS5 error from {client_addr}: {e}");
            }
        });
    }
}

async fn handle_connection(
    mut client: TcpStream,
    db: Database,
) -> Result<(), Box<dyn std::error::Error>> {
    // Phase 1: Method negotiation
    let mut buf = [0u8; 257];
    let n = client.read(&mut buf).await?;
    if n < 3 || buf[0] != 0x05 {
        return Err("invalid SOCKS version".into());
    }
    let nmethods = buf[1] as usize;
    if n < 2 + nmethods {
        return Err("truncated method list".into());
    }
    let methods = &buf[2..2 + nmethods];
    if !methods.contains(&0x02) {
        // Client doesn't support username/password auth
        client.write_all(&[0x05, 0xFF]).await?;
        return Err("client does not support username/password auth".into());
    }
    client.write_all(&[0x05, 0x02]).await?;

    // Phase 2: Username/password authentication
    let n = client.read(&mut buf).await?;
    if n < 5 || buf[0] != 0x01 {
        return Err("invalid auth version".into());
    }
    let ulen = buf[1] as usize;
    if n < 2 + ulen + 2 {
        return Err("truncated username/password".into());
    }
    let username = std::str::from_utf8(&buf[2..2 + ulen])
        .map_err(|_| "invalid username encoding")?;
    let plen = buf[2 + ulen] as usize;
    let password = std::str::from_utf8(&buf[3 + ulen..3 + ulen + plen])
        .map_err(|_| "invalid password encoding")?;

    let account = db
        .find_account_by_username(username)?
        .filter(|a| a.password == password);

    let account = match account {
        Some(a) => a,
        None => {
            client.write_all(&[0x01, 0x01]).await?;
            return Err("authentication failed".into());
        }
    };
    client.write_all(&[0x01, 0x00]).await?;

    // Phase 3: Read SOCKS5 request
    let n = client.read(&mut buf).await?;
    if n < 10 || buf[0] != 0x05 {
        return Err("invalid SOCKS5 request".into());
    }
    let cmd = buf[1];
    if cmd != 0x01 {
        // Only CONNECT is supported
        send_reply(&mut client, 0x07).await?; // Command not supported
        return Err("unsupported command".into());
    }

    // Phase 4: Connect to internal Mihomo listener
    let internal_addr = format!("127.0.0.1:{}", account.listen_port);
    let mut upstream = TcpStream::connect(&internal_addr).await?;

    // Forward the original SOCKS5 request to the upstream
    upstream.write_all(&buf[..n]).await?;

    // Read the upstream's SOCKS5 reply and forward back to client
    let n = upstream.read(&mut buf).await?;
    client.write_all(&buf[..n]).await?;

    // Phase 5: Bidirectional relay
    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    let c2u = tokio::io::copy(&mut client_read, &mut upstream_write);
    let u2c = tokio::io::copy(&mut upstream_read, &mut client_write);

    tokio::select! {
        _ = c2u => {}
        _ = u2c => {}
    }

    Ok(())
}

async fn send_reply(stream: &mut TcpStream, rep: u8) -> Result<(), Box<dyn std::error::Error>> {
    let reply = [0x05, rep, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    stream.write_all(&reply).await?;
    Ok(())
}
```

**Important considerations:**
- `tokio::io::copy_bidirectional` is available in `tokio::io` since tokio 1.35. Use it to simplify the relay code.
- Handle `RST`/`FIN` gracefully -- the relay should end when either side closes.
- Log connection start/end with the username for debugging.
- The `buf` size of 257 bytes is sufficient: version(1) + nmethods(1) + methods(255) = 257.

**Dependencies:** Task 1 (needs `find_account_by_username`)

---

### Task 4: Register module in `src/lib.rs`

**File:** `src/lib.rs`

Add:
```rust
pub mod socks_proxy;
```

**Dependencies:** Task 3

---

### Task 5: Update `src/config.rs` -- Internal 127.0.0.1 binding

**File:** `src/config.rs`

**Changes:**

1. **In `listener_yaml` function**, change the `listen` field from `"0.0.0.0"` to `"127.0.0.1"`:
```rust
insert_str(&mut mapping, "listen", "127.0.0.1");
```

This ensures the internal Mihomo SOCKS listeners only accept connections from localhost (the Rust SOCKS5 proxy).

2. **Update existing tests** to verify the new listen address:
```rust
assert!(yaml.contains("listen: 127.0.0.1"));
```

**Dependencies:** None (can be done independently)

---

### Task 6: Update `src/static/index.html` -- UI Changes

**File:** `src/static/index.html`

**Changes:**

1. **Remove the `listen_port` input field** from the account form (lines 98-101):
   - Delete the `<label id="acc-port-label">` block entirely.

2. **Remove `listen_port` from the form submit handler** (around line 640):
   - Remove `payload.listen_port = Number(payload.listen_port);`

3. **Remove `listen_port` from the edit account handler** (around line 703):
   - Remove `fields.listen_port.value = account.listen_port;`

4. **Remove the Port column from the accounts table** (line 577, 584):
   - Remove `<th>${_('tablePort')}</th>` from the header
   - Remove `<td class="mono">${account.listen_port}</td>` from the row

5. **Add SOCKS proxy port display** in the status/header area (after line 35). Add after the status line:
```html
<p id="socks-line">SOCKS5 Port: -</p>
```
Update this in `render()`:
```javascript
$('#socks-line').textContent = `SOCKS5 Port: ${state.socks_port || '-'}`;
```

6. **Capture `socks_port` from the status API** in `checkInit()` and `refreshAll()`:
```javascript
state.socks_port = status.socks_port;
```

7. **Add i18n keys** for the SOCKS port display (`socksPort: 'SOCKS5 Port'` / `SOCKS5 端口`).

8. **Remove `accountPort` i18n key** since the port field is gone (both `zh-CN` and `en`).

9. **Remove the port label translation setup** in `setLanguage()` (the `accPortLabel` block around lines 315-316).

10. **Remove the port column translation** (`tablePort` i18n key) from both language dictionaries.

11. **Layout adjustment:** The account form currently has 5 labels + 2 buttons. After removing the port label, it will have 4 labels + 2 buttons. The CSS grid should still work as-is since the grid auto-fills.

**Dependencies:** Task 2 (status endpoint needs to return `socks_port`)

---

### Task 7: Update Docker files

**File:** `Dockerfile`

Change line 38:
```
EXPOSE 3000 9999
```

**File:** `docker-compose.yml`

Change lines 11-14:
```yaml
ports:
  - "3000:3000"
  - "9999:9999"
```

**File:** `.env.example`

Add:
```
# Single external SOCKS5 port (handles all authenticated users).
SOCKS_PORT=9999
```

**Dependencies:** None (can be done independently)

---

### Task 8: Update tests

**File:** `src/db.rs` tests

- Update `manages_socks_accounts` test to remove port parameters from `add_socks_account` and `update_socks_account`
- Update `syncing_nodes_preserves_matching_node_ids_and_accounts` test
- Update `syncing_nodes_distinguishes_credentials_containing_delimiters` test
- Add a new test `auto_assigns_ports` that verifies `auto_assign_port()` behavior
- Add a new test `finds_account_by_username` that verifies the username lookup

**File:** `src/config.rs` tests

- Update `generates_listener_per_account` test to assert `127.0.0.1` binding

**Dependencies:** All other implementation tasks

---

## Verification Plan

### Unit Tests
1. `cargo test` -- all existing and new tests pass
2. Specifically verify:
   - `auto_assign_port` returns 50001 on empty DB
   - `auto_assign_port` returns 50002 when 50001 is taken
   - `find_account_by_username` returns correct account
   - `find_account_by_username` returns None for non-existent user
   - Config YAML contains `listen: 127.0.0.1` for SOCKS listeners
   - `validate_socks_account` rejects duplicate usernames

### Integration / Manual Testing
1. Start the app: `cargo run`
2. Add a subscription and sync nodes
3. Create two SOCKS accounts with different usernames pointing to different nodes
4. Start Mihomo
5. Test with a SOCKS5 client (e.g., `curl`):
   ```bash
   # Test user 1
   curl --socks5 user1:pass1@127.0.0.1:9999 https://httpbin.org/ip
   # Test user 2
   curl --socks5 user2:pass2@127.0.0.1:9999 https://httpbin.org/ip
   ```
6. Verify each request routes through the correct node (different IPs)
7. Test bad credentials:
   ```bash
   curl --socks5 baduser:badpass@127.0.0.1:9999 https://httpbin.org/ip
   # Should fail with SOCKS5 authentication error
   ```
8. Verify the UI shows the SOCKS5 port and form works without port field

### Docker Verification
1. `docker compose build`
2. `docker compose up`
3. Same curl tests against `127.0.0.1:9999`
4. Verify port 9999 is exposed and working

---

## Dependency Graph

```
Task 1 (db.rs) ──────┬──> Task 2 (main.rs api + server spawn)
                     │         │
                     │         └──> Task 6 (UI changes)
                     │
                     ├──> Task 3 (socks_proxy.rs)
                     │         │
                     │         └──> Task 4 (lib.rs)
                     │
Task 5 (config.rs) ──┤  (independent)
                     │
Task 7 (Docker) ─────┤  (independent)
                     │
                     └──> Task 8 (test updates) -- done last
```

## Files Changed Summary

| File | Change Type | Description |
|------|-------------|-------------|
| `src/socks_proxy.rs` | **New** | SOCKS5 multiplexer module |
| `src/db.rs` | **Modified** | Add `find_account_by_username`, `auto_assign_port`; modify `add_socks_account`/`update_socks_account` signatures |
| `src/main.rs` | **Modified** | Remove `listen_port` from request struct; add username uniqueness validation; spawn SOCKS5 proxy server; expose `socks_port` in status |
| `src/config.rs` | **Modified** | Change SOCKS listener bind from `0.0.0.0` to `127.0.0.1` |
| `src/lib.rs` | **Modified** | Add `pub mod socks_proxy` |
| `src/static/index.html` | **Modified** | Remove `listen_port` input; add SOCKS port display; remove port column |
| `Dockerfile` | **Modified** | `EXPOSE 3000 9999` |
| `docker-compose.yml` | **Modified** | Map `9999:9999` |
| `.env.example` | **Modified** | Add `SOCKS_PORT=9999` |

## Risk Assessment

1. **Port collision**: If a user has existing accounts with ports in the 50001+ range, the auto-assignment might conflict. Mitigation: `auto_assign_port` always picks max+1 so it should avoid collisions even with existing data.

2. **SOCKS5 clients without auth support**: Some clients may not support username/password auth. The proxy responds with `0xFF` (no acceptable methods) which is the SOCKS5-standard way to signal this. The client will get a clear error.

3. **Bidirectional relay edge cases**: TCP half-close is handled by `tokio::io::copy_bidirectional` which properly propagates shutdown signals. Connection leaks are prevented by `tokio::select!` on both copy futures.

4. **Race condition in port assignment**: Two concurrent account creations could theoretically get the same port. Mitigation: SQLite's write lock serializes concurrent `auto_assign_port` calls.

5. **Existing accounts after migration**: Accounts created before this change will have arbitrary `listen_port` values. The Mihomo config will continue generating listeners for those ports. The multiplexer will look up accounts by username and forward to whatever internal port is stored, so this should work transparently.
