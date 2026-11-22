use crate::providers::{get_provider, ProviderId};
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::OsString;
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;
use url::Url;

const MAX_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STATE_BYTES: usize = 16 * 1024;
const MAX_YAML_NODES: usize = 200_000;
const MAX_YAML_DEPTH: usize = 48;
const MAX_LISTENERS: usize = 4096;
const SOURCE_STATE_FILE: &str = "proxy-pool-source.json";
const SOURCE_OVERRIDE: &str = "DIEHAO_PROXY_POOL_FILE";
const HEALTH_CACHE_TTL: Duration = Duration::from_secs(120);
const HEALTH_CACHE_RETENTION: Duration = Duration::from_secs(600);
const HEALTH_PROBE_CONCURRENCY: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyPoolBinding {
  pub source_path: String,
  pub entry_id: String,
  pub fingerprint: String,
  pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolEntry {
  pub id: String,
  pub label: String,
  pub proxy_url: String,
  pub fingerprint: String,
  pub duplicate_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolSnapshot {
  pub source_path: Option<String>,
  pub discovered_sources: Vec<String>,
  pub entries: Vec<ProxyPoolEntry>,
  pub revision: String,
  pub checked_at_ms: u64,
  pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolHealthEntry {
  pub entry_id: String,
  pub available: bool,
  pub handshake_only: bool,
  pub latency_ms: u64,
  pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolHealthSnapshot {
  pub provider_id: ProviderId,
  pub revision: String,
  pub checked_at_ms: u64,
  pub entries: Vec<ProxyPoolHealthEntry>,
}

#[derive(Debug, Clone)]
struct CachedProxyHealth {
  checked_at: Instant,
  result: ProxyPoolHealthEntry,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceSelection {
  version: u32,
  source_path: String,
}

#[derive(Debug)]
pub struct ProxyPool {
  data_dir: PathBuf,
  health_cache: Mutex<HashMap<String, CachedProxyHealth>>,
  probe_lock: Mutex<()>,
}

impl ProxyPool {
  pub fn new(data_dir: PathBuf) -> Self {
    Self {
      data_dir,
      health_cache: Mutex::new(HashMap::new()),
      probe_lock: Mutex::new(()),
    }
  }

  pub fn snapshot(&self) -> ProxyPoolSnapshot {
    self.snapshot_with_environment(
      std::env::var_os(SOURCE_OVERRIDE),
      std::env::var_os("USERPROFILE").map(PathBuf::from),
    )
  }

  /// Only this application's source selection is written. The pool stays read-only.
  pub fn set_source(&self, raw_path: &str) -> Result<ProxyPoolSnapshot, String> {
    if std::env::var_os(SOURCE_OVERRIDE).is_some() {
      return Err("当前进程已指定测试代理池来源，请移除环境变量后再选择来源".to_string());
    }
    self.save_source(raw_path)?;
    Ok(self.snapshot())
  }

  pub async fn probe_health(
    &self,
    provider_id: ProviderId,
    force: bool,
  ) -> Result<ProxyPoolHealthSnapshot, String> {
    #[cfg(not(windows))]
    {
      let _ = provider_id;
      let _ = force;
      return Err("代理可用性检测目前仅支持 Windows 桌面版".to_string());
    }

    #[cfg(windows)]
    {
      let snapshot = self.snapshot();
      if let Some(error) = snapshot.error {
        return Err(error);
      }
      if snapshot.entries.is_empty() {
        return Err("此来源没有可检测的固定代理入口".to_string());
      }
      let target_url = Url::parse(get_provider(provider_id).info.entry_url)
        .map_err(|_| "服务入口地址错误，无法检测代理可用性".to_string())?;

      // Serialize whole-pool passes. Concurrent UI refreshes then reuse the fresh cache.
      let _probe_guard = self.probe_lock.lock().await;
      let started_at = Instant::now();
      let mut statuses = HashMap::new();
      let mut pending = Vec::new();
      {
        let mut cache = self.health_cache.lock().await;
        cache.retain(|_, item| {
          started_at.saturating_duration_since(item.checked_at) < HEALTH_CACHE_RETENTION
        });
        for entry in &snapshot.entries {
          let key = health_cache_key(provider_id, entry);
          if !force {
            if let Some(cached) = cache.get(&key).filter(|item| {
              started_at.saturating_duration_since(item.checked_at) < HEALTH_CACHE_TTL
            }) {
              statuses.insert(entry.id.clone(), cached.result.clone());
              continue;
            }
          }
          pending.push((key, entry.clone()));
        }
      }

      if !pending.is_empty() {
        let semaphore = Arc::new(Semaphore::new(HEALTH_PROBE_CONCURRENCY));
        let mut tasks = JoinSet::new();
        let mut pending = pending.into_iter();
        loop {
          while tasks.len() < HEALTH_PROBE_CONCURRENCY {
            let Some((key, entry)) = pending.next() else {
              break;
            };
            let permit = Arc::clone(&semaphore)
              .acquire_owned()
              .await
              .map_err(|_| "代理可用性检测调度失败".to_string())?;
            let target_url = target_url.clone();
            tasks.spawn(async move {
              let _permit = permit;
              let probe_started = Instant::now();
              let proxy_url = match Url::parse(&entry.proxy_url) {
                Ok(proxy_url) => proxy_url,
                Err(_) => {
                  return (
                    key,
                    ProxyPoolHealthEntry {
                      entry_id: entry.id,
                      available: false,
                      handshake_only: false,
                      latency_ms: 0,
                      message: Some("代理池入口地址格式错误".to_string()),
                    },
                  );
                }
              };
              let page_result =
                crate::network::check_proxy_http_route_fast(proxy_url.clone(), target_url.clone())
                  .await;
              let (available, handshake_only, message) = match page_result {
                Ok(()) => (true, false, None),
                Err(page_error) => {
                  let handshake_result =
                    crate::network::check_proxy_route_fast(proxy_url, target_url).await;
                  match handshake_result {
                    Ok(()) => (false, true, Some(format!("页面请求失败：{page_error}"))),
                    Err(handshake_error) => (
                      false,
                      false,
                      Some(format!(
                        "页面请求失败：{page_error}；代理握手失败：{handshake_error}"
                      )),
                    ),
                  }
                }
              };
              let latency_ms = probe_started.elapsed().as_millis().min(u64::MAX as u128) as u64;
              (
                key,
                ProxyPoolHealthEntry {
                  entry_id: entry.id,
                  available,
                  handshake_only,
                  latency_ms,
                  message,
                },
              )
            });
          }

          let Some(joined) = tasks.join_next().await else {
            break;
          };
          let (key, result) = joined.map_err(|_| "代理可用性检测任务异常结束".to_string())?;
          statuses.insert(result.entry_id.clone(), result.clone());
          self.health_cache.lock().await.insert(
            key,
            CachedProxyHealth {
              checked_at: Instant::now(),
              result,
            },
          );
        }
      }

      let mut entries = snapshot
        .entries
        .iter()
        .filter_map(|entry| statuses.get(&entry.id).cloned())
        .collect::<Vec<_>>();
      if entries.len() != snapshot.entries.len() {
        return Err("部分代理入口未能完成可用性检测，请重试".to_string());
      }
      sort_health_entries(&mut entries);
      Ok(ProxyPoolHealthSnapshot {
        provider_id,
        revision: snapshot.revision,
        checked_at_ms: now_ms(),
        entries,
      })
    }
  }

  fn save_source(&self, raw_path: &str) -> Result<(), String> {
    let state_path = self.data_dir.join(SOURCE_STATE_FILE);
    if raw_path.trim().is_empty() {
      match fs::remove_file(state_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("无法重置本应用的代理池来源选择".to_string()),
      }
      return Ok(());
    }

    let source = normalized_source(Path::new(raw_path.trim()))?;
    read_entries(&source)?;
    let selection = SourceSelection {
      version: 1,
      source_path: path_text(&source)?,
    };
    let bytes =
      serde_json::to_vec(&selection).map_err(|_| "无法保存本应用的代理池来源选择".to_string())?;
    fs::create_dir_all(&self.data_dir).map_err(|_| "无法创建本应用的代理池设置目录".to_string())?;
    let temporary = self
      .data_dir
      .join(format!(".proxy-pool-source-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
      let mut file = File::create(&temporary)?;
      file.write_all(&bytes)?;
      file.sync_all()?;
      fs::rename(&temporary, &state_path)
    })();
    if result.is_err() {
      let _ = fs::remove_file(&temporary);
      return Err("无法保存本应用的代理池来源选择".to_string());
    }
    Ok(())
  }

  fn snapshot_with_environment(
    &self,
    override_path: Option<OsString>,
    user_profile: Option<PathBuf>,
  ) -> ProxyPoolSnapshot {
    let mut snapshot = ProxyPoolSnapshot {
      source_path: None,
      discovered_sources: Vec::new(),
      entries: Vec::new(),
      revision: String::new(),
      checked_at_ms: now_ms(),
      error: None,
    };
    let result = (|| {
      // An explicit override is an isolation boundary: never discover production sources.
      let source = if let Some(raw) = override_path {
        normalized_source(&PathBuf::from(raw))?
      } else {
        let discovered = discover_sources(user_profile.as_deref())?;
        snapshot.discovered_sources = discovered
          .iter()
          .map(|path| path_text(path))
          .collect::<Result<_, _>>()?;
        match self.saved_source()? {
          Some(source) => source,
          None if discovered.len() == 1 => discovered[0].clone(),
          None if discovered.is_empty() => {
            return Err("未发现本地代理池，请选择 sidecar.yaml 来源文件".to_string())
          }
          None => return Err("发现多个本地代理池，请先选择来源，未自动绑定任何代理".to_string()),
        }
      };
      snapshot.source_path = Some(path_text(&source)?);
      snapshot.entries = read_entries(&source)?;
      snapshot.revision = fingerprint(
        &serde_json::to_value(&snapshot.entries).map_err(|_| "无法生成代理池状态".to_string())?,
      );
      Ok(())
    })();
    if let Err(error) = result {
      snapshot.entries.clear();
      snapshot.error = Some(error);
    }
    snapshot
  }

  fn saved_source(&self) -> Result<Option<PathBuf>, String> {
    let state_path = self.data_dir.join(SOURCE_STATE_FILE);
    let bytes = match read_limited(&state_path, MAX_STATE_BYTES) {
      Ok(bytes) => bytes,
      Err(ReadFailure::Missing) => return Ok(None),
      Err(_) => return Err("本应用的代理池来源设置无法读取，请重新选择或重置".to_string()),
    };
    let selection: SourceSelection = serde_json::from_slice(&bytes)
      .map_err(|_| "本应用的代理池来源设置无效，请重新选择或重置".to_string())?;
    if selection.version != 1 {
      return Err("本应用的代理池来源设置版本不受支持".to_string());
    }
    normalized_source(Path::new(&selection.source_path)).map(Some)
  }
}

fn health_cache_key(provider_id: ProviderId, entry: &ProxyPoolEntry) -> String {
  format!(
    "{}|{}|{}|{}",
    provider_id.as_str(),
    entry.id,
    entry.proxy_url,
    entry.fingerprint
  )
}

fn sort_health_entries(entries: &mut [ProxyPoolHealthEntry]) {
  entries.sort_by(|left, right| {
    health_rank(left)
      .cmp(&health_rank(right))
      .then_with(|| left.latency_ms.cmp(&right.latency_ms))
      .then_with(|| left.entry_id.cmp(&right.entry_id))
  });
}

fn health_rank(entry: &ProxyPoolHealthEntry) -> u8 {
  if entry.available {
    0
  } else if entry.handshake_only {
    1
  } else {
    2
  }
}

/// Re-read the selected file before use. A cached picker result never authorizes routing.
pub fn validate_binding(
  raw_proxy_url: &str,
  binding: &ProxyPoolBinding,
) -> Result<ProxyPoolBinding, String> {
  validate_binding_with_override(raw_proxy_url, binding, std::env::var_os(SOURCE_OVERRIDE))
}

fn validate_binding_with_override(
  raw_proxy_url: &str,
  binding: &ProxyPoolBinding,
  override_path: Option<OsString>,
) -> Result<ProxyPoolBinding, String> {
  let expected_source = override_path
    .map(|raw| normalized_source(&PathBuf::from(raw)))
    .transpose()?;
  let source = normalized_source(Path::new(&binding.source_path))?;
  if expected_source.is_some_and(|expected| expected != source) {
    return Err("账号绑定不属于当前进程指定的测试代理池，已阻止读取其他来源".to_string());
  }
  let entries = read_entries(&source)?;
  let current = entries
    .iter()
    .find(|entry| entry.id == binding.entry_id)
    .ok_or_else(|| "绑定的代理池入口已移除或不再符合要求，请重新选择代理".to_string())?;
  let endpoint = normalize_endpoint(raw_proxy_url)?;
  if current.proxy_url != endpoint || current.fingerprint != binding.fingerprint {
    return Err("代理池入口或节点定义已变化，已阻止打开，请重新确认代理绑定".to_string());
  }
  Ok(ProxyPoolBinding {
    source_path: path_text(&source)?,
    entry_id: current.id.clone(),
    fingerprint: current.fingerprint.clone(),
    label: current.label.clone(),
  })
}

fn now_ms() -> u64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .unwrap_or_default()
    .as_millis()
    .min(u64::MAX as u128) as u64
}

fn path_text(path: &Path) -> Result<String, String> {
  path
    .to_str()
    .map(str::to_owned)
    .ok_or_else(|| "代理池来源路径编码不受支持".to_string())
}

#[cfg(windows)]
fn is_local_absolute_source(path: &Path) -> bool {
  use std::path::{Component, Prefix};

  path.is_absolute()
    && matches!(
      path.components().next(),
      Some(Component::Prefix(prefix))
        if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    )
}

#[cfg(not(windows))]
fn is_local_absolute_source(path: &Path) -> bool {
  path.is_absolute() && !path.as_os_str().to_string_lossy().starts_with("//")
}

fn normalized_source(path: &Path) -> Result<PathBuf, String> {
  // Reject explicit network/device sources before canonicalize can access them.
  if !is_local_absolute_source(path) {
    return Err("代理池来源必须是本机 YAML 文件的绝对路径".to_string());
  }
  let canonical =
    fs::canonicalize(path).map_err(|_| "代理池来源不存在或不可读取，请重新选择来源".to_string())?;
  // Local paths can resolve through reparse points. Normal Windows canonical paths
  // have a VerbatimDisk prefix, which remains valid; UNC/device targets do not.
  if !is_local_absolute_source(&canonical) {
    return Err("代理池来源解析后不是本机文件路径，请重新选择来源".to_string());
  }
  if !canonical.is_file() {
    return Err("代理池来源必须是 YAML 文件，不能是目录".to_string());
  }
  path_text(&canonical)?;
  Ok(canonical)
}

fn discover_sources(user_profile: Option<&Path>) -> Result<Vec<PathBuf>, String> {
  let Some(profile) = user_profile else {
    return Ok(Vec::new());
  };
  let directories = match fs::read_dir(profile.join("WorkBuddy")) {
    Ok(directories) => directories,
    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
    Err(_) => return Err("无法只读扫描本地 WorkBuddy 代理池目录".to_string()),
  };
  let mut sources = BTreeSet::new();
  for (index, directory) in directories.enumerate() {
    if index > 2048 {
      return Err("WorkBuddy 目录过多，请直接选择代理池来源文件".to_string());
    }
    let directory = directory.map_err(|_| "无法读取本地代理池目录项".to_string())?;
    let kind = directory
      .file_type()
      .map_err(|_| "无法读取本地代理池目录项".to_string())?;
    if !kind.is_dir() || !is_date_directory(&directory.file_name().to_string_lossy()) {
      continue;
    }
    let candidate = directory
      .path()
      .join("telegram-panel")
      .join("flclash-sidecar")
      .join("sidecar.yaml");
    if candidate.is_file() {
      sources.insert(normalized_source(&candidate)?);
    }
  }
  Ok(sources.into_iter().collect())
}

fn is_date_directory(name: &str) -> bool {
  let parts = name.split('-').collect::<Vec<_>>();
  if !matches!(parts.len(), 3 | 6) || parts[0].len() != 4 {
    return false;
  }
  parts.iter().enumerate().all(|(index, part)| {
    (index == 0 || part.len() == 2) && part.bytes().all(|byte| byte.is_ascii_digit())
  })
}

enum ReadFailure {
  Missing,
  Invalid,
}

fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, ReadFailure> {
  let file = File::open(path).map_err(|error| {
    if error.kind() == std::io::ErrorKind::NotFound {
      ReadFailure::Missing
    } else {
      ReadFailure::Invalid
    }
  })?;
  let metadata = file.metadata().map_err(|_| ReadFailure::Invalid)?;
  if !metadata.is_file() || metadata.len() > limit as u64 {
    return Err(ReadFailure::Invalid);
  }
  let mut bytes = Vec::new();
  file
    .take(limit as u64 + 1)
    .read_to_end(&mut bytes)
    .map_err(|_| ReadFailure::Invalid)?;
  if bytes.is_empty() || bytes.len() > limit {
    return Err(ReadFailure::Invalid);
  }
  Ok(bytes)
}

fn read_entries(path: &Path) -> Result<Vec<ProxyPoolEntry>, String> {
  let bytes = read_limited(path, MAX_SOURCE_BYTES)
    .map_err(|_| "无法读取代理池 YAML：文件缺失、不可读、为空或超过 4 MiB".to_string())?;
  parse_entries(&bytes)
}

fn parse_entries(bytes: &[u8]) -> Result<Vec<ProxyPoolEntry>, String> {
  if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
    return Err("代理池 YAML 为空或超过 4 MiB".to_string());
  }
  let budget = Cell::new(MAX_YAML_NODES);
  let mut documents = serde_yaml::Deserializer::from_slice(bytes);
  let document = documents
    .next()
    .ok_or_else(|| "代理池 YAML 无效".to_string())?;
  let root = BoundedSeed {
    budget: &budget,
    depth: 0,
  }
  .deserialize(document)
  .map_err(|_| "代理池 YAML 无效、结构过大或包含不支持的标签/重复字段".to_string())?;
  if documents.next().is_some() {
    return Err("代理池来源只能包含一个 YAML 文档".to_string());
  }
  let root = root
    .as_object()
    .ok_or_else(|| "代理池 YAML 顶层必须是配置对象".to_string())?;
  if !empty_auth(root.get("authentication")) {
    return Err("此代理池启用了入站认证，当前仅支持无认证本机入口".to_string());
  }
  let listeners = array_field(root, "listeners")?;
  if listeners.len() > MAX_LISTENERS {
    return Err("代理池入口数量超过安全读取上限".to_string());
  }
  let proxies = array_field(root, "proxies")?;
  let groups = array_field(root, "proxy-groups")?;
  let mut reserved = BTreeSet::new();
  for group in groups {
    if let Some(name) = group.get("name").and_then(Value::as_str) {
      reserved.insert(name.to_string());
    }
  }
  if let Some(providers) = root
    .get("proxy-providers")
    .filter(|providers| !providers.is_null())
  {
    let providers = providers
      .as_object()
      .ok_or_else(|| "代理池 provider 配置结构无效".to_string())?;
    reserved.extend(providers.keys().cloned());
  }
  let mut nodes = BTreeMap::new();
  for proxy in proxies {
    let name = proxy
      .get("name")
      .and_then(Value::as_str)
      .filter(|name| !name.trim().is_empty())
      .ok_or_else(|| "代理池静态节点名称缺失或无效".to_string())?;
    if nodes.insert(name, proxy).is_some() || reserved.contains(name) {
      return Err("代理池节点名称重复或与动态引用冲突，无法确认唯一绑定".to_string());
    }
  }

  let mut ports = BTreeSet::new();
  let mut candidates = Vec::new();
  let mut target_counts = BTreeMap::<String, usize>::new();
  for listener in listeners {
    let listener = listener
      .as_object()
      .ok_or_else(|| "代理池入口结构无效".to_string())?;
    let port =
      port_number(listener.get("port")).ok_or_else(|| "代理池入口端口缺失或无效".to_string())?;
    // Even an unsupported listener can conflict with a candidate's socket binding.
    if !ports.insert(port) {
      return Err("代理池存在重复入口端口，无法确认唯一绑定".to_string());
    }
    let scheme = match listener.get("type").and_then(Value::as_str) {
      Some("http" | "mixed") => "http",
      Some("socks") => "socks5",
      _ => continue,
    };
    if !empty_auth(listener.get("users")) || !empty_auth(listener.get("authentication")) {
      continue;
    }
    let Some(host) = local_listener_host(listener.get("listen")) else {
      continue;
    };
    let Some(target) = listener.get("proxy").and_then(Value::as_str) else {
      continue;
    };
    if matches!(
      target.to_ascii_uppercase().as_str(),
      "DIRECT" | "REJECT" | "COMPATIBLE"
    ) {
      continue;
    }
    let Some(node) = nodes.get(target).copied().filter(|node| static_node(node)) else {
      continue;
    };
    let Ok(proxy_url) = normalize_endpoint(&format!("{scheme}://{host}:{port}")) else {
      continue;
    };
    let listener_name = listener.get("name").and_then(Value::as_str).unwrap_or("");
    let label = format!(
      "{} · {} · {}",
      safe_label(listener_name, "本机入口"),
      safe_label(target, "固定节点"),
      port
    );
    let entry = ProxyPoolEntry {
      id: format!("{scheme}:{port}"),
      label,
      proxy_url,
      fingerprint: fingerprint(node),
      duplicate_count: 1,
    };
    *target_counts.entry(target.to_string()).or_default() += 1;
    candidates.push((target.to_string(), entry));
  }
  if candidates.is_empty() {
    return Err(
      "未找到可用候选：仅支持无认证、本机监听并直指静态节点的 HTTP/SOCKS 入口".to_string(),
    );
  }
  let mut entries = candidates
    .into_iter()
    .map(|(target, mut entry)| {
      entry.duplicate_count = target_counts[&target];
      entry
    })
    .collect::<Vec<_>>();
  entries.sort_by(|left, right| left.id.cmp(&right.id));
  Ok(entries)
}

fn array_field<'a>(root: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], String> {
  match root.get(key) {
    None | Some(Value::Null) => Ok(&[]),
    Some(Value::Array(values)) => Ok(values),
    _ => Err("代理池列表字段结构无效".to_string()),
  }
}

fn empty_auth(value: Option<&Value>) -> bool {
  match value {
    None | Some(Value::Null) => true,
    Some(Value::Array(values)) => values.is_empty(),
    _ => false,
  }
}

fn port_number(value: Option<&Value>) -> Option<u16> {
  value
    .and_then(Value::as_u64)
    .filter(|port| (1..=u16::MAX as u64).contains(port))
    .map(|port| port as u16)
}

fn static_node(node: &Value) -> bool {
  let Some(node) = node.as_object() else {
    return false;
  };
  let Some(kind) = node.get("type").and_then(Value::as_str) else {
    return false;
  };
  if !matches!(
    kind,
    "ss"
      | "ssr"
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
  if [
    "dialer-proxy",
    "proxy",
    "detour",
    "use",
    "proxies",
    "proxy-providers",
  ]
  .iter()
  .any(|field| node.get(*field).is_some_and(|value| !value.is_null()))
  {
    return false;
  }
  node
    .get("server")
    .and_then(Value::as_str)
    .is_some_and(|server| !server.trim().is_empty())
    && port_number(node.get("port")).is_some()
}

fn local_listener_host(value: Option<&Value>) -> Option<String> {
  let raw = match value {
    None => "0.0.0.0",
    Some(value) => value.as_str()?,
  };
  if raw.eq_ignore_ascii_case("localhost") {
    return Some("127.0.0.1".to_string());
  }
  let address = raw
    .trim_start_matches('[')
    .trim_end_matches(']')
    .parse::<IpAddr>()
    .ok()?;
  if address.is_unspecified() {
    return Some(
      if address.is_ipv4() {
        "127.0.0.1"
      } else {
        "[::1]"
      }
      .to_string(),
    );
  }
  if !address.is_loopback() {
    return None;
  }
  Some(match address {
    IpAddr::V4(address) => address.to_string(),
    IpAddr::V6(address) => format!("[{address}]"),
  })
}

fn normalize_endpoint(raw: &str) -> Result<String, String> {
  let mut url = Url::parse(raw.trim()).map_err(|_| "代理池绑定地址无效".to_string())?;
  if !matches!(url.scheme(), "http" | "socks5")
    || url.port().is_none()
    || !url.username().is_empty()
    || url.password().is_some()
    || !matches!(url.path(), "" | "/")
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return Err("代理池绑定必须是无认证本机 HTTP/SOCKS5 端口".to_string());
  }
  let host = url
    .host_str()
    .ok_or_else(|| "代理池绑定地址缺少主机".to_string())?;
  let normalized = local_listener_host(Some(&Value::String(host.to_string())))
    .ok_or_else(|| "代理池绑定只允许本机入口".to_string())?;
  // Wildcard addresses are accepted only in the source, never as a saved client endpoint.
  if host
    .trim_matches(['[', ']'])
    .parse::<IpAddr>()
    .is_ok_and(|ip| ip.is_unspecified())
  {
    return Err("代理池绑定不能使用通配监听地址".to_string());
  }
  url
    .set_host(Some(&normalized))
    .map_err(|_| "代理池绑定主机无效".to_string())?;
  url.set_path("/");
  Ok(url.to_string())
}

fn safe_label(raw: &str, fallback: &str) -> String {
  if raw.contains("://") || raw.contains('@') {
    return fallback.to_string();
  }
  let value = raw
    .chars()
    .filter(|character| {
      !character.is_control()
        && !matches!(*character, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
    })
    .take(42)
    .collect::<String>();
  if value.trim().is_empty() {
    fallback.to_string()
  } else {
    value.trim().to_string()
  }
}

fn fingerprint(value: &Value) -> String {
  fn canonical(value: &Value, output: &mut Vec<u8>) {
    match value {
      Value::Array(values) => {
        output.push(b'[');
        for (index, value) in values.iter().enumerate() {
          if index > 0 {
            output.push(b',');
          }
          canonical(value, output);
        }
        output.push(b']');
      }
      Value::Object(values) => {
        output.push(b'{');
        let ordered = values.iter().collect::<BTreeMap<_, _>>();
        for (index, (key, value)) in ordered.into_iter().enumerate() {
          if index > 0 {
            output.push(b',');
          }
          output.extend(serde_json::to_vec(key).expect("JSON object keys are strings"));
          output.push(b':');
          canonical(value, output);
        }
        output.push(b'}');
      }
      _ => {
        output.extend(serde_json::to_vec(value).expect("bounded YAML contains only JSON values"))
      }
    }
  }
  let mut bytes = Vec::new();
  canonical(value, &mut bytes);
  format!("{:x}", Sha256::digest(bytes))
}

// Bound expansion while deserializing, including YAML aliases. Building Value first
// and checking its size afterwards would permit a small alias bomb to allocate freely.
#[derive(Clone, Copy)]
struct BoundedSeed<'a> {
  budget: &'a Cell<usize>,
  depth: usize,
}

impl BoundedSeed<'_> {
  fn child(&self) -> Self {
    Self {
      budget: self.budget,
      depth: self.depth + 1,
    }
  }
}

impl<'de> DeserializeSeed<'de> for BoundedSeed<'_> {
  type Value = Value;

  fn deserialize<D>(self, deserializer: D) -> Result<Value, D::Error>
  where
    D: de::Deserializer<'de>,
  {
    if self.depth > MAX_YAML_DEPTH || self.budget.get() == 0 {
      return Err(de::Error::custom("configuration structure limit exceeded"));
    }
    self.budget.set(self.budget.get() - 1);
    deserializer.deserialize_any(BoundedVisitor(self))
  }
}

