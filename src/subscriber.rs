use std::collections::HashMap;

use base64::{engine::general_purpose, Engine as _};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::types::{NodeType, ProxyNode};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedSubscription {
    pub source_url: String,
    pub nodes: Vec<ProxyNode>,
}

pub async fn fetch_subscription(
    url: &str,
    subscription_id: i64,
) -> Result<ParsedSubscription, String> {
    let body = reqwest::get(url)
        .await
        .map_err(|e| format!("failed to fetch subscription: {e}"))?
        .error_for_status()
        .map_err(|e| format!("subscription returned error status: {e}"))?
        .text()
        .await
        .map_err(|e| format!("failed to read subscription body: {e}"))?;

    let nodes = parse_subscription_content(&body, subscription_id)?;
    Ok(ParsedSubscription {
        source_url: url.to_string(),
        nodes,
    })
}

pub fn parse_subscription_content(
    content: &str,
    subscription_id: i64,
) -> Result<Vec<ProxyNode>, String> {
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if looks_like_clash_yaml(trimmed) {
        return parse_clash_yaml(trimmed, subscription_id);
    }

    if let Some(decoded) = decode_base64_subscription(trimmed) {
        if looks_like_clash_yaml(&decoded) {
            return parse_clash_yaml(&decoded, subscription_id);
        }
        return Ok(parse_uri_list(&decoded, subscription_id));
    }

    Ok(parse_uri_list(trimmed, subscription_id))
}

pub fn parse_clash_yaml(content: &str, subscription_id: i64) -> Result<Vec<ProxyNode>, String> {
    let parsed: Value =
        serde_yaml::from_str(content).map_err(|e| format!("YAML parse error: {e}"))?;
    let proxies = parsed
        .as_mapping()
        .and_then(|map| map.get(Value::String("proxies".to_string())))
        .and_then(Value::as_sequence)
        .ok_or_else(|| "YAML subscription does not contain a proxies list".to_string())?;

    let mut nodes = Vec::new();
    for item in proxies {
        if let Some(node) = parse_yaml_node(item, subscription_id) {
            nodes.push(node);
        }
    }
    Ok(nodes)
}

fn parse_yaml_node(value: &Value, subscription_id: i64) -> Option<ProxyNode> {
    let map = value.as_mapping()?;
    let name = yaml_string(map, "name")?;
    let node_type_name = yaml_string(map, "type")?;
    if is_proxy_group_type(&node_type_name) {
        return None;
    }

    let server = yaml_string(map, "server")?.trim().to_string();
    let port = yaml_i64(map, "port").and_then(|port| u16::try_from(port).ok())?;
    if server.is_empty() || port == 0 {
        return None;
    }

    let node_type = NodeType::parse(&node_type_name);
    let username = yaml_string(map, "username");
    let password = yaml_string(map, "password");
    let raw = serde_yaml::to_string(value).unwrap_or_else(|_| name.clone());

    Some(ProxyNode {
        id: 0,
        subscription_id,
        name,
        raw,
        node_type,
        server,
        port,
        username,
        password,
        enabled: true,
        created_at: 0,
    })
}

