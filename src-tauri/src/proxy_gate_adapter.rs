use serde::{Deserialize, Serialize};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

const EXE_OVERRIDE: &str = "PROXY_GATE_EXE";
const CONFIG_OVERRIDE: &str = "PROXY_GATE_CONFIG";
const POOL_OVERRIDE: &str = "DIEHAO_PROXY_POOL_FILE";
const EXE_FILE: &str = "proxy-gate.exe";
const CONFIG_FILE: &str = "proxy-gate.json";
const STATUS_FILE: &str = "proxy-gate-status.json";
const PROXY_POOL_SOURCE_FILE: &str = "proxy-pool-source.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateEntry {
  pub entry_id: String,
  pub display_name: String,
  pub r#type: String,
  pub fingerprint: String,
  pub chain_depth: Option<u8>,
  pub health: Option<String>,
  pub latency_ms: Option<u64>,
  pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateAssignment {
  pub account_id: String,
  pub provider_id: String,
  pub uuid: String,
  pub account_alias: Option<String>,
  pub entry_id: String,
  pub display_name: String,
  pub fingerprint: String,
  pub port: u16,
  pub endpoint: String,
  pub binding_valid: bool,
  pub allowed: Option<bool>,
  pub health: Option<String>,
  pub latency_ms: Option<u64>,
  pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateSnapshot {
  pub available: bool,
  pub running: bool,
  pub error: Option<String>,
  pub exe_path: String,
  pub config_path: String,
  pub pool_path: Option<String>,
  pub entries: Vec<ProxyGateEntry>,
  pub assignments: Vec<ProxyGateAssignment>,
  pub status: Option<ProxyGateStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateProbeSnapshot {
  pub available: bool,
  pub error: Option<String>,
  pub revision: String,
  pub entries: Vec<ProxyGateProbeEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateProbeEntry {
  pub entry_id: String,
  pub display_name: String,
  pub r#type: String,
  pub proxy_url: String,
  pub fingerprint: String,
  pub chain_depth: Option<u8>,
  pub health: String,
  pub latency_ms: u64,
  pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateStatus {
  #[serde(default)]
  pub updated_at_ms: u64,
  #[serde(default)]
  pub pool_path: Option<String>,
  #[serde(default)]
  pub pool_revision: Option<String>,
  #[serde(default)]
  pub error: Option<String>,
  #[serde(default)]
  pub assignments: Vec<ProxyGateRuntimeAssignment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyGateRuntimeAssignment {
  pub account_id: String,
  pub entry_id: String,
  pub fingerprint: String,
  pub label: String,
  #[serde(default)]
  pub endpoint: String,
  #[serde(default)]
  pub upstream_proxy_url: Option<String>,
  #[serde(default)]
  pub allowed: bool,
  #[serde(default)]
  pub binding_valid: bool,
  #[serde(default)]
  pub health: Option<String>,
  #[serde(default)]
  pub latency_ms: Option<u64>,
  #[serde(default)]
  pub error: Option<String>,
}

pub struct ProxyGateAdapter {
  data_dir: PathBuf,
  managed_child: Mutex<Option<Child>>,
}

impl ProxyGateAdapter {
  pub fn new(data_dir: PathBuf) -> Self {
    Self {
      data_dir,
      managed_child: Mutex::new(None),
    }
  }

  pub fn snapshot(&self) -> ProxyGateSnapshot {
    let exe_path = self.exe_path();
    let config_path = self.config_path();
    let mut snapshot = ProxyGateSnapshot {
      available: exe_path.is_file(),
      running: self.is_running(),
      error: None,
      exe_path: exe_path.display().to_string(),
      config_path: config_path.display().to_string(),
      pool_path: None,
      entries: Vec::new(),
      assignments: Vec::new(),
      status: None,
    };

    if !exe_path.is_file() {
      snapshot.error = Some(format!(
        "未找到 Proxy Gate 可执行文件：{}。可通过 PROXY_GATE_EXE 指定路径。",
        exe_path.display()
      ));
      return snapshot;
    }

    let config = match self.ensure_config() {
      Ok(config) => config,
      Err(error) => {
        snapshot.available = false;
        snapshot.error = Some(error);
        return snapshot;
      }
    };
    snapshot.running = snapshot.running || listener_reachable(&config);
    snapshot.pool_path = config
      .pool_path
      .as_ref()
      .map(|path| path.display().to_string());

    let inspected = match self.inspect_pool() {
      Ok(inspected) => inspected,
      Err(error) => {
        snapshot.available = false;
        snapshot.error = Some(error);
        snapshot.assignments = assignments_without_pool(&config);
        return snapshot;
      }
    };
    if snapshot.pool_path.is_none() {
      snapshot.pool_path = inspected.source_path.clone();
    }

    let status = self.read_status();
    let runtime_by_account = status
      .as_ref()
      .map(|status| {
        status
          .assignments
          .iter()
          .map(|assignment| (assignment.account_id.as_str(), assignment))
          .collect::<std::collections::HashMap<_, _>>()
      })
      .unwrap_or_default();
    let runtime_by_entry = status
      .as_ref()
      .map(|status| {
        status
          .assignments
          .iter()
          .map(|assignment| (assignment.entry_id.as_str(), assignment))
          .collect::<std::collections::HashMap<_, _>>()
      })
      .unwrap_or_default();

    snapshot.entries = inspected
      .entries
      .iter()
      .map(|entry| {
        let runtime = runtime_by_entry.get(entry.id.as_str()).copied();
        ProxyGateEntry {
          entry_id: entry.id.clone(),
          display_name: entry.label.clone(),
          r#type: proxy_type(&entry.proxy_url),
          fingerprint: entry.fingerprint.clone(),
          chain_depth: None,
          health: runtime.and_then(|item| item.health.clone()),
          latency_ms: runtime.and_then(|item| item.latency_ms),
          message: runtime.and_then(|item| item.error.clone()),
        }
      })
      .collect();
    sort_entries(&mut snapshot.entries);

    let entries_by_id = inspected
      .entries
      .iter()
      .map(|entry| (entry.id.as_str(), entry))
      .collect::<std::collections::HashMap<_, _>>();
    snapshot.assignments = config
      .assignments
      .iter()
      .map(|assignment| {
        let entry = entries_by_id.get(assignment.entry_id.as_str()).copied();
        let runtime = runtime_by_account
          .get(assignment.account_id.as_str())
          .copied();
        let binding_valid = entry.is_some_and(|entry| entry.fingerprint == assignment.fingerprint);
        let (provider_id, uuid) = split_account_id(&assignment.account_id);
        ProxyGateAssignment {
          account_id: assignment.account_id.clone(),
          provider_id,
          uuid,
          account_alias: None,
          entry_id: assignment.entry_id.clone(),
          display_name: assignment.label.clone(),
          fingerprint: assignment.fingerprint.clone(),
          port: assignment.port,
          endpoint: format!("http://{}:{}", config.listen_host, assignment.port),
          binding_valid,
          allowed: runtime.map(|runtime| runtime.allowed),
          health: runtime.and_then(|runtime| runtime.health.clone()),
          latency_ms: runtime.and_then(|runtime| runtime.latency_ms),
          error: if !binding_valid {
            Some("代理入口定义已变化，需要重新确认绑定".to_string())
          } else {
            runtime.and_then(|runtime| runtime.error.clone())
          },
        }
      })
      .collect();
    snapshot.assignments.sort_by(|left, right| {
      left
        .account_id
        .cmp(&right.account_id)
        .then(left.port.cmp(&right.port))
    });
    snapshot.status = status;
    snapshot
  }

  pub fn status(&self) -> Result<Option<ProxyGateStatus>, String> {
    let path = self.status_path();
    if !path.exists() {
      return Ok(None);
    }
    let bytes =
      fs::read(&path).map_err(|_| format!("无法读取 Proxy Gate 状态：{}", path.display()))?;
    serde_json::from_slice(&bytes)
      .map(Some)
      .map_err(|_| format!("Proxy Gate 状态文件格式无效：{}", path.display()))
  }

  pub fn probe_pool(&self) -> ProxyGateProbeSnapshot {
    let mut snapshot = ProxyGateProbeSnapshot {
      available: false,
      error: None,
      revision: String::new(),
      entries: Vec::new(),
    };
    if !self.exe_path().is_file() {
      snapshot.error = Some(format!(
        "未找到 Proxy Gate 可执行文件：{}。可通过 PROXY_GATE_EXE 指定路径。",
        self.exe_path().display()
      ));
      return snapshot;
    }
    if let Err(error) = self.ensure_config() {
      snapshot.error = Some(error);
      return snapshot;
    }
    let output = match self.run_gate(&["list", "--json", "--concurrency", "16"]) {
      Ok(output) => output,
      Err(error) => {
        snapshot.error = Some(error);
        return snapshot;
      }
    };
    let parsed = match serde_json::from_str::<ListOutput>(&output) {
      Ok(parsed) => parsed,
      Err(error) => {
        snapshot.error = Some(format!("Proxy Gate 返回的健康检测 JSON 无效：{error}"));
        return snapshot;
      }
    };
    let entries_by_id = parsed
      .entries
      .iter()
      .map(|entry| (entry.id.as_str(), entry))
      .collect::<std::collections::HashMap<_, _>>();
    snapshot.entries = parsed
      .health
      .into_iter()
      .filter_map(|health| {
        let entry = entries_by_id.get(health.entry_id.as_str()).copied()?;
        Some(ProxyGateProbeEntry {
          entry_id: entry.id.clone(),
          display_name: entry.label.clone(),
          r#type: proxy_type(&entry.proxy_url),
          proxy_url: entry.proxy_url.clone(),
          fingerprint: entry.fingerprint.clone(),
          chain_depth: None,
          health: health.status,
          latency_ms: health.latency_ms,
          message: health.message,
        })
      })
      .collect();
    snapshot.entries.sort_by(|left, right| {
      health_rank(Some(&left.health))
        .cmp(&health_rank(Some(&right.health)))
        .then_with(|| left.latency_ms.cmp(&right.latency_ms))
        .then_with(|| left.entry_id.cmp(&right.entry_id))
    });
    snapshot.revision = parsed.revision;
    snapshot.available = true;
    snapshot
  }

  pub fn start(&self) -> Result<(), String> {
    self.stop()?;
    let exe_path = self.exe_path();
    if !exe_path.is_file() {
      return Err(format!(
        "未找到 Proxy Gate 可执行文件：{}。可通过 PROXY_GATE_EXE 指定路径。",
        exe_path.display()
      ));
    }
    self.ensure_config()?;
    let config_path = self.config_path();
    let mut command = Command::new(&exe_path);
    command
      .arg("--config")
      .arg(&config_path)
      .arg("run")
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::null());
    if let Some(parent) = config_path.parent() {
      command.current_dir(parent);
    }
    configure_hidden_process(&mut command);
    let mut child = command
      .spawn()
      .map_err(|error| format!("无法启动 Proxy Gate：{}（{}）", exe_path.display(), error))?;
    std::thread::sleep(Duration::from_millis(350));
    if let Some(status) = child
      .try_wait()
      .map_err(|_| "无法检查 Proxy Gate 启动状态".to_string())?
    {
      return Err(format!("Proxy Gate 启动后立即退出：{status}"));
    }
    let mut managed = self
      .managed_child
      .lock()
      .map_err(|_| "Proxy Gate 进程状态锁不可用".to_string())?;
    *managed = Some(child);
    Ok(())
  }

  pub fn stop(&self) -> Result<(), String> {
    let mut managed = self
      .managed_child
      .lock()
      .map_err(|_| "Proxy Gate 进程状态锁不可用".to_string())?;
    let Some(mut child) = managed.take() else {
      drop(managed);
      if self.externally_running() {
        return Err("Proxy Gate 正由托盘或其他进程运行，请从对应托盘退出后再重试".to_string());
      }
      return Ok(());
    };
    match child.try_wait() {
      Ok(Some(_)) => return Ok(()),
      Ok(None) => {}
      Err(_) => return Err("无法检查 Proxy Gate 进程状态".to_string()),
    }
    child
      .kill()
      .map_err(|_| "无法停止 Proxy Gate 进程".to_string())?;
    let _ = child.wait();
    Ok(())
  }

  pub fn assign(
    &self,
    provider_id: &str,
    uuid: &str,
    entry_id: &str,
  ) -> Result<ProxyGateAssignment, String> {
    if provider_id.trim().is_empty() || uuid.trim().is_empty() {
      return Err("providerId 和 uuid 不能为空".to_string());
    }
    if entry_id.trim().is_empty() {
      return Err("请选择一个代理池入口".to_string());
    }
    if self.externally_running() {
      return Err(
        "Proxy Gate 正由托盘或其他进程运行。请先退出该进程，再由叠号启动并绑定账号".to_string(),
      );
    }
    self.ensure_config()?;
    let was_running = self.is_running();
    let account_id = gate_account_id(provider_id, uuid);
    self.run_gate(&["assign", &account_id, entry_id])?;
    let restart_result = if was_running { self.start() } else { Ok(()) };
    if let Err(error) = restart_result {
      let _ = self.run_gate(&["unassign", &account_id]);
      return Err(error);
    }
    let snapshot = self.snapshot();
    snapshot
      .assignments
      .into_iter()
      .find(|assignment| assignment.account_id == account_id)
      .ok_or_else(|| "Proxy Gate 已执行绑定，但无法读取绑定结果".to_string())
  }

  pub fn unassign(&self, provider_id: &str, uuid: &str) -> Result<(), String> {
    if self.externally_running() {
      return Err(
        "Proxy Gate 正由托盘或其他进程运行。请先退出该进程，再由叠号解除绑定".to_string(),
      );
    }
    self.ensure_config()?;
    let was_running = self.is_running();
    let account_id = gate_account_id(provider_id, uuid);
    self.run_gate(&["unassign", &account_id])?;
    if was_running {
      self.start()?;
    }
    Ok(())
  }

  pub fn set_pool_source(&self, source_path: &str) -> Result<(), String> {
    if self.externally_running() {
      return Err(
        "Proxy Gate 正由托盘或其他进程运行。请先退出该进程，再由叠号切换代理池".to_string(),
      );
    }
    let source = if source_path.trim().is_empty() {
      let discovered = self.discovered_pool_paths()?;
      match discovered.as_slice() {
        [] => return Err("Proxy Gate 未自动发现 sidecar.yaml，请手工选择代理池来源".to_string()),
        [source] => source.clone(),
        _ => {
          return Err(format!(
            "自动发现多个代理池，请手工选择一个：{}",
            discovered
              .iter()
              .map(|path| path.display().to_string())
              .collect::<Vec<_>>()
              .join(" | ")
          ))
        }
      }
    } else {
      normalized_file(source_path)?
    };
    let mut config = self.ensure_config()?;
    if !config.assignments.is_empty() {
      return Err("Proxy Gate 已有账号绑定。请先逐账号解除绑定，再更换代理池来源。".to_string());
    }
    config.pool_path = Some(source);
    self.save_config(&config)?;
    if self.is_running() {
      self.start()?;
    }
    Ok(())
  }

  fn inspect_pool(&self) -> Result<InspectOutput, String> {
    let output = self.run_gate(&["inspect", "--json"])?;
    serde_json::from_str(&output)
      .map_err(|error| format!("Proxy Gate 返回的代理池 JSON 无效：{error}"))
  }

  fn run_gate(&self, args: &[&str]) -> Result<String, String> {
    let exe_path = self.exe_path();
    if !exe_path.is_file() {
      return Err(format!(
        "未找到 Proxy Gate 可执行文件：{}",
        exe_path.display()
      ));
    }
    let config_path = self.config_path();
    let mut command = Command::new(&exe_path);
    command.arg("--config").arg(&config_path).args(args);
    if let Some(parent) = config_path.parent() {
      command.current_dir(parent);
    }
    configure_hidden_process(&mut command);
    let output = command
      .output()
      .map_err(|error| format!("无法运行 Proxy Gate：{error}"))?;
    if !output.status.success() {
      let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
      let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
      let detail = if !stderr.is_empty() { stderr } else { stdout };
      return Err(if detail.is_empty() {
        format!("Proxy Gate 命令失败：{}", output.status)
      } else {
        format!("Proxy Gate 命令失败：{detail}")
      });
    }
    String::from_utf8(output.stdout).map_err(|_| "Proxy Gate 输出不是有效 UTF-8".to_string())
  }

  fn ensure_config(&self) -> Result<GateConfig, String> {
    let path = self.config_path();
    if path.exists() {
      let mut config = read_config(&path)?;
      if config.pool_path.is_none() {
        config.pool_path = self.selected_pool_path();
        if config.pool_path.is_some() {
          self.save_config(&config)?;
        }
      }
      return Ok(config);
    }
    let config = GateConfig {
      version: 1,
      pool_path: self.selected_pool_path().or_else(|| {
        self
          .discovered_pool_paths()
          .ok()
          .and_then(|sources| (sources.len() == 1).then(|| sources[0].clone()))
      }),
      listen_host: "127.0.0.1".to_string(),
      port_start: 19100,
      port_end: 19199,
      health_target: "https://muse.ai/".to_string(),
      health_interval_seconds: 60,
      allow_handshake_only: true,
      assignments: Vec::new(),
    };
    self.save_config(&config)?;
    Ok(config)
  }

  fn selected_pool_path(&self) -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os(POOL_OVERRIDE) {
      return normalized_file(PathBuf::from(raw).to_string_lossy().as_ref()).ok();
    }
    let path = self.data_dir.join(PROXY_POOL_SOURCE_FILE);
    let bytes = fs::read(path).ok()?;
    let selection: ProxyPoolSource = serde_json::from_slice(&bytes).ok()?;
    if selection.version != 1 {
      return None;
    }
    normalized_file(&selection.source_path).ok()
  }

  fn save_config(&self, config: &GateConfig) -> Result<(), String> {
    let path = self.config_path();
    if let Some(parent) = path.parent() {
      fs::create_dir_all(parent)
        .map_err(|_| format!("无法创建 Proxy Gate 配置目录：{}", parent.display()))?;
    }
    let bytes =
      serde_json::to_vec_pretty(config).map_err(|_| "无法序列化 Proxy Gate 配置".to_string())?;
    let temp = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp, bytes).map_err(|_| "无法写入 Proxy Gate 配置".to_string())?;
    if path.exists() {
      fs::remove_file(&path).map_err(|_| "无法替换 Proxy Gate 配置".to_string())?;
    }
    fs::rename(&temp, &path).map_err(|_| "无法保存 Proxy Gate 配置".to_string())
  }

  fn read_status(&self) -> Option<ProxyGateStatus> {
    self.status().ok().flatten()
  }

  fn discovered_pool_paths(&self) -> Result<Vec<PathBuf>, String> {
    let output = self.run_gate(&["discover"])?;
    output
      .lines()
      .map(str::trim)
      .filter(|line| !line.is_empty() && *line != "未发现 sidecar.yaml")
      .map(normalized_file)
      .collect()
  }

  fn is_running(&self) -> bool {
    let Ok(mut managed) = self.managed_child.lock() else {
      return false;
    };
    let Some(child) = managed.as_mut() else {
      return false;
    };
    match child.try_wait() {
      Ok(None) => true,
      Ok(Some(_)) | Err(_) => {
        *managed = None;
        false
      }
    }
  }

  fn externally_running(&self) -> bool {
    if self.is_running() {
      return false;
    }
    self
      .ensure_config()
      .ok()
      .is_some_and(|config| listener_reachable(&config))
  }

  fn exe_path(&self) -> PathBuf {
    std::env::var_os(EXE_OVERRIDE)
      .map(PathBuf::from)
      .unwrap_or_else(|| self.data_dir.join(EXE_FILE))
  }

  fn config_path(&self) -> PathBuf {
    std::env::var_os(CONFIG_OVERRIDE)
      .map(PathBuf::from)
      .unwrap_or_else(|| self.data_dir.join(CONFIG_FILE))
  }

  fn status_path(&self) -> PathBuf {
    self.config_path().with_file_name(STATUS_FILE)
  }
}

impl Drop for ProxyGateAdapter {
  fn drop(&mut self) {
    // The managed process intentionally outlives this adapter so account traffic
    // is not cut when the management window is recreated. Users stop it explicitly.
  }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProxyPoolSource {
  version: u32,
  source_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateConfig {
  version: u32,
  pool_path: Option<PathBuf>,
  listen_host: String,
  port_start: u16,
  port_end: u16,
  health_target: String,
  health_interval_seconds: u64,
  allow_handshake_only: bool,
  #[serde(default)]
  assignments: Vec<GateConfigAssignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GateConfigAssignment {
  account_id: String,
  entry_id: String,
  fingerprint: String,
  label: String,
  port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InspectOutput {
  source_path: Option<String>,
  entries: Vec<PoolEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PoolEntry {
  id: String,
  label: String,
  proxy_url: String,
  fingerprint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListOutput {
  revision: String,
  entries: Vec<PoolEntry>,
  health: Vec<ListHealthEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListHealthEntry {
  entry_id: String,
  status: String,
  latency_ms: u64,
  message: Option<String>,
}

fn read_config(path: &Path) -> Result<GateConfig, String> {
  let bytes =
    fs::read(path).map_err(|_| format!("无法读取 Proxy Gate 配置：{}", path.display()))?;
  let config: GateConfig = serde_json::from_slice(&bytes)
    .map_err(|_| format!("Proxy Gate 配置格式无效：{}", path.display()))?;
  if config.version != 1 {
    return Err(format!("不支持的 Proxy Gate 配置版本 {}", config.version));
  }
  if config.listen_host != "127.0.0.1" && config.listen_host != "::1" {
    return Err("Proxy Gate 只允许监听本机回环地址".to_string());
  }
  if config.port_start > config.port_end {
    return Err("Proxy Gate 端口范围无效".to_string());
  }
  Ok(config)
}

fn assignments_without_pool(config: &GateConfig) -> Vec<ProxyGateAssignment> {
  config
    .assignments
    .iter()
    .map(|assignment| {
      let (provider_id, uuid) = split_account_id(&assignment.account_id);
      ProxyGateAssignment {
        account_id: assignment.account_id.clone(),
        provider_id,
        uuid,
        account_alias: None,
        entry_id: assignment.entry_id.clone(),
        display_name: assignment.label.clone(),
        fingerprint: assignment.fingerprint.clone(),
        port: assignment.port,
        endpoint: format!("http://{}:{}", config.listen_host, assignment.port),
        binding_valid: false,
        allowed: Some(false),
        health: None,
        latency_ms: None,
        error: Some("代理池无法读取".to_string()),
      }
    })
    .collect()
}

fn split_account_id(account_id: &str) -> (String, String) {
  account_id
    .split_once(':')
    .map(|(provider, uuid)| (provider.to_string(), uuid.to_string()))
    .unwrap_or_else(|| (String::new(), account_id.to_string()))
}

fn gate_account_id(provider_id: &str, uuid: &str) -> String {
  format!(
    "{}:{}",
    provider_id.trim().to_ascii_lowercase(),
    uuid.trim()
  )
}

fn proxy_type(proxy_url: &str) -> String {
  url::Url::parse(proxy_url)
    .ok()
    .and_then(|url| url.scheme().split('+').next().map(str::to_string))
    .unwrap_or_else(|| "proxy".to_string())
}

fn sort_entries(entries: &mut [ProxyGateEntry]) {
  entries.sort_by(|left, right| {
    health_rank(left.health.as_deref())
      .cmp(&health_rank(right.health.as_deref()))
      .then_with(|| {
        left
          .latency_ms
          .unwrap_or(u64::MAX)
          .cmp(&right.latency_ms.unwrap_or(u64::MAX))
      })
      .then_with(|| left.entry_id.cmp(&right.entry_id))
  });
}

fn health_rank(health: Option<&str>) -> u8 {
  match health {
    Some("available") => 0,
    Some("handshake_only") => 1,
    Some("unavailable") => 2,
    _ => 3,
  }
}

fn normalized_file(raw: &str) -> Result<PathBuf, String> {
  let path = PathBuf::from(raw.trim());
  if raw.trim().is_empty() || !path.is_absolute() {
    return Err("代理池来源必须是本机文件的绝对路径".to_string());
  }
  let canonical = fs::canonicalize(&path).map_err(|_| "代理池来源不存在或不可读取".to_string())?;
  if !canonical.is_file() {
    return Err("代理池来源必须是文件".to_string());
  }
  Ok(canonical)
}

fn listener_reachable(config: &GateConfig) -> bool {
  let ip = match config.listen_host.as_str() {
    "127.0.0.1" => IpAddr::V4(Ipv4Addr::LOCALHOST),
    "::1" => IpAddr::V6(Ipv6Addr::LOCALHOST),
    _ => return false,
  };
  config.assignments.iter().any(|assignment| {
    TcpStream::connect_timeout(
      &SocketAddr::new(ip, assignment.port),
      Duration::from_millis(120),
    )
    .is_ok()
  })
}

#[cfg(windows)]
fn configure_hidden_process(command: &mut Command) {
  use std::os::windows::process::CommandExt;
  command.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn configure_hidden_process(_command: &mut Command) {}

#[cfg(test)]
mod tests {
  use super::{
    gate_account_id, health_rank, proxy_type, sort_entries, split_account_id, ProxyGateEntry,
  };

  #[test]
  fn account_ids_are_provider_scoped() {
    assert_eq!(gate_account_id("Muse", "abc"), "muse:abc".to_string());
    assert_eq!(
      split_account_id("hark:abc"),
      ("hark".to_string(), "abc".to_string())
    );
  }

  #[test]
  fn proxy_types_are_derived_from_url() {
    assert_eq!(proxy_type("socks5://127.0.0.1:7891"), "socks5");
    assert_eq!(proxy_type("http://127.0.0.1:7891"), "http");
  }

  #[test]
  fn health_sort_put_available_first_and_unknown_last() {
    assert_eq!(health_rank(Some("available")), 0);
    assert_eq!(health_rank(Some("handshake_only")), 1);
    assert_eq!(health_rank(Some("unavailable")), 2);
    assert_eq!(health_rank(None), 3);

    let mut entries = vec![
      ProxyGateEntry {
        entry_id: "c".to_string(),
        display_name: String::new(),
        r#type: "socks5".to_string(),
        fingerprint: String::new(),
        chain_depth: None,
        health: None,
        latency_ms: None,
        message: None,
      },
      ProxyGateEntry {
        entry_id: "b".to_string(),
        display_name: String::new(),
        r#type: "socks5".to_string(),
        fingerprint: String::new(),
        chain_depth: None,
        health: Some("handshake_only".to_string()),
        latency_ms: Some(50),
        message: None,
      },
      ProxyGateEntry {
        entry_id: "a".to_string(),
        display_name: String::new(),
        r#type: "socks5".to_string(),
        fingerprint: String::new(),
        chain_depth: None,
        health: Some("available".to_string()),
        latency_ms: Some(80),
        message: None,
      },
    ];
    sort_entries(&mut entries);
    assert_eq!(
      entries
        .iter()
        .map(|entry| entry.entry_id.as_str())
        .collect::<Vec<_>>(),
      vec!["a", "b", "c"]
    );
  }
}
