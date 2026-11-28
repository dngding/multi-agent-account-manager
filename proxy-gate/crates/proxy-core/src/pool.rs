use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};

use crate::CoreError;

const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STATIC_PROXY_CHAIN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolEntry {
    pub id: String,
    pub label: String,
    pub proxy_url: String,
    pub fingerprint: String,
    pub duplicate_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolSnapshot {
    pub source_path: PathBuf,
    pub entries: Vec<PoolEntry>,
    pub revision: String,
}

pub fn load_pool(path: &Path) -> Result<PoolSnapshot, CoreError> {
    let canonical = fs::canonicalize(path)
        .map_err(|_| CoreError::InvalidPool("代理池来源不存在或不可读取".to_string()))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|_| CoreError::InvalidPool("代理池来源不可读取".to_string()))?;
    if !metadata.is_file() {
        return Err(CoreError::InvalidPool(
            "代理池来源必须是 YAML 文件".to_string(),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_SOURCE_BYTES as u64 {
        return Err(CoreError::InvalidPool(
            "代理池 YAML 为空或超过 4 MiB".to_string(),
        ));
    }
    let bytes = fs::read(&canonical)?;
    let mut snapshot = parse_pool_bytes(&bytes)?;
    snapshot.source_path = canonical;
    Ok(snapshot)
}

pub fn parse_pool_bytes(bytes: &[u8]) -> Result<PoolSnapshot, CoreError> {
    if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
        return Err(CoreError::InvalidPool(
            "代理池 YAML 为空或超过 4 MiB".to_string(),
        ));
    }

    let mut documents = serde_yaml::Deserializer::from_slice(bytes);
    let document = documents
        .next()
        .ok_or_else(|| CoreError::InvalidPool("代理池 YAML 无效".to_string()))?;
    let root = Value::deserialize(document)
        .map_err(|_| CoreError::InvalidPool("代理池 YAML 结构无效".to_string()))?;
    if documents.next().is_some() {
        return Err(CoreError::InvalidPool(
            "代理池来源只能包含一个 YAML 文档".to_string(),
        ));
    }
    let root = root
        .as_mapping()
        .ok_or_else(|| CoreError::InvalidPool("代理池顶层必须是配置对象".to_string()))?;
    if authentication_configured(get(root, "authentication")) {
        return Err(CoreError::InvalidPool(
            "代理池启用了入站认证，当前仅支持无认证本机入口".to_string(),
        ));
    }

    let proxies = proxy_nodes(root)?;
    let dynamic_targets = dynamic_proxy_names(root)?;
    let listeners = sequence(root, "listeners").unwrap_or(&[]);
    let mut raw_entries = Vec::new();
    let mut listener_ports = BTreeSet::new();

    for (index, listener) in listeners.iter().enumerate() {
        let listener = listener.as_mapping().ok_or_else(|| {
            CoreError::InvalidPool(format!("第 {} 个 listener 不是配置对象", index + 1))
        })?;
        let label = required_string(listener, "name")?;
        let listener_type = required_string(listener, "type")?.to_ascii_lowercase();
        let port = required_port(listener, "port")?;
        if !listener_ports.insert(port) {
            return Err(CoreError::InvalidPool(format!(
                "代理池存在重复 listener 端口: {port}"
            )));
        }
        let protocol = match listener_type.as_str() {
            "http" | "mixed" => "http",
            "socks" | "socks5" => "socks5",
            _ => continue,
        };
        if listener_requires_auth(listener) {
            continue;
        }
        let raw_host =
            optional_string(listener, "listen")?.unwrap_or_else(|| "0.0.0.0".to_string());
        let Some(host) = normalize_listener_host(&raw_host) else {
            continue;
        };
        let target_name = required_string(listener, "proxy")?;
        let chain = resolve_static_chain(&target_name, &proxies, &dynamic_targets)?;
        let fingerprint = chain_fingerprint(&chain)?;
        let id = format!("{protocol}:{port}");
        let label = format!("{label} · {target_name} · {port}");
        raw_entries.push(PoolEntry {
            id,
            label,
            proxy_url: format!("{protocol}://{host}:{port}"),
            fingerprint,
            duplicate_count: 1,
        });
    }

    let mut counts = BTreeMap::<String, usize>::new();
    for entry in &raw_entries {
        *counts.entry(entry.fingerprint.clone()).or_default() += 1;
    }
    for entry in &mut raw_entries {
        entry.duplicate_count = *counts.get(&entry.fingerprint).unwrap_or(&1);
    }

    let mut revision_input = raw_entries
        .iter()
        .map(|entry| format!("{}\n{}", entry.id, entry.fingerprint))
        .collect::<Vec<_>>();
    revision_input.sort();

    Ok(PoolSnapshot {
        source_path: PathBuf::new(),
        entries: raw_entries,
        revision: sha256_hex(revision_input.join("\n").as_bytes()),
    })
}

pub fn discover_sources() -> Result<Vec<PathBuf>, CoreError> {
    let Some(profile) = std::env::var_os("USERPROFILE").map(PathBuf::from) else {
        return Ok(Vec::new());
    };
    let workbuddy = profile.join("WorkBuddy");
    let entries = match fs::read_dir(&workbuddy) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(CoreError::Io(error.to_string())),
    };

    let mut discovered = BTreeSet::new();
    for (index, dir) in entries.enumerate() {
        if index > 2048 {
            return Err(CoreError::InvalidPool(
                "WorkBuddy 目录过多，请直接指定 sidecar.yaml".to_string(),
            ));
        }
        let dir = match dir {
            Ok(dir) => dir,
            Err(_) => continue,
        };
        if !dir.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
            || !is_date_directory(&dir.file_name().to_string_lossy())
        {
            continue;
        }
        let candidate = dir
            .path()
            .join("telegram-panel")
            .join("flclash-sidecar")
            .join("sidecar.yaml");
        if candidate.is_file() {
            if let Ok(canonical) = fs::canonicalize(candidate) {
                discovered.insert(canonical);
            }
        }
    }
    Ok(discovered.into_iter().collect())
}

fn proxy_nodes(root: &Mapping) -> Result<BTreeMap<String, Value>, CoreError> {
    let mut nodes = BTreeMap::new();
    for (index, value) in sequence(root, "proxies").unwrap_or(&[]).iter().enumerate() {
        let mapping = value.as_mapping().ok_or_else(|| {
            CoreError::InvalidPool(format!("第 {} 个 proxy 不是配置对象", index + 1))
        })?;
        let name = required_string(mapping, "name")?;
        if !static_node_definition(value) {
            continue;
        }
        if nodes.insert(name.clone(), value.clone()).is_some() {
            return Err(CoreError::InvalidPool(format!("proxy 名称重复: {name}")));
        }
    }
    Ok(nodes)
}

fn dynamic_proxy_names(root: &Mapping) -> Result<BTreeSet<String>, CoreError> {
    let mut names = BTreeSet::new();
    for section in ["proxy-groups", "proxy-providers"] {
        let Some(value) = get(root, section) else {
            continue;
        };
        if let Some(group_list) = value.as_sequence() {
            for group in group_list {
                if let Some(group) = group.as_mapping() {
                    if let Some(name) = optional_string(group, "name")? {
                        names.insert(name);
                    }
                }
            }
        } else if let Some(provider_map) = value.as_mapping() {
            for key in provider_map.keys() {
                if let Some(name) = key.as_str() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    Ok(names)
}

fn resolve_static_chain(
    initial_name: &str,
    proxies: &BTreeMap<String, Value>,
    dynamic_targets: &BTreeSet<String>,
) -> Result<Vec<(String, Value)>, CoreError> {
    let mut chain = Vec::new();
    let mut seen = BTreeSet::new();
    let mut current = initial_name.to_string();

    loop {
        if dynamic_targets.contains(&current) {
            return Err(CoreError::InvalidPool(format!(
                "listener 目标 {current} 是动态代理组或 provider"
            )));
        }
        if !seen.insert(current.clone()) {
            return Err(CoreError::InvalidPool(format!(
                "dialer-proxy 存在循环引用: {current}"
            )));
        }
        if chain.len() >= MAX_STATIC_PROXY_CHAIN {
            return Err(CoreError::InvalidPool(format!(
                "静态 dialer-proxy 超过 {MAX_STATIC_PROXY_CHAIN} 层"
            )));
        }
        let node = proxies.get(&current).ok_or_else(|| {
            CoreError::InvalidPool(format!("listener 目标 {current} 不是静态 proxy"))
        })?;
        chain.push((current.clone(), node.clone()));

        let Some(node_map) = node.as_mapping() else {
            return Err(CoreError::InvalidPool(
                "静态 proxy 定义必须是对象".to_string(),
            ));
        };
        let Some(next) = optional_string(node_map, "dialer-proxy")? else {
            break;
        };
        let next = next.trim().to_string();
        if next.is_empty()
            || matches!(
                next.to_ascii_uppercase().as_str(),
                "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE"
            )
        {
            return Err(CoreError::InvalidPool(format!(
                "dialer-proxy 指向不支持的特殊目标: {next}"
            )));
        }
        current = next;
    }

    Ok(chain)
}

fn chain_fingerprint(chain: &[(String, Value)]) -> Result<String, CoreError> {
    let value = serde_json::to_value(
        chain
            .iter()
            .map(|(name, node)| serde_json::json!({"name": name, "definition": node}))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| CoreError::InvalidPool("无法生成代理目标指纹".to_string()))?;
    let canonical = canonical_json(&value);
    Ok(sha256_hex(canonical.as_bytes()))
}

fn canonical_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            let body = keys
                .into_iter()
                .map(|key| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(key).unwrap_or_default(),
                        canonical_json(map.get(key).expect("sorted key must exist"))
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!("{{{body}}}")
        }
        serde_json::Value::Array(values) => format!(
            "[{}]",
            values
                .iter()
                .map(canonical_json)
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => serde_json::to_string(value).unwrap_or_else(|_| "null".to_string()),
    }
}

fn listener_requires_auth(listener: &Mapping) -> bool {
    for key in ["username", "password", "token"] {
        if optional_string(listener, key)
            .ok()
            .flatten()
            .is_some_and(|value| !value.is_empty())
        {
            return true;
        }
    }
    authentication_configured(get(listener, "authentication"))
        || authentication_configured(get(listener, "users"))
}

fn authentication_configured(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Sequence(values)) => !values.is_empty(),
        Some(Value::Mapping(values)) => !values.is_empty(),
        Some(Value::String(value)) => !value.trim().is_empty(),
        Some(_) => true,
    }
}

fn static_node_definition(value: &Value) -> bool {
    let Some(node) = value.as_mapping() else {
        return false;
    };
    let Some(kind) = get(node, "type").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(
        kind,
        "ss" | "ssr"
            | "vmess"
            | "vless"
            | "trojan"
            | "snell"
            | "tuic"
            | "hysteria"
            | "hysteria2"
            | "wireguard"
            | "socks5"
            | "http"
            | "ssh"
            | "anytls"
            | "shadowtls"
            | "mieru"
    ) {
        return false;
    }
    if ["proxy", "detour", "use", "proxies", "proxy-providers"]
        .iter()
        .any(|field| get(node, field).is_some_and(|value| !value.is_null()))
    {
        return false;
    }
    get(node, "server")
        .and_then(Value::as_str)
        .is_some_and(|server| !server.trim().is_empty())
        && get(node, "port").is_some_and(valid_port)
}

fn valid_port(value: &Value) -> bool {
    match value {
        Value::Number(number) => number
            .as_u64()
            .is_some_and(|port| (1..=u16::MAX as u64).contains(&port)),
        Value::String(value) => value.parse::<u16>().is_ok_and(|port| port > 0),
        _ => false,
    }
}

fn normalize_listener_host(raw: &str) -> Option<String> {
    if raw.eq_ignore_ascii_case("localhost") {
        return Some("127.0.0.1".to_string());
    }
    let address = raw
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok()?;
    if address.is_unspecified() {
        return Some(match address {
            IpAddr::V4(_) => "127.0.0.1".to_string(),
            IpAddr::V6(_) => "[::1]".to_string(),
        });
    }
    if !address.is_loopback() {
        return None;
    }
    Some(match address {
        IpAddr::V4(address) => address.to_string(),
        IpAddr::V6(address) => format!("[{address}]"),
    })
}

fn required_string(mapping: &Mapping, key: &str) -> Result<String, CoreError> {
    optional_string(mapping, key)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| CoreError::InvalidPool(format!("缺少字符串字段 {key}")))
}