fn yaml_string(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(Value::String(key.to_string()))
        .and_then(|value| match value {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
}

fn yaml_i64(map: &serde_yaml::Mapping, key: &str) -> Option<i64> {
    map.get(Value::String(key.to_string()))
        .and_then(|value| match value {
            Value::Number(n) => n.as_i64(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        })
}

fn is_proxy_group_type(node_type: &str) -> bool {
    matches!(
        node_type.trim().to_ascii_lowercase().as_str(),
        "select" | "url-test" | "fallback" | "load-balance" | "relay"
    )
}

pub fn parse_uri_list(content: &str, subscription_id: i64) -> Vec<ProxyNode> {
    content
        .lines()
        .filter_map(|line| parse_uri_node(line.trim(), subscription_id))
        .collect()
}

fn parse_uri_node(uri: &str, subscription_id: i64) -> Option<ProxyNode> {
    if uri.is_empty() || uri.starts_with('#') {
        return None;
    }

    if let Some(rest) = uri.strip_prefix("socks5://") {
        return parse_host_uri(rest, subscription_id, uri, NodeType::Socks5, 1080);
    }

    if let Some(rest) = uri.strip_prefix("http://") {
        return parse_host_uri(rest, subscription_id, uri, NodeType::Http, 8080);
    }

    if let Some(rest) = uri.strip_prefix("trojan://") {
        return parse_trojan_uri(rest, subscription_id, uri);
    }

    if let Some(rest) = uri.strip_prefix("vless://") {
        return parse_vless_uri(rest, subscription_id, uri);
    }

    None
}

fn parse_host_uri(
    rest: &str,
    subscription_id: i64,
    raw: &str,
    node_type: NodeType,
    default_port: u16,
) -> Option<ProxyNode> {
    let (credentials, host_port) = rest
        .rsplit_once('@')
        .map_or((None, rest), |(left, right)| (Some(left), right));
    let (server, port) = split_host_port(host_port, default_port)?;
    let (username, password) = parse_credentials(credentials);
    let label = match node_type {
        NodeType::Http => "HTTP",
        NodeType::Socks5 => "SOCKS5",
        NodeType::Socks4 => "SOCKS4",
        NodeType::Trojan => "TROJAN",
        NodeType::Unknown(_) => "PROXY",
    };

    Some(ProxyNode {
        id: 0,
        subscription_id,
        name: format!("{label}-{server}-{port}"),
        raw: raw.to_string(),
        node_type,
        server,
        port,
        username,
        password,
        enabled: true,
        created_at: 0,
    })
}

fn parse_trojan_uri(rest: &str, subscription_id: i64, _raw_uri: &str) -> Option<ProxyNode> {
    let (without_fragment, fragment) =
        rest.split_once('#').map_or((rest, None), |(left, right)| {
            (left, Some(percent_decode(right)))
        });
    let (authority, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, ""), |(left, right)| (left, right));
    let (password, host_port) = authority.rsplit_once('@')?;
    let (server, port) = split_host_port(host_port, 443)?;
    let params = parse_query_params(query);
    let name = fragment
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("TROJAN-{server}-{port}"));
    let sni = params.get("sni").or_else(|| params.get("peer")).cloned();
    let skip_cert = params
        .get("allowInsecure")
        .or_else(|| params.get("allow_insecure"))
        .or_else(|| params.get("skip-cert-verify"))
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "True" | "TRUE"));

    let mut raw = format!(
        "name: {}\ntype: trojan\nserver: {}\nport: {}\npassword: {}\n",
        yaml_scalar(&name),
        yaml_scalar(&server),
        port,
        yaml_scalar(&percent_decode(password))
    );
    if let Some(sni) = &sni {
        raw.push_str(&format!("sni: {}\n", yaml_scalar(sni)));
    }
    if skip_cert {
        raw.push_str("skip-cert-verify: true\n");
    }

    Some(ProxyNode {
        id: 0,
        subscription_id,
        name,
        raw,
        node_type: NodeType::Trojan,
        server,
        port,
        username: None,
        password: Some(percent_decode(password)),
        enabled: true,
        created_at: 0,
    })
}

