use crate::db::Database;
use std::net::SocketAddr;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Start the SOCKS5 multiplexer server. Runs forever.
///
/// Each incoming connection is authenticated via SOCKS5 username/password (RFC 1929).
/// Based on the username, the connection is forwarded to the corresponding internal
/// Mihomo SOCKS listener on `127.0.0.1:<account.listen_port>`.
pub async fn serve(addr: SocketAddr, db: Database) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("SOCKS5 proxy failed to bind {addr}: {e}");
            return;
        }
    };
    println!("SOCKS5 multiplexer listening on {addr}");

    loop {
        match listener.accept().await {
            Ok((stream, client_addr)) => {
                let db = db.clone();
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, db).await {
                        if e.to_string() != "EOF" {
                            eprintln!("SOCKS5 error from {client_addr}: {e}");
                        }
                    }
                });
            }
            Err(e) => {
                eprintln!("SOCKS5 accept error: {e}");
            }
        }
    }
}

async fn handle_connection(
    mut client: TcpStream,
    db: Database,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut buf = [0u8; 512];

    // --- Phase 1: Method negotiation (RFC 1928 §3) ---
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

    // --- Phase 2: Username/password authentication (RFC 1929) ---
    let n = client.read(&mut buf).await?;
    if n < 5 || buf[0] != 0x01 {
        return Err("invalid auth version".into());
    }
    let ulen = buf[1] as usize;
    if n < 4 + ulen {
        return Err("truncated username/password".into());
    }
    let username =
        std::str::from_utf8(&buf[2..2 + ulen]).map_err(|_| "invalid username encoding")?;
    let plen = buf[2 + ulen] as usize;
    if n < 3 + ulen + plen {
        return Err("truncated password".into());
    }
    let password = &buf[3 + ulen..3 + ulen + plen];

    let account = db
        .find_account_by_username(username)?
        .filter(|a| a.password.as_bytes() == password);

    let account = match account {
        Some(a) => a,
        None => {
            client.write_all(&[0x01, 0x01]).await?;
            return Err("authentication failed".into());
        }
    };
    client.write_all(&[0x01, 0x00]).await?;

    // --- Phase 3: SOCKS5 request ---
    let n = client.read(&mut buf).await?;
    if n < 10 {
        return Err("truncated SOCKS5 request".into());
    }
    if buf[0] != 0x05 {
        return Err("invalid SOCKS5 request version".into());
    }
    let cmd = buf[1];
    if cmd != 0x01 {
        // Only CONNECT is supported
        send_reply(&mut client, 0x07).await?; // Command not supported
        return Err("unsupported command (only CONNECT)".into());
    }

    // Save the client's CONNECT request before upstream handshake overwrites buf
    let connect_request = buf[..n].to_vec();

    // --- Phase 4: Full SOCKS5 handshake with internal Mihomo listener ---
    let internal_addr = format!("127.0.0.1:{}", account.listen_port);
    let mut upstream = match TcpStream::connect(&internal_addr).await {
        Ok(s) => s,
        Err(e) => {
            send_reply(&mut client, 0x01).await?;
            return Err(
                format!("failed to connect to internal listener {internal_addr}: {e}").into(),
            );
        }
    };

    // 4a: Method negotiation — upstream requires username/password auth
    upstream.write_all(&[0x05, 0x01, 0x02]).await?;
    let m = upstream.read(&mut buf).await?;
    if m < 2 || buf[0] != 0x05 || buf[1] != 0x02 {
        send_reply(&mut client, 0x01).await?;
        return Err("upstream did not accept username/password auth".into());
    }

    // 4b: Username/password sub-negotiation (RFC 1929)
    let u = account.username.as_bytes();
    let p = account.password.as_bytes();
    let mut auth_msg = Vec::with_capacity(3 + u.len() + p.len());
    auth_msg.push(0x01); // VER
    auth_msg.push(u.len() as u8); // ULEN
    auth_msg.extend_from_slice(u); // UNAME
    auth_msg.push(p.len() as u8); // PLEN
    auth_msg.extend_from_slice(p); // PASSWD
    upstream.write_all(&auth_msg).await?;
    let m = upstream.read(&mut buf).await?;
    if m < 2 || buf[0] != 0x01 || buf[1] != 0x00 {
        send_reply(&mut client, 0x01).await?;
        return Err("upstream authentication failed".into());
    }

    // 4c: Forward client's original CONNECT request to upstream
    upstream.write_all(&connect_request).await?;

    // 4d: Read upstream's reply and forward to client
    let m = upstream.read(&mut buf).await?;
    if m < 10 || buf[0] != 0x05 {
        send_reply(&mut client, 0x01).await?;
        return Err("upstream SOCKS5 reply invalid".into());
    }
    if buf[1] != 0x00 {
        // Upstream returned an error code — forward it to client
        client.write_all(&buf[..m]).await?;
        return Err(format!("upstream connect failed, REP={}", buf[1]).into());
    }
    client.write_all(&buf[..m]).await?;

    // --- Phase 5: Bidirectional relay ---
    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = upstream.into_split();

    let c2u = tokio::io::copy(&mut client_read, &mut upstream_write);
    let u2c = tokio::io::copy(&mut upstream_read, &mut client_write);

    tokio::select! {
        r = c2u => { let _ = r; }
        r = u2c => { let _ = r; }
    }

    Ok(())
}

async fn send_reply(
    stream: &mut TcpStream,
    rep: u8,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // SOCKS5 reply: VER=0x05 REP RSV=0x00 ATYP=0x01(IPv4) BND.ADDR=0.0.0.0 BND.PORT=0
    let reply = [0x05, rep, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
    stream.write_all(&reply).await?;
    Ok(())
}