fn optional_string(mapping: &Mapping, key: &str) -> Result<Option<String>, CoreError> {
    match get(mapping, key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Number(value)) => Ok(Some(value.to_string())),
        Some(_) => Err(CoreError::InvalidPool(format!("字段 {key} 必须是字符串"))),
    }
}

fn required_port(mapping: &Mapping, key: &str) -> Result<u16, CoreError> {
    let raw =
        get(mapping, key).ok_or_else(|| CoreError::InvalidPool(format!("缺少端口字段 {key}")))?;
    let port = match raw {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    }
    .and_then(|value| u16::try_from(value).ok())
    .filter(|value| *value > 0)
    .ok_or_else(|| CoreError::InvalidPool(format!("字段 {key} 不是有效端口")))?;
    Ok(port)
}

fn sequence<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a [Value]> {
    get(mapping, key)
        .and_then(Value::as_sequence)
        .map(Vec::as_slice)
}

fn get<'a>(mapping: &'a Mapping, key: &str) -> Option<&'a Value> {
    mapping
        .iter()
        .find_map(|(candidate, value)| (candidate.as_str() == Some(key)).then_some(value))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn is_date_directory(name: &str) -> bool {
    let parts = name.split('-').collect::<Vec<_>>();
    matches!(parts.len(), 3 | 6)
        && parts.first().is_some_and(|year| year.len() == 4)
        && parts.iter().enumerate().all(|(index, part)| {
            (index == 0 || part.len() == 2) && part.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use super::parse_pool_bytes;
    use crate::CoreError;

    fn source(listener_proxy: &str, extra_proxy: &str) -> String {
        format!(
            r#"
proxies:
  - {{name: fixed-a, type: ss, server: a.example, port: 443, cipher: aes-128-gcm, password: one, dialer-proxy: fixed-b}}
  - {{name: fixed-b, type: ss, server: b.example, port: 8443, cipher: aes-128-gcm, password: two}}
{extra_proxy}
listeners:
  - {{name: local-a, type: socks, listen: 0.0.0.0, port: 18081, proxy: {listener_proxy}}}
"#
        )
    }

    #[test]
    fn parses_static_chain_and_normalizes_loopback() {
        let pool = parse_pool_bytes(source("fixed-a", "").as_bytes()).unwrap();
        let entry = &pool.entries[0];
        assert_eq!(entry.proxy_url, "socks5://127.0.0.1:18081");
        assert_eq!(entry.duplicate_count, 1);
        assert_eq!(entry.fingerprint.len(), 64);
    }

    #[test]
    fn rejects_dynamic_missing_and_cyclic_targets() {
        let dynamic = r#"
proxy-groups:
  - {name: auto-group, type: select, proxies: [fixed-a]}
"#;
        assert!(parse_pool_bytes(source("auto-group", dynamic).as_bytes()).is_err());
        assert!(parse_pool_bytes(source("missing", "").as_bytes()).is_err());

        let cycle = r#"
proxies:
  - {name: fixed-a, type: ss, server: a.example, port: 443, cipher: aes-128-gcm, password: one, dialer-proxy: fixed-b}
  - {name: fixed-b, type: ss, server: b.example, port: 443, cipher: aes-128-gcm, password: two, dialer-proxy: fixed-a}
listeners:
  - {name: local-a, type: http, port: 18081, proxy: fixed-a}
"#;
        assert!(parse_pool_bytes(cycle.as_bytes()).is_err());
    }

    #[test]
    fn rejects_authentication_and_remote_listeners() {
        let authenticated = source("fixed-a", "").replace(
            "listen: 0.0.0.0",
            "listen: 0.0.0.0, authentication: [\"user:pass\"]",
        );
        assert!(parse_pool_bytes(authenticated.as_bytes())
            .unwrap()
            .entries
            .is_empty());

        let remote = source("fixed-a", "").replace("listen: 0.0.0.0", "listen: 192.168.1.8");
        assert!(parse_pool_bytes(remote.as_bytes())
            .unwrap()
            .entries
            .is_empty());
    }

    #[test]
    fn fingerprints_each_hop_and_counts_duplicate_targets() {
        let first = parse_pool_bytes(source("fixed-a", "").as_bytes()).unwrap();
        let changed = source("fixed-a", "")
            .replace("password: two", "password: changed")
            .as_bytes()
            .to_vec();
        let second = parse_pool_bytes(&changed).unwrap();
        assert_ne!(first.entries[0].fingerprint, second.entries[0].fingerprint);

        let duplicated = source("fixed-a", "").replace(
            "listeners:",
            "listeners:\n  - {name: local-b, type: http, listen: 127.0.0.1, port: 18082, proxy: fixed-a}",
        );
        let pool = parse_pool_bytes(duplicated.as_bytes()).unwrap();
        assert_eq!(pool.entries.len(), 2);
        assert!(pool.entries.iter().all(|entry| entry.duplicate_count == 2));
    }

    #[test]
    fn invalid_pool_errors_are_specific() {
        let error = parse_pool_bytes(b"").unwrap_err();
        assert!(matches!(error, CoreError::InvalidPool(_)));
    }
}
