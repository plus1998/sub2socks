use serde::{Deserialize, Serialize};

/// The protocol type of a proxy node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    Http,
    Socks5,
    Socks4,
    Trojan,
    Unknown(String),
}

impl Serialize for NodeType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for NodeType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(|value| Self::parse(&value))
    }
}

impl NodeType {
    pub fn as_str(&self) -> &str {
        match self {
            NodeType::Http => "http",
            NodeType::Socks5 => "socks5",
            NodeType::Socks4 => "socks4",
            NodeType::Trojan => "trojan",
            NodeType::Unknown(s) => s.as_str(),
        }
    }

    pub fn parse(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "http" => NodeType::Http,
            "socks5" | "socks" => NodeType::Socks5,
            "socks4" => NodeType::Socks4,
            "trojan" => NodeType::Trojan,
            other => NodeType::Unknown(other.to_string()),
        }
    }
}

impl std::str::FromStr for NodeType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

#[cfg(test)]
mod node_type_tests {
    use super::*;

    #[test]
    fn serializes_known_and_dynamic_types_as_strings() {
        assert_eq!(serde_json::to_string(&NodeType::Http).unwrap(), "\"http\"");
        assert_eq!(
            serde_json::to_string(&NodeType::Unknown("vless".to_string())).unwrap(),
            "\"vless\""
        );
    }

    #[test]
    fn deserializes_dynamic_types_from_strings() {
        assert_eq!(
            serde_json::from_str::<NodeType>("\"vless\"").unwrap(),
            NodeType::Unknown("vless".to_string())
        );
    }
}

/// One subscription entry fetched from a remote URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub last_synced_at: Option<i64>,
    pub created_at: i64,
}

/// A parsed proxy node belonging to a subscription.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyNode {
    pub id: i64,
    pub subscription_id: i64,
    pub name: String,
    pub raw: String,
    pub node_type: NodeType,
    pub server: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    pub enabled: bool,
    pub created_at: i64,
}

/// A local SOCKS account that forwards traffic to a target proxy node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SocksAccount {
    pub id: i64,
    pub name: String,
    pub username: String,
    pub password: String,
    pub node_id: i64,
    pub listen_port: u16,
    pub enabled: bool,
    pub created_at: i64,
}

/// A key-value runtime setting persisted in the database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeSetting {
    pub key: String,
    pub value: String,
}

/// Payload for the first-run initialization endpoint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitRequest {
    pub admin_user: String,
    pub admin_pass: String,
}
