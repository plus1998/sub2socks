use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use rand::{distributions::Alphanumeric, Rng};
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use serde_yaml::{Mapping, Value};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout, Instant};

use crate::types::ProxyNode;

pub const PROBE_URL: &str = "https://www.gstatic.com/generate_204";
const PROXY_NAME: &str = "Node-Under-Test";
const TOTAL_TIMEOUT: Duration = Duration::from_secs(12);
const READY_TIMEOUT: Duration = Duration::from_secs(4);
const PORT_ATTEMPTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeTestOutcome {
    pub ok: bool,
    pub latency_ms: Option<i64>,
    pub error: Option<String>,
}

impl NodeTestOutcome {
    pub fn success(latency_ms: i64) -> Self {
        Self {
            ok: true,
            latency_ms: Some(latency_ms),
            error: None,
        }
    }

    pub fn failure(error: impl Into<String>) -> Self {
        Self {
            ok: false,
            latency_ms: None,
            error: Some(error.into()),
        }
    }
}

pub async fn test_node(binary_path: Option<PathBuf>, node: &ProxyNode) -> NodeTestOutcome {
    let Some(binary_path) = binary_path.filter(|path| path.exists()) else {
        return NodeTestOutcome::failure(
            "mihomo binary unavailable; configure MIHOMO_BINARY or install mihomo",
        );
    };

    match run_test(&binary_path, node).await {
        Ok(latency) => NodeTestOutcome::success(latency),
        Err(error) => NodeTestOutcome::failure(error),
    }
}

async fn run_test(binary_path: &Path, node: &ProxyNode) -> Result<i64, String> {
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    let client = Client::builder()
        .connect_timeout(Duration::from_secs(1))
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|_| "failed to initialize probe client".to_string())?;

    for _ in 0..PORT_ATTEMPTS {
        let port = available_local_port().await?;
        let secret = random_token(32);
        let config = build_test_config(node, port, &secret)?;
        let config_path = temporary_config_path();
        let mut process = match spawn_mihomo(binary_path, &config_path, &config).await {
            Ok(process) => process,
            Err(error) => {
                remove_file(&config_path);
                return Err(error);
            }
        };

        let remaining = deadline.saturating_duration_since(Instant::now());
        let result = if remaining.is_zero() {
            Err(ProbeError::Message("node test timed out".to_string()))
        } else {
            timeout(
                remaining,
                probe_process(&client, &mut process, port, &secret),
            )
            .await
            .unwrap_or_else(|_| Err(ProbeError::Message("node test timed out".to_string())))
        };
        stop_process(&mut process).await;
        remove_file(&config_path);

        match result {
            Err(ProbeError::PortCollision) => continue,
            Ok(latency) => return Ok(latency),
            Err(ProbeError::Message(error)) => return Err(error),
        }
    }

    Err("could not allocate a local controller port".to_string())
}

async fn available_local_port() -> Result<u16, String> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| "could not allocate a local controller port".to_string())?;
    let port = listener
        .local_addr()
        .map_err(|_| "could not allocate a local controller port".to_string())?
        .port();
    drop(listener);
    Ok(port)
}

fn random_token(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn temporary_config_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "sub2socks-node-test-{}-{}.yaml",
        std::process::id(),
        random_token(16)
    ))
}

async fn spawn_mihomo(binary: &Path, config_path: &Path, config: &str) -> Result<Child, String> {
    fs::write(config_path, config).map_err(|_| "failed to write temporary config".to_string())?;
    Command::new(binary)
        .arg("-f")
        .arg(config_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "failed to start temporary mihomo".to_string())
}

enum ProbeError {
    PortCollision,
    Message(String),
}

#[derive(Deserialize)]
struct DelayResponse {
    delay: i64,
}

async fn probe_process(
    client: &Client,
    process: &mut Child,
    port: u16,
    secret: &str,
) -> Result<i64, ProbeError> {
    let controller = format!("http://127.0.0.1:{port}");
    let ready_deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if process
            .try_wait()
            .map_err(|_| ProbeError::Message("temporary mihomo status check failed".to_string()))?
            .is_some()
        {
            return Err(ProbeError::PortCollision);
        }

        match client
            .get(format!("{controller}/version"))
            .bearer_auth(secret)
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => break,
            Ok(response) if response.status() == StatusCode::UNAUTHORIZED => {
                return Err(ProbeError::PortCollision)
            }
            _ if Instant::now() < ready_deadline => sleep(Duration::from_millis(100)).await,
            _ => {
                return Err(ProbeError::Message(
                    "temporary mihomo did not become ready".to_string(),
                ))
            }
        }
    }

    let response = client
        .get(format!("{controller}/proxies/{PROXY_NAME}/delay"))
        .bearer_auth(secret)
        .query(&[("url", PROBE_URL), ("timeout", "5000")])
        .send()
        .await
        .map_err(|_| ProbeError::Message("probe request failed".to_string()))?;
    if !response.status().is_success() {
        return Err(ProbeError::Message(format!(
            "node probe failed (controller status {})",
            response.status().as_u16()
        )));
    }
    let delay = response
        .json::<DelayResponse>()
        .await
        .map_err(|_| ProbeError::Message("invalid probe response".to_string()))?
        .delay;
    if delay < 0 {
        return Err(ProbeError::Message("invalid probe latency".to_string()));
    }
    Ok(delay)
}

async fn stop_process(process: &mut Child) {
    if process.try_wait().ok().flatten().is_none() {
        let _ = process.kill().await;
    }
    let _ = process.wait().await;
}

