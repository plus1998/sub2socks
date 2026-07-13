use std::collections::{HashMap, HashSet};

use serde_yaml::{Mapping, Value};

use crate::types::{NodeType, ProxyNode, SocksAccount};

const DEFAULT_MIXED_PORT: u16 = 7890;
const DEFAULT_CONTROLLER: &str = "127.0.0.1:9090";
const DEFAULT_SECRET: &str = "rust-proxy-manager";
const DEFAULT_GROUP: &str = "All Nodes";

pub fn generate_mihomo_config(
    nodes: &[ProxyNode],
    accounts: &[SocksAccount],
) -> Result<String, String> {
    let enabled_nodes: Vec<&ProxyNode> = nodes.iter().filter(|node| node.enabled).collect();
    let mut used_names = HashSet::new();
    let mut node_tags = HashMap::new();
    let mut proxy_values = Vec::new();

    for node in enabled_nodes {
        let tag = unique_tag(&node.name, node.id, &mut used_names);
        node_tags.insert(node.id, tag.clone());
        proxy_values.push(proxy_yaml(node, &tag)?);
    }

    let mut root = Mapping::new();
    insert_str(&mut root, "bind-address", "0.0.0.0");
    insert_bool(&mut root, "allow-lan", true);
    insert_u16(&mut root, "mixed-port", DEFAULT_MIXED_PORT);
    insert_str(&mut root, "mode", "rule");
    insert_str(&mut root, "log-level", "info");
    insert_str(&mut root, "external-controller", DEFAULT_CONTROLLER);
    insert_str(&mut root, "secret", DEFAULT_SECRET);
    root.insert(
        Value::String("proxies".to_string()),
        Value::Sequence(proxy_values),
    );

    if !node_tags.is_empty() {
        let mut group = Mapping::new();
        insert_str(&mut group, "name", DEFAULT_GROUP);
        insert_str(&mut group, "type", "select");
        group.insert(
            Value::String("proxies".to_string()),
            Value::Sequence(
                node_tags
                    .values()
                    .cloned()
                    .map(Value::String)
                    .collect::<Vec<_>>(),
            ),
        );
        root.insert(
            Value::String("proxy-groups".to_string()),
            Value::Sequence(vec![Value::Mapping(group)]),
        );
    }

    let listeners = accounts
        .iter()
        .filter(|account| account.enabled)
        .filter_map(|account| listener_yaml(account, &node_tags))
        .collect::<Vec<_>>();
    root.insert(
        Value::String("listeners".to_string()),
        Value::Sequence(listeners),
    );

    let final_rule = if node_tags.is_empty() {
        "MATCH,DIRECT".to_string()
    } else {
        format!("MATCH,{DEFAULT_GROUP}")
    };
    root.insert(
        Value::String("rules".to_string()),
        Value::Sequence(vec![Value::String(final_rule)]),
    );

    serde_yaml::to_string(&Value::Mapping(root))
        .map_err(|e| format!("failed to serialize config: {e}"))
}

fn unique_tag(name: &str, id: i64, used: &mut HashSet<String>) -> String {
    let base = if name.trim().is_empty() {
        format!("node-{id}")
    } else {
        name.trim().to_string()
    };

    if used.insert(base.clone()) {
        return base;
    }

    let tagged = format!("{base} #{id}");
    used.insert(tagged.clone());
    tagged
}

fn proxy_yaml(node: &ProxyNode, tag: &str) -> Result<Value, String> {
    let mut mapping = parse_raw_proxy_mapping(&node.raw).unwrap_or_default();
    mapping.insert(
        Value::String("name".to_string()),
        Value::String(tag.to_string()),
    );

    if !mapping.contains_key(Value::String("type".to_string())) {
        insert_str(&mut mapping, "type", node.node_type.as_str());
    }
    if !mapping.contains_key(Value::String("server".to_string())) {
        insert_str(&mut mapping, "server", &node.server);
    }
    if !mapping.contains_key(Value::String("port".to_string())) {
        insert_u16(&mut mapping, "port", node.port);
    }
    if !mapping.contains_key(Value::String("username".to_string())) {
        if let Some(username) = &node.username {
            insert_str(&mut mapping, "username", username);
        }
    }
    if !mapping.contains_key(Value::String("password".to_string())) {
        if let Some(password) = &node.password {
            insert_str(&mut mapping, "password", password);
        }
    }

    if matches!(node.node_type, NodeType::Unknown(_))
        && !mapping.contains_key(Value::String("type".to_string()))
    {
        return Err(format!("node {} has no supported proxy type", node.name));
    }

    Ok(Value::Mapping(mapping))
}

fn parse_raw_proxy_mapping(raw: &str) -> Option<Mapping> {
    let value: Value = serde_yaml::from_str(raw).ok()?;
    value.as_mapping().cloned()
}

fn listener_yaml(account: &SocksAccount, node_tags: &HashMap<i64, String>) -> Option<Value> {
    let target = node_tags.get(&account.node_id)?;
    let mut mapping = Mapping::new();
    insert_str(&mut mapping, "name", &account.name);
    insert_str(&mut mapping, "type", "socks");
    insert_str(&mut mapping, "listen", "127.0.0.1");
    insert_u16(&mut mapping, "port", account.listen_port);
    let mut user = Mapping::new();
    insert_str(&mut user, "username", &account.username);
    insert_str(&mut user, "password", &account.password);
    mapping.insert(
        Value::String("users".to_string()),
        Value::Sequence(vec![Value::Mapping(user)]),
    );
    insert_str(&mut mapping, "proxy", target);
    Some(Value::Mapping(mapping))
}

fn insert_str(mapping: &mut Mapping, key: &str, value: &str) {
    mapping.insert(
        Value::String(key.to_string()),
        Value::String(value.to_string()),
    );
}

fn insert_u16(mapping: &mut Mapping, key: &str, value: u16) {
    mapping.insert(
        Value::String(key.to_string()),
        Value::Number(serde_yaml::Number::from(value)),
    );
}

fn insert_bool(mapping: &mut Mapping, key: &str, value: bool) {
    mapping.insert(Value::String(key.to_string()), Value::Bool(value));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: i64, name: &str) -> ProxyNode {
        ProxyNode {
            id,
            subscription_id: 1,
            name: name.to_string(),
            raw: format!("name: {name}\ntype: http\nserver: 1.2.3.4\nport: 8080"),
            node_type: NodeType::Http,
            server: "1.2.3.4".to_string(),
            port: 8080,
            username: None,
            password: None,
            enabled: true,
            created_at: 0,
            last_tested_at: None,
            last_test_ok: None,
            last_test_latency_ms: None,
            last_test_error: None,
        }
    }

    #[test]
    fn generates_listener_per_account() {
        let nodes = vec![node(10, "node-a")];
        let accounts = vec![SocksAccount {
            id: 1,
            name: "acct".to_string(),
            username: "u".to_string(),
            password: "p".to_string(),
            node_id: 10,
            listen_port: 10801,
            enabled: true,
            created_at: 0,
        }];

        let yaml = generate_mihomo_config(&nodes, &accounts).unwrap();
        let parsed: Value = serde_yaml::from_str(&yaml).unwrap();
        let root = parsed.as_mapping().unwrap();
        assert!(root.get(Value::String("proxies".to_string())).is_some());
        assert!(yaml.contains("proxy: node-a"));
        assert!(yaml.contains("port: 10801"));
        assert!(yaml.contains("listen: 127.0.0.1"));
    }
}