struct BoundedVisitor<'a>(BoundedSeed<'a>);

impl<'de> Visitor<'de> for BoundedVisitor<'_> {
  type Value = Value;

  fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
    formatter.write_str("a bounded untagged YAML configuration")
  }

  fn visit_bool<E: de::Error>(self, value: bool) -> Result<Value, E> {
    Ok(Value::Bool(value))
  }
  fn visit_i64<E: de::Error>(self, value: i64) -> Result<Value, E> {
    Ok(Value::Number(value.into()))
  }
  fn visit_u64<E: de::Error>(self, value: u64) -> Result<Value, E> {
    Ok(Value::Number(value.into()))
  }
  fn visit_f64<E: de::Error>(self, value: f64) -> Result<Value, E> {
    Number::from_f64(value)
      .map(Value::Number)
      .ok_or_else(|| de::Error::custom("non-finite number"))
  }
  fn visit_str<E: de::Error>(self, value: &str) -> Result<Value, E> {
    Ok(Value::String(value.to_string()))
  }
  fn visit_string<E: de::Error>(self, value: String) -> Result<Value, E> {
    Ok(Value::String(value))
  }
  fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
    Ok(Value::Null)
  }
  fn visit_none<E: de::Error>(self) -> Result<Value, E> {
    Ok(Value::Null)
  }

  fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Value, A::Error> {
    let mut values = Vec::new();
    while let Some(value) = sequence.next_element_seed(self.0.child())? {
      values.push(value);
    }
    Ok(Value::Array(values))
  }

  fn visit_map<A: MapAccess<'de>>(self, mut mapping: A) -> Result<Value, A::Error> {
    let mut values = Map::new();
    while let Some(key) = mapping.next_key::<String>()? {
      if values.contains_key(&key) {
        return Err(de::Error::custom("duplicate mapping field"));
      }
      let value = mapping.next_value_seed(self.0.child())?;
      values.insert(key, value);
    }
    Ok(Value::Object(values))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const NODE: &str = "{name: fixed-a, type: ss, server: example.invalid, port: 443, cipher: aes-128-gcm, password: synthetic-test-secret}";

  fn source(listeners: &str) -> String {
    format!("proxies:\n  - {NODE}\nlisteners:\n{listeners}\n")
  }

  fn one_listener() -> &'static str {
    "  - {name: local-a, type: socks, listen: 0.0.0.0, port: 18081, proxy: fixed-a}"
  }

  struct TestDirectory(PathBuf);
  impl TestDirectory {
    fn new() -> Self {
      let path = std::env::temp_dir().join(format!("diehao-proxy-pool-{}", uuid::Uuid::new_v4()));
      fs::create_dir_all(&path).unwrap();
      Self(path)
    }
    fn write_source(&self, yaml: &str) -> PathBuf {
      let path = self.0.join("sidecar.yaml");
      fs::write(&path, yaml).unwrap();
      fs::canonicalize(path).unwrap()
    }
  }
  impl Drop for TestDirectory {
    fn drop(&mut self) {
      let _ = fs::remove_dir_all(&self.0);
    }
  }

  fn binding(path: &Path, entry: &ProxyPoolEntry) -> ProxyPoolBinding {
    ProxyPoolBinding {
      source_path: path_text(path).unwrap(),
      entry_id: entry.id.clone(),
      fingerprint: entry.fingerprint.clone(),
      label: entry.label.clone(),
    }
  }

  #[cfg(windows)]
  #[test]
  fn source_paths_accept_local_disks_and_reject_unc_devices_and_relative_paths() {
    for path in [
      r"C:\pool\sidecar.yaml",
      r"\\?\C:\pool\sidecar.yaml",
      "C:/pool/sidecar.yaml",
    ] {
      assert!(is_local_absolute_source(Path::new(path)), "{path}");
    }
    for path in [
      r"\\server\share\sidecar.yaml",
      r"\\?\UNC\server\share\sidecar.yaml",
      r"\\.\C:\pool\sidecar.yaml",
      r"\\?\GLOBALROOT\Device\HarddiskVolume1\sidecar.yaml",
      r"C:relative.yaml",
      r"\rooted.yaml",
      "relative.yaml",
      "//server/share/sidecar.yaml",
      "",
    ] {
      assert!(!is_local_absolute_source(Path::new(path)), "{path}");
    }
  }

  #[cfg(not(windows))]
  #[test]
  fn source_paths_reject_network_style_and_relative_paths() {
    assert!(is_local_absolute_source(Path::new("/tmp/sidecar.yaml")));
    for path in ["//server/share/sidecar.yaml", "relative.yaml", ""] {
      assert!(!is_local_absolute_source(Path::new(path)), "{path}");
    }
  }

  #[test]
  fn imports_static_local_socks_http_and_mixed_without_exporting_secrets() {
    let yaml = source(&format!("{}\n  - {{name: local-b, type: mixed, listen: 127.0.0.1, port: 18082, proxy: fixed-a}}\n  - {{name: local-c, type: http, listen: '::1', port: 18083, proxy: fixed-a}}", one_listener()));
    let entries = parse_entries(yaml.as_bytes()).unwrap();
    assert_eq!(entries.len(), 3);
    assert!(entries.iter().all(|entry| entry.duplicate_count == 3));
    assert!(entries
      .iter()
      .any(|entry| entry.proxy_url == "socks5://127.0.0.1:18081/"));
    assert!(entries
      .iter()
      .any(|entry| entry.proxy_url == "http://[::1]:18083/"));
    let serialized = serde_json::to_string(&entries).unwrap();
    assert!(!serialized.contains("synthetic-test-secret"));
    assert!(!serialized.contains("example.invalid"));
  }

  #[test]
  fn accepts_an_explicitly_empty_proxy_providers_field() {
    let yaml = format!(
      "proxies:\n  - {NODE}\nproxy-providers:\nlisteners:\n{}",
      one_listener()
    );
    let entries = parse_entries(yaml.as_bytes()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].proxy_url, "socks5://127.0.0.1:18081/");
  }

  #[test]
  fn orders_available_proxies_by_latency_before_unavailable_entries() {
    let mut entries = vec![
      ProxyPoolHealthEntry {
        entry_id: "unavailable-fast".to_string(),
        available: false,
        handshake_only: false,
        latency_ms: 5,
        message: Some("握手失败".to_string()),
      },
      ProxyPoolHealthEntry {
        entry_id: "handshake-only".to_string(),
        available: false,
        handshake_only: true,
        latency_ms: 10,
        message: Some("页面请求失败".to_string()),
      },
      ProxyPoolHealthEntry {
        entry_id: "available-slow".to_string(),
        available: true,
        handshake_only: false,
        latency_ms: 80,
        message: None,
      },
      ProxyPoolHealthEntry {
        entry_id: "available-fast".to_string(),
        available: true,
        handshake_only: false,
        latency_ms: 20,
        message: None,
      },
    ];
    sort_health_entries(&mut entries);
    assert_eq!(
      entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<Vec<_>>(),
      vec![
        "available-fast",
        "available-slow",
        "handshake-only",
        "unavailable-fast"
      ]
    );
  }

  #[test]
  fn rejects_ambiguous_ports_names_and_duplicate_yaml_fields() {
    let duplicate_port = source(&format!("{}\n{}", one_listener(), one_listener()));
    assert!(parse_entries(duplicate_port.as_bytes()).is_err());
    let duplicate_name = format!(
      "proxies:\n  - {NODE}\n  - {NODE}\nlisteners:\n{}",
      one_listener()
    );
    assert!(parse_entries(duplicate_name.as_bytes()).is_err());
    assert!(parse_entries(b"proxies: []\nproxies: []\nlisteners: []").is_err());
  }

  #[test]
  fn filters_authenticated_remote_group_direct_unknown_and_chained_entries() {
    for change in [
      ("listen: 0.0.0.0", "listen: 192.0.2.1"),
      ("proxy: fixed-a", "proxy: DIRECT"),
      ("proxy: fixed-a", "proxy: missing-node"),
      ("type: socks", "type: unknown"),
      (
        "proxy: fixed-a}",
        "proxy: fixed-a, users: [{username: test, password: private}]}",
      ),
    ] {
      assert!(parse_entries(
        source(one_listener())
          .replace(change.0, change.1)
          .as_bytes()
      )
      .is_err());
    }
    for extra in [
      "authentication: ['test:private']\n",
      "proxy-groups: [{name: fixed-a, type: select, proxies: [DIRECT]}]\n",
    ] {
      assert!(parse_entries(format!("{extra}{}", source(one_listener())).as_bytes()).is_err());
    }
    assert!(parse_entries(
      source(one_listener())
        .replace("cipher: aes-128-gcm", "dialer-proxy: auto-group")
        .as_bytes()
    )
    .is_err());
  }

  #[test]
  fn canonical_hash_ignores_mapping_order_but_tracks_nested_secret_changes() {
    let first = serde_json::json!({"a": {"password": "one", "enabled": true}, "b": [1, 2]});
    let reordered =
      serde_json::from_str::<Value>(r#"{"b":[1,2],"a":{"enabled":true,"password":"one"}}"#)
        .unwrap();
    let changed = serde_json::json!({"a": {"password": "two", "enabled": true}, "b": [1, 2]});
    assert_eq!(fingerprint(&first), fingerprint(&reordered));
    assert_ne!(fingerprint(&first), fingerprint(&changed));
    assert_ne!(
      fingerprint(&first),
      fingerprint(&serde_json::json!({
        "a": {"password": "one", "enabled": true}, "b": [2, 1]
      }))
    );
  }

  #[test]
  fn binding_rechecks_source_and_blocks_secret_target_endpoint_or_removed_entry_drift() {
    let directory = TestDirectory::new();
    let yaml = source(one_listener());
    let path = directory.write_source(&yaml);
    let entry = parse_entries(yaml.as_bytes()).unwrap().remove(0);
    let saved = binding(&path, &entry);
    assert!(validate_binding_with_override(&entry.proxy_url, &saved, None).is_ok());
    assert!(validate_binding_with_override("socks5://127.0.0.1:18082", &saved, None).is_err());
    for changed in [
      yaml.replace("synthetic-test-secret", "different-secret"),
      yaml.replace("example.invalid", "different.invalid"),
      yaml.replace("proxy: fixed-a", "proxy: DIRECT"),
      yaml.replace("port: 18081", "port: 18082"),
    ] {
      fs::write(&path, changed).unwrap();
      assert!(validate_binding_with_override(&entry.proxy_url, &saved, None).is_err());
    }
    fs::remove_file(&path).unwrap();
    assert!(validate_binding_with_override(&entry.proxy_url, &saved, None).is_err());
  }

  #[test]
  fn binding_allows_comment_reordering_and_refreshes_listener_label() {
    let directory = TestDirectory::new();
    let yaml = source(one_listener());
    let path = directory.write_source(&yaml);
    let entry = parse_entries(yaml.as_bytes()).unwrap().remove(0);
    let saved = binding(&path, &entry);
    fs::write(
      &path,
      format!(
        "# harmless comment\n{}",
        yaml.replace("local-a", "local-renamed")
      ),
    )
    .unwrap();
    let fresh = validate_binding_with_override("socks5://localhost:18081", &saved, None).unwrap();
    assert_eq!(fresh.fingerprint, saved.fingerprint);
    assert_ne!(fresh.label, saved.label);
  }

  #[test]
  fn errors_never_include_yaml_credentials() {
    for yaml in [
      "password: private-do-not-echo\nlisteners: [broken",
      "password: private-do-not-echo\nlisteners: invalid",
      "password: private-do-not-echo\n---\nproxies: []",
      "password: !unsafe private-do-not-echo",
    ] {
      let error = parse_entries(yaml.as_bytes()).unwrap_err();
      assert!(!error.contains("private-do-not-echo"));
      assert!(!error.contains(yaml));
    }
  }

  #[test]
  fn enforces_file_depth_and_alias_expansion_limits() {
    assert!(parse_entries(&vec![b' '; MAX_SOURCE_BYTES + 1]).is_err());
    let deep = format!(
      "{}0{}",
      "[".repeat(MAX_YAML_DEPTH + 5),
      "]".repeat(MAX_YAML_DEPTH + 5)
    );
    assert!(parse_entries(deep.as_bytes()).is_err());
    let mut aliases = "a: &a [0,0,0,0,0,0,0,0,0,0]\n".to_string();
    for (name, previous) in [('b', 'a'), ('c', 'b'), ('d', 'c'), ('e', 'd'), ('f', 'e')] {
      aliases.push_str(&format!(
        "{name}: &{name} [{}]\n",
        vec![format!("*{previous}"); 10].join(",")
      ));
    }
    assert!(parse_entries(aliases.as_bytes()).is_err());
  }

  #[test]
  fn discovery_is_one_level_and_multiple_sources_never_auto_select() {
    let directory = TestDirectory::new();
    for name in [
      "2026-11-21-10-20-30",
      "2026-11-22",
      "other/deeper/2026-11-23",
    ] {
      let path = directory
        .0
        .join("WorkBuddy")
        .join(name)
        .join("telegram-panel/flclash-sidecar");
      fs::create_dir_all(&path).unwrap();
      fs::write(path.join("sidecar.yaml"), source(one_listener())).unwrap();
    }
    let found = discover_sources(Some(&directory.0)).unwrap();
    assert_eq!(found.len(), 2);
    let pool = ProxyPool::new(directory.0.join("app-data"));
    let snapshot = pool.snapshot_with_environment(None, Some(directory.0.clone()));
    assert_eq!(snapshot.discovered_sources.len(), 2);
    assert!(snapshot.source_path.is_none());
    assert!(snapshot.entries.is_empty());
    assert!(snapshot.error.is_some());
  }

  #[test]
  fn invalid_explicit_override_never_falls_back_to_saved_or_discovered_source() {
    let directory = TestDirectory::new();
    let path = directory.write_source(&source(one_listener()));
    let pool = ProxyPool::new(directory.0.join("app-data"));
    pool.save_source(path.to_str().unwrap()).unwrap();
    for invalid in [
      OsString::from(""),
      OsString::from("relative.yaml"),
      directory.0.join("missing.yaml").into_os_string(),
    ] {
      let snapshot = pool.snapshot_with_environment(Some(invalid), Some(directory.0.clone()));
      assert!(snapshot.entries.is_empty());
      assert!(snapshot.discovered_sources.is_empty());
      assert!(snapshot.error.is_some());
    }
  }

  #[test]
  fn binding_override_refuses_other_sources_without_redirecting_the_account() {
    let first = TestDirectory::new();
    let second = TestDirectory::new();
    let yaml = source(one_listener());
    let source_a = first.write_source(&yaml);
    let source_b = second.write_source(&yaml);
    let entry = parse_entries(yaml.as_bytes()).unwrap().remove(0);
    let saved = binding(&source_a, &entry);
    assert!(validate_binding_with_override(
      &entry.proxy_url,
      &saved,
      Some(source_a.into_os_string())
    )
    .is_ok());
    assert!(validate_binding_with_override(
      &entry.proxy_url,
      &saved,
      Some(source_b.into_os_string())
    )
    .is_err());
    assert!(validate_binding_with_override(
      &entry.proxy_url,
      &saved,
      Some(OsString::from("missing-relative.yaml"))
    )
    .is_err());
  }

  #[test]
  fn source_selection_only_writes_app_state_and_can_reset_without_touching_source() {
    let directory = TestDirectory::new();
    let yaml = source(one_listener());
    let path = directory.write_source(&yaml);
    let pool = ProxyPool::new(directory.0.join("app-data"));
    pool.save_source(path.to_str().unwrap()).unwrap();
    let state = fs::read_to_string(pool.data_dir.join(SOURCE_STATE_FILE)).unwrap();
    assert!(!state.contains("synthetic-test-secret"));
    assert_eq!(pool.saved_source().unwrap(), Some(path.clone()));
    assert_eq!(fs::read_to_string(&path).unwrap(), yaml);
    let invalid_source = directory.0.join("invalid.yaml");
    fs::write(&invalid_source, b"invalid: [yaml").unwrap();
    assert!(pool.save_source(invalid_source.to_str().unwrap()).is_err());
    assert_eq!(pool.saved_source().unwrap(), Some(path.clone()));
    pool.save_source("").unwrap();
    assert!(pool.saved_source().unwrap().is_none());
    assert_eq!(fs::read_to_string(path).unwrap(), yaml);
  }

  #[test]
  fn corrupted_saved_selection_blocks_instead_of_auto_falling_back() {
    let directory = TestDirectory::new();
    let data = directory.0.join("app-data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join(SOURCE_STATE_FILE), b"{broken-private").unwrap();
    let snapshot = ProxyPool::new(data).snapshot_with_environment(None, Some(directory.0.clone()));
    assert!(snapshot.entries.is_empty());
    assert!(snapshot.error.is_some());
    assert!(!snapshot.error.unwrap().contains("broken-private"));
  }
}
