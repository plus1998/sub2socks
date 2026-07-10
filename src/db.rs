use rusqlite::{params, Connection, OptionalExtension, Params, Result, Row};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::types::{NodeType, ProxyNode, RuntimeSetting, SocksAccount, Subscription};

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct NodeSyncKey {
    node_type: String,
    server: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
}

fn node_sync_key(node: &ProxyNode) -> NodeSyncKey {
    NodeSyncKey {
        node_type: node.node_type.as_str().to_string(),
        server: node.server.clone(),
        port: node.port,
        username: node.username.clone(),
        password: node.password.clone(),
    }
}

impl Database {
    pub fn init() -> Result<Self> {
        let path = std::env::var_os("RUST_PROXY_MANAGER_DB")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("proxy_manager.db"));
        Self::open(path)
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        db.create_tables()?;
        Ok(db)
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn.lock().map_err(|_| rusqlite::Error::InvalidQuery)
    }

    fn create_tables(&self) -> Result<()> {
        let conn = self.conn()?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS runtime_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS subscriptions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                url TEXT NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_synced_at INTEGER,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now'))
            )",
            [],
        )?;

        if Self::table_exists(&conn, "proxy_nodes")?
            && !Self::table_has_column(&conn, "proxy_nodes", "subscription_id")?
        {
            conn.execute("DROP TABLE proxy_nodes", [])?;
        }

        conn.execute(
            "CREATE TABLE IF NOT EXISTS proxy_nodes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subscription_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                raw TEXT NOT NULL,
                node_type TEXT NOT NULL,
                server TEXT NOT NULL,
                port INTEGER NOT NULL,
                username TEXT,
                password TEXT,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                FOREIGN KEY (subscription_id) REFERENCES subscriptions(id) ON DELETE CASCADE
            )",
            [],
        )?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS socks_accounts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                username TEXT NOT NULL,
                password TEXT NOT NULL,
                node_id INTEGER NOT NULL,
                listen_port INTEGER NOT NULL UNIQUE,
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                FOREIGN KEY (node_id) REFERENCES proxy_nodes(id) ON DELETE CASCADE
            )",
            [],
        )?;

        Ok(())
    }

    fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
            [table],
            |row| row.get::<_, i64>(0),
        )
        .map(|v| v == 1)
    }

    fn table_has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
        for name in rows {
            if name? == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub fn is_initialized(&self) -> Result<bool> {
        Ok(self.get_setting("initialized")?.as_deref() == Some("true"))
    }

    pub fn initialize(&self, admin_user: &str, admin_pass: &str) -> Result<()> {
        self.set_setting("initialized", "true")?;
        self.set_setting("admin_user", admin_user)?;
        // TODO: hash this password before a production release.
        self.set_setting("admin_pass", admin_pass)?;
        Ok(())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO settings (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row("SELECT value FROM settings WHERE key = ?", [key], |row| {
            row.get(0)
        })
        .optional()
    }

    pub fn set_runtime_setting(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO runtime_settings (key, value, updated_at) VALUES (?, ?, strftime('%s','now'))
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = strftime('%s','now')",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_runtime_setting(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT value FROM runtime_settings WHERE key = ?",
            [key],
            |row| row.get(0),
        )
        .optional()
    }

    pub fn list_runtime_settings(&self) -> Result<Vec<RuntimeSetting>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT key, value FROM runtime_settings ORDER BY key")?;
        let rows = stmt.query_map([], |row| {
            Ok(RuntimeSetting {
                key: row.get(0)?,
                value: row.get(1)?,
            })
        })?;
        rows.collect()
    }

    pub fn add_subscription(&self, name: &str, url: &str) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO subscriptions (name, url, enabled) VALUES (?, ?, 1)
             ON CONFLICT(url) DO UPDATE SET name = excluded.name, enabled = 1",
            params![name, url],
        )?;
        conn.query_row("SELECT id FROM subscriptions WHERE url = ?", [url], |row| {
            row.get(0)
        })
    }

    pub fn list_subscriptions(&self) -> Result<Vec<Subscription>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, url, enabled, last_synced_at, created_at
             FROM subscriptions ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], Self::map_subscription)?;
        rows.collect()
    }

    pub fn get_subscription(&self, id: i64) -> Result<Option<Subscription>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, url, enabled, last_synced_at, created_at
             FROM subscriptions WHERE id = ?",
            [id],
            Self::map_subscription,
        )
        .optional()
    }

    pub fn delete_subscription(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM subscriptions WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn set_subscription_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE subscriptions SET enabled = ? WHERE id = ?",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    pub fn mark_subscription_synced(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE subscriptions SET last_synced_at = strftime('%s','now') WHERE id = ?",
            [id],
        )?;
        Ok(())
    }

    pub fn replace_subscription_nodes(
        &self,
        subscription_id: i64,
        nodes: &[ProxyNode],
    ) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction()?;
        let mut existing_by_key = {
            let mut stmt = tx.prepare(
                "SELECT id, subscription_id, name, raw, node_type, server, port, username, password, enabled, created_at
                 FROM proxy_nodes WHERE subscription_id = ? ORDER BY id ASC",
            )?;
            let existing = stmt
                .query_map(params![subscription_id], Self::map_node)?
                .collect::<Result<Vec<_>>>()?;
            let mut by_key: HashMap<NodeSyncKey, Vec<ProxyNode>> = HashMap::new();
            for node in existing {
                by_key.entry(node_sync_key(&node)).or_default().push(node);
            }
            by_key
        };

        for node in nodes {
            let key = node_sync_key(node);
            let existing = existing_by_key
                .get_mut(&key)
                .and_then(|matches| (!matches.is_empty()).then(|| matches.remove(0)));

            if let Some(existing) = existing {
                tx.execute(
                    "UPDATE proxy_nodes
                     SET name = ?, raw = ?, node_type = ?, server = ?, port = ?, username = ?, password = ?
                     WHERE id = ?",
                    params![
                        node.name,
                        node.raw,
                        node.node_type.as_str(),
                        node.server,
                        node.port as i64,
                        node.username,
                        node.password,
                        existing.id,
                    ],
                )?;
            } else {
                tx.execute(
                    "INSERT INTO proxy_nodes
                     (subscription_id, name, raw, node_type, server, port, username, password, enabled)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![
                        subscription_id,
                        node.name,
                        node.raw,
                        node.node_type.as_str(),
                        node.server,
                        node.port as i64,
                        node.username,
                        node.password,
                        node.enabled as i64
                    ],
                )?;
            }
        }

        for stale_nodes in existing_by_key.values() {
            for node in stale_nodes {
                tx.execute("DELETE FROM proxy_nodes WHERE id = ?", [node.id])?;
            }
        }

        tx.commit()?;
        Ok(())
    }

    pub fn list_nodes(&self) -> Result<Vec<ProxyNode>> {
        self.query_nodes(
            "SELECT id, subscription_id, name, raw, node_type, server, port, username, password, enabled, created_at
             FROM proxy_nodes ORDER BY id DESC",
            [],
        )
    }

    pub fn list_enabled_nodes(&self) -> Result<Vec<ProxyNode>> {
        self.query_nodes(
            "SELECT id, subscription_id, name, raw, node_type, server, port, username, password, enabled, created_at
             FROM proxy_nodes WHERE enabled = 1 ORDER BY id DESC",
            [],
        )
    }

    pub fn list_nodes_by_subscription(&self, subscription_id: i64) -> Result<Vec<ProxyNode>> {
        self.query_nodes(
            "SELECT id, subscription_id, name, raw, node_type, server, port, username, password, enabled, created_at
             FROM proxy_nodes WHERE subscription_id = ? ORDER BY id DESC",
            params![subscription_id],
        )
    }

    pub fn get_node(&self, id: i64) -> Result<Option<ProxyNode>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, subscription_id, name, raw, node_type, server, port, username, password, enabled, created_at
             FROM proxy_nodes WHERE id = ?",
            [id],
            Self::map_node,
        )
        .optional()
    }

    pub fn set_node_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE proxy_nodes SET enabled = ? WHERE id = ?",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    pub fn delete_node(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM proxy_nodes WHERE id = ?", [id])?;
        Ok(())
    }

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
            Some(max) if max >= BASE_PORT as i64 => Ok((max as u16) + 1),
            _ => Ok(BASE_PORT),
        }
    }

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

    pub fn list_socks_accounts(&self) -> Result<Vec<SocksAccount>> {
        self.query_accounts(
            "SELECT id, name, username, password, node_id, listen_port, enabled, created_at
             FROM socks_accounts ORDER BY id DESC",
            [],
        )
    }

    pub fn list_enabled_socks_accounts(&self) -> Result<Vec<SocksAccount>> {
        self.query_accounts(
            "SELECT id, name, username, password, node_id, listen_port, enabled, created_at
             FROM socks_accounts WHERE enabled = 1 ORDER BY id DESC",
            [],
        )
    }

    pub fn get_socks_account(&self, id: i64) -> Result<Option<SocksAccount>> {
        let conn = self.conn()?;
        conn.query_row(
            "SELECT id, name, username, password, node_id, listen_port, enabled, created_at
             FROM socks_accounts WHERE id = ?",
            [id],
            Self::map_account,
        )
        .optional()
    }

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

    pub fn delete_socks_account(&self, id: i64) -> Result<()> {
        let conn = self.conn()?;
        conn.execute("DELETE FROM socks_accounts WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn set_socks_account_enabled(&self, id: i64, enabled: bool) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE socks_accounts SET enabled = ? WHERE id = ?",
            params![enabled as i64, id],
        )?;
        Ok(())
    }

    fn query_nodes<P: Params>(&self, sql: &str, params: P) -> Result<Vec<ProxyNode>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, Self::map_node)?;
        rows.collect()
    }

    fn query_accounts<P: Params>(&self, sql: &str, params: P) -> Result<Vec<SocksAccount>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params, Self::map_account)?;
        rows.collect()
    }

    fn map_subscription(row: &Row<'_>) -> Result<Subscription> {
        Ok(Subscription {
            id: row.get(0)?,
            name: row.get(1)?,
            url: row.get(2)?,
            enabled: row.get::<_, i64>(3)? != 0,
            last_synced_at: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    fn map_node(row: &Row<'_>) -> Result<ProxyNode> {
        let node_type: String = row.get(4)?;
        Ok(ProxyNode {
            id: row.get(0)?,
            subscription_id: row.get(1)?,
            name: row.get(2)?,
            raw: row.get(3)?,
            node_type: NodeType::parse(&node_type),
            server: row.get(5)?,
            port: row.get::<_, i64>(6)? as u16,
            username: row.get(7)?,
            password: row.get(8)?,
            enabled: row.get::<_, i64>(9)? != 0,
            created_at: row.get(10)?,
        })
    }

    fn map_account(row: &Row<'_>) -> Result<SocksAccount> {
        Ok(SocksAccount {
            id: row.get(0)?,
            name: row.get(1)?,
            username: row.get(2)?,
            password: row.get(3)?,
            node_id: row.get(4)?,
            listen_port: row.get::<_, i64>(5)? as u16,
            enabled: row.get::<_, i64>(6)? != 0,
            created_at: row.get(7)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_db() -> Database {
        Database::open(":memory:").unwrap()
    }

    fn sample_node(subscription_id: i64, name: &str) -> ProxyNode {
        ProxyNode {
            id: 0,
            subscription_id,
            name: name.to_string(),
            raw: format!("name: {name}\ntype: http\nserver: 127.0.0.1\nport: 8080"),
            node_type: NodeType::Http,
            server: "127.0.0.1".to_string(),
            port: 8080,
            username: None,
            password: None,
            enabled: true,
            created_at: 0,
        }
    }

    #[test]
    fn initializes_settings() {
        let db = new_db();
        assert!(!db.is_initialized().unwrap());
        db.initialize("admin", "secret").unwrap();
        assert!(db.is_initialized().unwrap());
        assert_eq!(
            db.get_setting("admin_user").unwrap().as_deref(),
            Some("admin")
        );
    }

    #[test]
    fn manages_subscriptions_and_nodes() {
        let db = new_db();
        let sub_id = db
            .add_subscription("sub", "https://example.com/sub")
            .unwrap();
        db.replace_subscription_nodes(sub_id, &[sample_node(sub_id, "node-a")])
            .unwrap();

        let subs = db.list_subscriptions().unwrap();
        assert_eq!(subs.len(), 1);

        let nodes = db.list_nodes_by_subscription(sub_id).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "node-a");

        db.set_node_enabled(nodes[0].id, false).unwrap();
        assert!(db.list_enabled_nodes().unwrap().is_empty());

        db.mark_subscription_synced(sub_id).unwrap();
        assert!(db
            .get_subscription(sub_id)
            .unwrap()
            .unwrap()
            .last_synced_at
            .is_some());
    }

    #[test]
    fn manages_socks_accounts() {
        let db = new_db();
        let sub_id = db
            .add_subscription("sub", "https://example.com/sub")
            .unwrap();
        db.replace_subscription_nodes(sub_id, &[sample_node(sub_id, "node-a")])
            .unwrap();
        let node_id = db.list_nodes().unwrap()[0].id;

        let account_id = db
            .add_socks_account("acct", "user", "pass", node_id)
            .unwrap();
        assert_eq!(db.list_enabled_socks_accounts().unwrap().len(), 1);

        db.update_socks_account(account_id, "acct2", "user2", "pass2", node_id)
            .unwrap();
        let account = db.list_socks_accounts().unwrap().remove(0);
        assert_eq!(account.name, "acct2");
        assert!(account.listen_port >= 50001);

        db.set_socks_account_enabled(account_id, false).unwrap();
        assert!(db.list_enabled_socks_accounts().unwrap().is_empty());
    }

    #[test]
    fn syncing_nodes_preserves_matching_node_ids_and_accounts() {
        let db = new_db();
        let sub_id = db
            .add_subscription("sub", "https://example.com/sub")
            .unwrap();
        db.replace_subscription_nodes(sub_id, &[sample_node(sub_id, "node-a")])
            .unwrap();
        let node_id = db.list_nodes_by_subscription(sub_id).unwrap()[0].id;
        let account_id = db
            .add_socks_account("acct", "user", "pass", node_id)
            .unwrap();

        let mut refreshed = sample_node(sub_id, "node-renamed");
        refreshed.raw =
            "name: node-renamed\ntype: http\nserver: 127.0.0.1\nport: 8080\nupdated: true"
                .to_string();
        db.replace_subscription_nodes(sub_id, &[refreshed]).unwrap();

        let nodes = db.list_nodes_by_subscription(sub_id).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, node_id);
        assert_eq!(nodes[0].name, "node-renamed");
        assert!(nodes[0].raw.contains("updated: true"));

        let accounts = db.list_socks_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, account_id);
        assert_eq!(accounts[0].node_id, node_id);
    }

    #[test]
    fn syncing_nodes_distinguishes_credentials_containing_delimiters() {
        let db = new_db();
        let sub_id = db
            .add_subscription("sub", "https://example.com/sub")
            .unwrap();

        let mut node_a = sample_node(sub_id, "node-a");
        node_a.username = Some("u|p".to_string());
        node_a.password = Some(String::new());

        let mut node_b = sample_node(sub_id, "node-b");
        node_b.username = Some("u".to_string());
        node_b.password = Some("p|".to_string());

        db.replace_subscription_nodes(sub_id, &[node_a, node_b.clone()])
            .unwrap();

        let node_b_id = db
            .list_nodes_by_subscription(sub_id)
            .unwrap()
            .into_iter()
            .find(|node| node.username.as_deref() == Some("u"))
            .unwrap()
            .id;
        let account_id = db
            .add_socks_account("acct", "user", "pass", node_b_id)
            .unwrap();

        node_b.name = "node-b-refreshed".to_string();
        db.replace_subscription_nodes(sub_id, &[node_b]).unwrap();

        let nodes = db.list_nodes_by_subscription(sub_id).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].id, node_b_id);
        assert_eq!(nodes[0].name, "node-b-refreshed");

        let accounts = db.list_socks_accounts().unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id, account_id);
        assert_eq!(accounts[0].node_id, node_b_id);
    }
}