fn parse_vless_uri(rest: &str, subscription_id: i64, _raw_uri: &str) -> Option<ProxyNode> {
    let (without_fragment, fragment) =
        rest.split_once('#').map_or((rest, None), |(left, right)| {
            (left, Some(percent_decode(right)))
        });
    let (authority, query) = without_fragment
        .split_once('?')
        .map_or((without_fragment, ""), |(left, right)| (left, right));
    let (uuid, host_port) = authority.rsplit_once('@')?;
    let (server, port) = split_host_port(host_port, 443)?;
    let params = parse_query_params(query);
    let name = fragment
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("VLESS-{server}-{port}"));
    let uuid = percent_decode(uuid);

    let mut raw = format!(
        "name: {}\ntype: vless\nserver: {}\nport: {}\nuuid: {}\n",
        yaml_scalar(&name),
        yaml_scalar(&server),
        port,
        yaml_scalar(&uuid)
    );

    if let Some(network) = query_value(&params, "type") {
        raw.push_str(&format!("network: {}\n", yaml_scalar(network)));
        match network {
            "ws" => append_ws_opts(&mut raw, &params),
            "grpc" => append_grpc_opts(&mut raw, &params),
            _ => {}
        }
    }
    if let Some(flow) = query_value(&params, "flow") {
        raw.push_str(&format!("flow: {}\n", yaml_scalar(flow)));
    }
    if let Some(servername) = first_query_value(&params, &["sni", "peer"]) {
        raw.push_str(&format!("servername: {}\n", yaml_scalar(servername)));
    }
    if matches!(query_value(&params, "security"), Some("tls" | "reality")) {
        raw.push_str("tls: true\n");
    }
    if let Some(fingerprint) = query_value(&params, "fp") {
        raw.push_str(&format!(
            "client-fingerprint: {}\n",
            yaml_scalar(fingerprint)
        ));
    }
    append_reality_opts(&mut raw, &params);

    Some(ProxyNode {
        id: 0,
        subscription_id,
        name,
        raw,
        node_type: NodeType::Unknown("vless".to_string()),
        server,
        port,
        username: None,
        password: Some(uuid),
        enabled: true,
        created_at: 0,
    })
}

fn query_value<'a>(params: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

fn first_query_value<'a>(params: &'a HashMap<String, String>, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| query_value(params, key))
}

fn append_ws_opts(raw: &mut String, params: &HashMap<String, String>) {
    let path = query_value(params, "path");
    let host = first_query_value(params, &["host", "Host"]);
    if path.is_none() && host.is_none() {
        return;
    }

    raw.push_str("ws-opts:\n");
    if let Some(path) = path {
        raw.push_str(&format!("  path: {}\n", yaml_scalar(path)));
    }
    if let Some(host) = host {
        raw.push_str("  headers:\n");
        raw.push_str(&format!("    Host: {}\n", yaml_scalar(host)));
    }
}

fn append_grpc_opts(raw: &mut String, params: &HashMap<String, String>) {
    let Some(service_name) = first_query_value(
        params,
        &["serviceName", "service-name", "grpc-service-name"],
    ) else {
        return;
    };

    raw.push_str("grpc-opts:\n");
    raw.push_str(&format!(
        "  grpc-service-name: {}\n",
        yaml_scalar(service_name)
    ));
}

fn append_reality_opts(raw: &mut String, params: &HashMap<String, String>) {
    let public_key = first_query_value(params, &["pbk", "public-key"]);
    let short_id = first_query_value(params, &["sid", "short-id"]);
    let spider_x = first_query_value(params, &["spx", "spiderX", "spider-x"]);
    if public_key.is_none() && short_id.is_none() && spider_x.is_none() {
        return;
    }

    raw.push_str("reality-opts:\n");
    if let Some(public_key) = public_key {
        raw.push_str(&format!("  public-key: {}\n", yaml_scalar(public_key)));
    }
    if let Some(short_id) = short_id {
        raw.push_str(&format!("  short-id: {}\n", yaml_scalar(short_id)));
    }
    if let Some(spider_x) = spider_x {
        raw.push_str(&format!("  spider-x: {}\n", yaml_scalar(spider_x)));
    }
}

fn split_host_port(input: &str, default_port: u16) -> Option<(String, u16)> {
    let trimmed = input.split(['?', '#']).next().unwrap_or(input);
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('[') {
        let end = trimmed.find(']')?;
        let host = &trimmed[1..end];
        if host.is_empty() {
            return None;
        }
        let rest = &trimmed[end + 1..];
        let port = if rest.is_empty() {
            default_port
        } else {
            parse_port(rest.strip_prefix(':')?)?
        };
        return Some((host.to_string(), port));
    }

    match trimmed.matches(':').count() {
        0 => Some((trimmed.to_string(), default_port)),
        1 => {
            let (host, port) = trimmed.rsplit_once(':')?;
            if host.is_empty() {
                return None;
            }
            Some((host.to_string(), parse_port(port)?))
        }
        _ => Some((trimmed.to_string(), default_port)),
    }
}