fn remove_file(path: &Path) {
    let _ = fs::remove_file(path);
}

pub fn build_test_config(
    node: &ProxyNode,
    controller_port: u16,
    secret: &str,
) -> Result<String, String> {
    let mut proxy = parse_proxy_mapping(node)?;
    proxy.insert(
        Value::String("name".to_string()),
        Value::String(PROXY_NAME.to_string()),
    );

    let mut root = Mapping::new();
    insert_str(&mut root, "bind-address", "127.0.0.1");
    insert_bool(&mut root, "allow-lan", false);
    insert_str(&mut root, "mode", "rule");
    insert_str(&mut root, "log-level", "silent");
    insert_str(
        &mut root,
        "external-controller",
        &format!("127.0.0.1:{controller_port}"),
    );
    insert_str(&mut root, "secret", secret);
    root.insert(
        Value::String("proxies".to_string()),
        Value::Sequence(vec![Value::Mapping(proxy)]),
    );
    root.insert(
        Value::String("rules".to_string()),
        Value::Sequence(vec![Value::String("MATCH,DIRECT".to_string())]),
    );
    serde_yaml::to_string(&Value::Mapping(root))
        .map_err(|_| "failed to build temporary mihomo config".to_string())
}

fn parse_proxy_mapping(node: &ProxyNode) -> Result<Mapping, String> {
    let mut mapping = serde_yaml::from_str::<Value>(&node.raw)
        .ok()
        .and_then(|value| value.as_mapping().cloned())
        .unwrap_or_default();
    insert_missing_str(&mut mapping, "type", node.node_type.as_str());
    insert_missing_str(&mut mapping, "server", &node.server);
    if !mapping.contains_key(Value::String("port".to_string())) {
        mapping.insert(
            Value::String("port".to_string()),
            Value::Number(serde_yaml::Number::from(node.port)),
        );
    }
    if let Some(username) = &node.username {
        insert_missing_str(&mut mapping, "username", username);
    }
    if let Some(password) = &node.password {
        insert_missing_str(&mut mapping, "password", password);
    }
    if mapping
        .get(Value::String("type".to_string()))
        .and_then(Value::as_str)
        .is_none()
    {
        return Err("node has no valid proxy type".to_string());
    }
    Ok(mapping)
}

fn insert_missing_str(mapping: &mut Mapping, key: &str, value: &str) {
    let yaml_key = Value::String(key.to_string());
    if !mapping.contains_key(&yaml_key) {
        mapping.insert(yaml_key, Value::String(value.to_string()));
    }
}

fn insert_str(mapping: &mut Mapping, key: &str, value: &str) {
    mapping.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
}

fn insert_bool(mapping: &mut Mapping, key: &str, value: bool) {
    mapping.insert(Value::String(key.to_string()), Value::Bool(value));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::NodeType;
    use std::net::{IpAddr, SocketAddr};

    fn hy2_node() -> ProxyNode {
        ProxyNode {
            id: 9,
            subscription_id: 1,
            name: "private name".to_string(),
            raw: "name: private name\ntype: hysteria2\nserver: hy.example.com\nport: 443\npassword: super-secret\nsni: edge.example.com\nup: 20 Mbps"
                .to_string(),
            node_type: NodeType::Hysteria2,
            server: "hy.example.com".to_string(),
            port: 443,
            username: None,
            password: Some("super-secret".to_string()),
            enabled: true,
            created_at: 0,
            last_tested_at: None,
            last_test_ok: None,
            last_test_latency_ms: None,
            last_test_error: None,
        }
    }

    #[test]
    fn test_config_contains_only_target_hy2_raw() {
        let yaml = build_test_config(&hy2_node(), 34567, "controller-secret").unwrap();
        let parsed: Value = serde_yaml::from_str(&yaml).unwrap();
        let root = parsed.as_mapping().unwrap();
        let proxies = root
            .get(Value::String("proxies".to_string()))
            .and_then(Value::as_sequence)
            .unwrap();

        assert_eq!(proxies.len(), 1);
        assert_eq!(
            proxies[0]
                .as_mapping()
                .unwrap()
                .get(Value::String("name".to_string()))
                .and_then(Value::as_str),
            Some(PROXY_NAME)
        );
        assert!(PROXY_NAME
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'));
        assert_eq!(
            format!("http://127.0.0.1:34567/proxies/{PROXY_NAME}/delay"),
            "http://127.0.0.1:34567/proxies/Node-Under-Test/delay"
        );
        assert!(yaml.contains("type: hysteria2"));
        assert!(yaml.contains("password: super-secret"));
        assert!(yaml.contains("up: 20 Mbps"));
        assert!(yaml.contains("external-controller: 127.0.0.1:34567"));
        assert!(yaml.contains("secret: controller-secret"));
        assert!(!yaml.contains("0.0.0.0"));
        assert!(!yaml.contains("All Nodes"));
    }

    #[tokio::test]
    async fn unavailable_binary_is_a_persistable_failure() {
        let result = test_node(None, &hy2_node()).await;
        assert!(!result.ok);
        assert!(result.error.unwrap().contains("binary unavailable"));
    }

    #[test]
    fn errors_do_not_include_node_credentials() {
        let result = NodeTestOutcome::failure("probe request failed");
        assert!(!result.error.unwrap().contains("super-secret"));
    }

    #[test]
    fn controller_address_is_loopback() {
        let address = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 9090);
        assert!(address.ip().is_loopback());
    }
}