fn parse_port(value: &str) -> Option<u16> {
    value.parse::<u16>().ok().filter(|port| *port != 0)
}

fn parse_credentials(credentials: Option<&str>) -> (Option<String>, Option<String>) {
    match credentials.and_then(|value| value.split_once(':')) {
        Some((username, password)) => (Some(username.to_string()), Some(password.to_string())),
        None => (credentials.map(str::to_string), None),
    }
}

fn parse_query_params(query: &str) -> std::collections::HashMap<String, String> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .filter_map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (!key.is_empty()).then(|| (percent_decode(key), percent_decode(value)))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(hex) = std::str::from_utf8(&bytes[index + 1..index + 3]) {
                if let Ok(decoded) = u8::from_str_radix(hex, 16) {
                    output.push(decoded);
                    index += 3;
                    continue;
                }
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn yaml_scalar(value: &str) -> String {
    let rendered = serde_yaml::to_string(value).unwrap_or_else(|_| format!("'{value}'"));
    rendered
        .trim()
        .strip_prefix("---\n")
        .unwrap_or(rendered.trim())
        .trim_end_matches("\n...")
        .to_string()
}

fn looks_like_clash_yaml(content: &str) -> bool {
    content.contains("proxies:") || (content.contains("name:") && content.contains("server:"))
}

fn decode_base64_subscription(content: &str) -> Option<String> {
    let compact: String = content.lines().map(str::trim).collect();
    if compact.len() < 8 || !compact.chars().all(is_base64_char) {
        return None;
    }

    general_purpose::STANDARD
        .decode(compact.as_bytes())
        .or_else(|_| general_purpose::STANDARD_NO_PAD.decode(compact.as_bytes()))
        .or_else(|_| general_purpose::URL_SAFE.decode(compact.as_bytes()))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

fn is_base64_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '-' | '_' | '=')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clash_yaml() {
        let yaml = r#"
proxies:
  - name: Test HTTP
    type: http
    server: 1.2.3.4
    port: 8080
    username: user
    password: pass
"#;
        let nodes = parse_subscription_content(yaml, 7).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].subscription_id, 7);
        assert_eq!(nodes[0].node_type, NodeType::Http);
        assert!(nodes[0].raw.contains("Test HTTP"));
    }

    #[test]
    fn ignores_proxy_groups_in_mixed_clash_proxy_lists() {
        let yaml = r#"
proxies:
  - name: Real Node
    type: vless
    server: proxy.example.com
    port: 443
    uuid: 00000000-0000-4000-8000-000000000000
  - name: Select Group
    type: select
    proxies: [Real Node, DIRECT]
  - name: Auto Group
    type: url-test
    proxies: [Real Node]
    url: http://www.gstatic.com/generate_204
  - name: Missing Endpoint
    type: vless
    proxies: [Real Node]
  - name: Invalid Port
    type: vless
    server: proxy.example.com
    port: 0
proxy-groups:
  - name: Top-level Group
    type: fallback
    proxies: [Real Node]
"#;

        let nodes = parse_subscription_content(yaml, 8).unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "Real Node");
        assert_eq!(nodes[0].server, "proxy.example.com");
        assert_eq!(nodes[0].port, 443);
    }

    #[test]
    fn parses_plain_uri_list() {
        let content = "socks5://user:pass@10.11.12.13:1080\nhttp://proxy.example.com:8080";
        let nodes = parse_subscription_content(content, 1).unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].server, "10.11.12.13");
        assert_eq!(nodes[0].username.as_deref(), Some("user"));
        assert_eq!(nodes[1].node_type, NodeType::Http);
    }

    #[test]
    fn parses_base64_uri_list() {
        let encoded = general_purpose::STANDARD.encode("http://proxy.example.com:8080");
        let nodes = parse_subscription_content(&encoded, 1).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].server, "proxy.example.com");
    }

    #[test]
    fn parses_base64_trojan_uri_list() {
        let uri =
            "trojan://secret@example.com:443?allowInsecure=1&sni=www.example.com#Test%20Trojan";
        let encoded = general_purpose::STANDARD.encode(uri);
        let nodes = parse_subscription_content(&encoded, 9).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].subscription_id, 9);
        assert_eq!(nodes[0].node_type, NodeType::Trojan);
        assert_eq!(nodes[0].name, "Test Trojan");
        assert_eq!(nodes[0].server, "example.com");
        assert_eq!(nodes[0].port, 443);
        assert_eq!(nodes[0].password.as_deref(), Some("secret"));
        assert!(nodes[0].raw.contains("type: trojan"));
        assert!(nodes[0].raw.contains("sni: www.example.com"));
        assert!(nodes[0].raw.contains("skip-cert-verify: true"));
    }

    #[test]
    fn parses_base64_vless_uri_list() {
        let uri = "vless://00000000-0000-4000-8000-000000000000@example.com:443?type=tcp&security=reality&flow=xtls-rprx-vision&fp=chrome&sni=www.example.com&pbk=public-key&sid=1234abcd&spx=%2Ffoo#Test%20VLESS";
        let encoded = general_purpose::STANDARD.encode(uri);
        let nodes = parse_subscription_content(&encoded, 11).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].subscription_id, 11);
        assert_eq!(nodes[0].node_type, NodeType::Unknown("vless".to_string()));
        assert_eq!(nodes[0].name, "Test VLESS");
        assert_eq!(nodes[0].server, "example.com");
        assert_eq!(nodes[0].port, 443);
        assert_eq!(
            nodes[0].password.as_deref(),
            Some("00000000-0000-4000-8000-000000000000")
        );
        assert!(nodes[0].raw.contains("type: vless"));
        assert!(nodes[0]
            .raw
            .contains("uuid: 00000000-0000-4000-8000-000000000000"));
        assert!(nodes[0].raw.contains("network: tcp"));
        assert!(nodes[0].raw.contains("flow: xtls-rprx-vision"));
        assert!(nodes[0].raw.contains("servername: www.example.com"));
        assert!(nodes[0].raw.contains("tls: true"));
        assert!(nodes[0].raw.contains("client-fingerprint: chrome"));
        assert!(nodes[0].raw.contains("reality-opts:"));
        assert!(nodes[0].raw.contains("public-key: public-key"));
        assert!(nodes[0].raw.contains("short-id: 1234abcd"));
        assert!(nodes[0].raw.contains("spider-x: /foo"));
    }

    #[test]
    fn parses_vless_websocket_options() {
        let uri = "vless://00000000-0000-4000-8000-000000000000@example.com:443?type=ws&security=tls&host=cdn.example.com&path=%2Fvless#WS";
        let nodes = parse_subscription_content(uri, 12).unwrap();

        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].raw.contains("network: ws"));
        assert!(nodes[0].raw.contains("ws-opts:"));
        assert!(nodes[0].raw.contains("path: /vless"));
        assert!(nodes[0].raw.contains("headers:"));
        assert!(nodes[0].raw.contains("Host: cdn.example.com"));
    }

    #[test]
    fn parses_vless_grpc_options() {
        let uri = "vless://00000000-0000-4000-8000-000000000000@example.com:443?type=grpc&security=tls&serviceName=my-service#GRPC";
        let nodes = parse_subscription_content(uri, 13).unwrap();

        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].raw.contains("network: grpc"));
        assert!(nodes[0].raw.contains("grpc-opts:"));
        assert!(nodes[0].raw.contains("grpc-service-name: my-service"));
    }

    #[test]
    fn rejects_nodes_with_invalid_explicit_ports() {
        let nodes = parse_subscription_content(
            "vless://00000000-0000-4000-8000-000000000000@example.com:65536#bad",
            14,
        )
        .unwrap();

        assert!(nodes.is_empty());
    }

    #[test]
    fn preserves_unbracketed_ipv6_hosts_without_ports() {
        let nodes = parse_subscription_content(
            "vless://00000000-0000-4000-8000-000000000000@2001:db8::1?security=tls#ipv6",
            15,
        )
        .unwrap();

        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].server, "2001:db8::1");
        assert_eq!(nodes[0].port, 443);
    }
}
