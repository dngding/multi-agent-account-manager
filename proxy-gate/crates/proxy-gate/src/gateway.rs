use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use maam_proxy_core::{open_tunnel, probe_entries, EntryHealth, HealthStatus};
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, RwLock};
use tokio::time::timeout;

use crate::config::{Assignment, GateConfig};

const MAX_PROXY_REQUEST_BYTES: usize = 16 * 1024;
const CLIENT_REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const HEALTH_CONCURRENCY: usize = 16;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateStatus {
    pub updated_at_ms: u64,
    pub pool_path: Option<String>,
    pub pool_revision: Option<String>,
    pub error: Option<String>,
    pub assignments: Vec<AssignmentStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentStatus {
    pub account_id: String,
    pub entry_id: String,
    pub fingerprint: String,
    pub label: String,
    pub endpoint: String,
    pub upstream_proxy_url: Option<String>,
    pub allowed: bool,
    pub binding_valid: bool,
    pub health: Option<HealthStatus>,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct RuntimeAssignment {
    configured: Assignment,
    endpoint: String,
    upstream_proxy_url: Option<String>,
    allowed: bool,
    binding_valid: bool,
    health: Option<HealthStatus>,
    latency_ms: Option<u64>,
    error: Option<String>,
    generation: watch::Sender<u64>,
}

pub struct Gateway {
    inner: Arc<RwLock<HashMap<String, RuntimeAssignment>>>,
    config_path: PathBuf,
    status_path: PathBuf,
    initial_config: GateConfig,
}

impl Gateway {
    pub async fn start(config_path: PathBuf) -> Result<Self> {
        let mut config = GateConfig::load_or_create(&config_path)?;
        let pool_path = config.resolve_pool_path()?;
        config.save(&config_path)?;
        let pool = maam_proxy_core::load_pool(&pool_path)?;
        let entries = pool
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect::<HashMap<_, _>>();

        let mut runtime = HashMap::new();
        for assignment in &config.assignments {
            let endpoint = endpoint(&config.listen_host, assignment.port);
            let entry = entries.get(assignment.entry_id.as_str()).copied();
            let binding_valid =
                entry.is_some_and(|entry| entry.fingerprint == assignment.fingerprint);
            let error = if entry.is_none() {
                Some(format!(
                    "绑定的代理入口 {} 不在当前代理池中",
                    assignment.entry_id
                ))
            } else if !binding_valid {
                Some("代理入口定义已变化，需要用户重新确认绑定".to_string())
            } else {
                None
            };
            runtime.insert(
                assignment.account_id.clone(),
                RuntimeAssignment {
                    configured: assignment.clone(),
                    endpoint,
                    upstream_proxy_url: entry.map(|entry| entry.proxy_url.clone()),
                    allowed: false,
                    binding_valid,
                    health: None,
                    latency_ms: None,
                    error,
                    generation: watch::channel(0).0,
                },
            );
        }

        let inner = Arc::new(RwLock::new(runtime));
        let status_path = status_path_for(&config_path);
        let gateway = Self {
            inner,
            config_path,
            status_path,
            initial_config: config,
        };

        gateway.refresh_once().await?;
        gateway.bind_listeners().await?;

        let monitor = gateway.clone_handle();
        tokio::spawn(async move {
            if let Err(error) = monitor.monitor_loop().await {
                eprintln!("Proxy Gate 后台监控停止: {error:#}");
            }
        });

        Ok(gateway)
    }

    pub async fn wait_forever(self) -> Result<()> {
        if let Err(error) = tokio::signal::ctrl_c().await {
            bail!("无法监听退出信号: {error}");
        }
        Ok(())
    }

    async fn bind_listeners(&self) -> Result<()> {
        let assignments = {
            let state = self.inner.read().await;
            state
                .values()
                .map(|runtime| {
                    (
                        runtime.configured.account_id.clone(),
                        runtime.endpoint.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        for (account_id, endpoint) in assignments {
            let listener = TcpListener::bind(&endpoint)
                .await
                .with_context(|| format!("无法监听账号 {} 的本机入口 {endpoint}", account_id))?;
            let state = Arc::clone(&self.inner);
            let listener_account_id = account_id.clone();
            tokio::spawn(async move {
                serve_listener(listener, state, listener_account_id).await;
            });
            println!("账号 {account_id} -> http://{endpoint}");
        }
        Ok(())
    }

    async fn monitor_loop(&self) -> Result<()> {
        loop {
            let delay = self.initial_config.health_interval_seconds;
            tokio::time::sleep(Duration::from_secs(delay)).await;
            if let Err(error) = self.refresh_once().await {
                self.block_all(&format!("刷新代理池失败: {error:#}")).await;
            }
        }
    }

    async fn refresh_once(&self) -> Result<()> {
        let config = GateConfig::load(&self.config_path)?;
        if config.assignments != self.initial_config.assignments {
            self.block_all("配置文件中的账号绑定已改变，请重启 Proxy Gate 以加载新端口")
                .await;
            bail!("账号绑定配置在运行期间发生变化");
        }

        let pool_path = config.pool_path.clone().context("代理池路径为空")?;
        let pool = maam_proxy_core::load_pool(&pool_path)?;
        let entries_by_id = pool
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect::<HashMap<_, _>>();

        let assigned_entry_ids = config
            .assignments
            .iter()
            .map(|assignment| assignment.entry_id.as_str())
            .collect::<HashSet<_>>();
        let assigned_entries = pool
            .entries
            .iter()
            .filter(|entry| assigned_entry_ids.contains(entry.id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        let target = url::Url::parse(&config.health_target)?;
        let health = if assigned_entries.is_empty() {
            Vec::new()
        } else {
            probe_entries(&assigned_entries, &target, HEALTH_CONCURRENCY).await?
        };
        let health_by_id = health
            .iter()
            .map(|health| (health.entry_id.as_str(), health))
            .collect::<HashMap<_, _>>();

        let updates = {
            let mut state = self.inner.write().await;
            for assignment in &config.assignments {
                let Some(runtime) = state.get_mut(&assignment.account_id) else {
                    continue;
                };
                let entry = entries_by_id.get(assignment.entry_id.as_str()).copied();
                let entry_health = health_by_id.get(assignment.entry_id.as_str()).copied();
                runtime.binding_valid =
                    entry.is_some_and(|entry| entry.fingerprint == assignment.fingerprint);
                runtime.upstream_proxy_url = entry.map(|entry| entry.proxy_url.clone());
                runtime.health = entry_health.map(|health| health.status);
                runtime.latency_ms = entry_health.map(|health| health.latency_ms);

                let prior_allowed = runtime.allowed;
                let prior_error = runtime.error.clone();
                runtime.error = if entry.is_none() {
                    Some(format!(
                        "绑定的代理入口 {} 不在当前代理池中",
                        assignment.entry_id
                    ))
                } else if !runtime.binding_valid {
                    Some("代理入口定义已变化，需要用户重新确认绑定".to_string())
                } else if !health_allows(entry_health, config.allow_handshake_only) {
                    Some(
                        entry_health
                            .and_then(|health| health.message.clone())
                            .unwrap_or_else(|| "代理入口不可用".to_string()),
                    )
                } else {
                    None
                };
                runtime.allowed = runtime.error.is_none();

                if prior_allowed != runtime.allowed
                    || prior_error != runtime.error
                    || !runtime.allowed
                {
                    invalidate_connections(runtime);
                }
            }
            state.values().map(status_from_runtime).collect::<Vec<_>>()
        };

        write_status(&self.status_path, &pool_path, &pool.revision, None, updates)?;
        Ok(())
    }

    async fn block_all(&self, reason: &str) {
        let assignments = {
            let mut state = self.inner.write().await;
            for runtime in state.values_mut() {
                runtime.allowed = false;
                runtime.binding_valid = false;
                runtime.error = Some(reason.to_string());
                invalidate_connections(runtime);
            }
            state.values().map(status_from_runtime).collect::<Vec<_>>()
        };
        let pool_path = self.initial_config.pool_path.as_ref();
        let _ = write_status(
            &self.status_path,
            pool_path.unwrap_or(&PathBuf::new()),
            "",
            Some(reason.to_string()),
            assignments,
        );
    }

    fn clone_handle(&self) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::clone(&self.inner),
            config_path: self.config_path.clone(),
            status_path: self.status_path.clone(),
            initial_config: self.initial_config.clone(),
        })
    }
}

async fn serve_listener(
    listener: TcpListener,
    state: Arc<RwLock<HashMap<String, RuntimeAssignment>>>,
    account_id: String,
) {
    loop {
        match listener.accept().await {
            Ok((socket, _peer)) => {
                let state = Arc::clone(&state);
                let account_id = account_id.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_client(socket, state, &account_id).await {
                        eprintln!("账号 {account_id} 的代理连接结束: {error:#}");
                    }
                });
            }
            Err(error) => {
                eprintln!("账号 {account_id} 的本机监听失败: {error}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn handle_client(
    mut client: TcpStream,
    state: Arc<RwLock<HashMap<String, RuntimeAssignment>>>,
    account_id: &str,
) -> Result<()> {
    let request = timeout(CLIENT_REQUEST_TIMEOUT, read_proxy_request(&mut client))
        .await
        .context("读取浏览器代理请求超时")??;

    let (runtime, mut generation) = {
        let state = state.read().await;
        let runtime = state.get(account_id).cloned().context("账号没有代理绑定")?;
        let generation = runtime.generation.subscribe();
        (runtime, generation)
    };

    if !runtime.allowed {
        let reason = runtime.error.as_deref().unwrap_or("代理入口不可用");
        write_gateway_error(&mut client, 503, reason).await?;
        return Ok(());
    }
    let Some(proxy_url) = runtime
        .upstream_proxy_url
        .as_deref()
        .and_then(|value| url::Url::parse(value).ok())
    else {
        write_gateway_error(&mut client, 503, "代理绑定没有有效上游入口").await?;
        return Ok(());
    };

    let target = parse_connect_target(&request)?;
    let upstream = match open_tunnel(&proxy_url, &target.0, target.1).await {
        Ok(stream) => stream,
        Err(error) => {
            write_gateway_error(&mut client, 502, &format!("上游代理不可用: {error}")).await?;
            return Ok(());
        }
    };

    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\nProxy-Agent: Proxy-Gate\r\n\r\n")
        .await?;

    let mut upstream = upstream;
    tokio::select! {
        result = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {
            result.context("转发代理流量失败")?;
        }
        changed = generation.changed() => {
            if changed.is_ok() {
                let _ = client.shutdown().await;
            }
        }
    }
    Ok(())
}

async fn read_proxy_request(stream: &mut TcpStream) -> Result<String> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 512];
    while bytes.len() < MAX_PROXY_REQUEST_BYTES {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            bail!("浏览器在代理请求头完成前断开连接");
        }
        bytes.extend_from_slice(&chunk[..read]);
        if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            return String::from_utf8(bytes).context("代理请求头不是 UTF-8");
        }
    }
    bail!("代理请求头过大");
}

fn parse_connect_target(request: &str) -> Result<(String, u16)> {
    let first_line = request.lines().next().context("代理请求为空")?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next().context("代理请求缺少方法")?;
    let authority = parts.next().context("代理请求缺少目标")?;
    let version = parts.next().context("代理请求缺少 HTTP 版本")?;
    if method != "CONNECT" {
        bail!("当前仅支持 HTTP CONNECT 请求");
    }
    if !version.starts_with("HTTP/1.") {
        bail!("代理请求 HTTP 版本不受支持");
    }
    if authority.starts_with('[') {
        let closing = authority.find(']').context("IPv6 目标缺少右括号")?;
        let host = authority[1..closing].to_string();
        let port = authority[closing + 1..]
            .strip_prefix(':')
            .context("代理目标缺少端口")?
            .parse::<u16>()
            .context("代理目标端口无效")?;
        return Ok((host, port));
    }
    let (host, port) = authority.rsplit_once(':').context("代理目标缺少端口")?;
    if host.trim().is_empty() {
        bail!("代理目标主机为空");
    }
    Ok((
        host.to_string(),
        port.parse::<u16>().context("代理目标端口无效")?,
    ))
}

async fn write_gateway_error(stream: &mut TcpStream, status: u16, reason: &str) -> Result<()> {
    let reason = reason.replace(['\r', '\n'], " ");
    let body = format!("Proxy Gate: {reason}");
    let response = format!(
        "HTTP/1.1 {status} Proxy Gate\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn health_allows(health: Option<&EntryHealth>, allow_handshake_only: bool) -> bool {
    match health.map(|health| &health.status) {
        Some(HealthStatus::Available) => true,
        Some(HealthStatus::HandshakeOnly) => allow_handshake_only,
        Some(HealthStatus::Unavailable) | None => false,
    }
}

fn invalidate_connections(runtime: &mut RuntimeAssignment) {
    let next = runtime.generation.borrow().saturating_add(1);
    let _ = runtime.generation.send(next);
}

fn status_from_runtime(runtime: &RuntimeAssignment) -> AssignmentStatus {
    AssignmentStatus {
        account_id: runtime.configured.account_id.clone(),
        entry_id: runtime.configured.entry_id.clone(),
        fingerprint: runtime.configured.fingerprint.clone(),
        label: runtime.configured.label.clone(),
        endpoint: format!("http://{}", runtime.endpoint),
        upstream_proxy_url: runtime.upstream_proxy_url.clone(),
        allowed: runtime.allowed,
        binding_valid: runtime.binding_valid,
        health: runtime.health,
        latency_ms: runtime.latency_ms,
        error: runtime.error.clone(),
    }
}

fn endpoint(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn status_path_for(config_path: &Path) -> PathBuf {
    config_path.with_file_name("proxy-gate-status.json")
}

fn write_status(
    path: &Path,
    pool_path: &Path,
    revision: &str,
    error: Option<String>,
    mut assignments: Vec<AssignmentStatus>,
) -> Result<()> {
    assignments.sort_by(|left, right| left.account_id.cmp(&right.account_id));
    let status = GateStatus {
        updated_at_ms: now_ms(),
        pool_path: (!pool_path.as_os_str().is_empty()).then(|| pool_path.display().to_string()),
        pool_revision: (!revision.is_empty()).then(|| revision.to_string()),
        error,
        assignments,
    };
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&status)?)?;
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    std::fs::rename(temp, path)?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::{handle_client, health_allows, parse_connect_target, RuntimeAssignment};
    use crate::config::Assignment;
    use maam_proxy_core::{EntryHealth, HealthStatus};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::{watch, RwLock};

    #[test]
    fn parses_ipv4_and_ipv6_connect_targets() {
        assert_eq!(
            parse_connect_target("CONNECT muse.ai:443 HTTP/1.1\r\n\r\n").unwrap(),
            ("muse.ai".to_string(), 443)
        );
        assert_eq!(
            parse_connect_target("CONNECT [2606:4700::1111]:443 HTTP/1.1\r\n\r\n").unwrap(),
            ("2606:4700::1111".to_string(), 443)
        );
    }

    #[test]
    fn rejects_non_connect_requests() {
        assert!(parse_connect_target("GET / HTTP/1.1\r\n\r\n").is_err());
    }

    #[test]
    fn handshake_only_is_configurable() {
        let health = EntryHealth {
            entry_id: "entry".to_string(),
            status: HealthStatus::HandshakeOnly,
            latency_ms: 10,
            message: None,
        };
        assert!(health_allows(Some(&health), true));
        assert!(!health_allows(Some(&health), false));
    }

    fn runtime(
        allowed: bool,
        upstream_proxy_url: Option<String>,
        error: Option<&str>,
    ) -> RuntimeAssignment {
        RuntimeAssignment {
            configured: Assignment {
                account_id: "account-a".to_string(),
                entry_id: "entry-a".to_string(),
                fingerprint: "fingerprint".to_string(),
                label: "entry-a".to_string(),
                port: 19100,
            },
            endpoint: "127.0.0.1:19100".to_string(),
            upstream_proxy_url,
            allowed,
            binding_valid: true,
            health: Some(HealthStatus::Available),
            latency_ms: Some(1),
            error: error.map(str::to_string),
            generation: watch::channel(0).0,
        }
    }

    #[tokio::test]
    async fn blocked_gateway_returns_503_before_contacting_upstream() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (client, server) = tokio::join!(TcpStream::connect(address), async {
            listener.accept().await.unwrap().0
        });
        let mut client = client.unwrap();
        let mut state = HashMap::new();
        state.insert(
            "account-a".to_string(),
            runtime(false, None, Some("代理入口不可用")),
        );

        let handler = tokio::spawn(handle_client(
            server,
            Arc::new(RwLock::new(state)),
            "account-a",
        ));
        client
            .write_all(b"CONNECT muse.ai:443 HTTP/1.1\r\nHost: muse.ai:443\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        handler.await.unwrap().unwrap();
        let response = String::from_utf8_lossy(&response);
        assert!(response.starts_with("HTTP/1.1 503"));
        assert!(response.contains("代理入口不可用"));
    }

    #[tokio::test]
    async fn allowed_gateway_tunnels_through_http_upstream() {
        let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = upstream.local_addr().unwrap();
        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = Vec::new();
            let mut byte = [0u8; 1];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
            }
            assert!(String::from_utf8_lossy(&request).starts_with("CONNECT muse.ai:443"));
            stream
                .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
                .await
                .unwrap();
            let mut payload = [0u8; 4];
            stream.read_exact(&mut payload).await.unwrap();
            assert_eq!(&payload, b"ping");
            stream.write_all(b"pong").await.unwrap();
        });

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (client, server) = tokio::join!(TcpStream::connect(address), async {
            listener.accept().await.unwrap().0
        });
        let mut client = client.unwrap();
        let mut state = HashMap::new();
        state.insert(
            "account-a".to_string(),
            runtime(true, Some(format!("http://{upstream_address}")), None),
        );
        let handler = tokio::spawn(handle_client(
            server,
            Arc::new(RwLock::new(state)),
            "account-a",
        ));

        client
            .write_all(b"CONNECT muse.ai:443 HTTP/1.1\r\nHost: muse.ai:443\r\n\r\n")
            .await
            .unwrap();
        let mut header = Vec::new();
        let mut byte = [0u8; 1];
        while !header.ends_with(b"\r\n\r\n") {
            client.read_exact(&mut byte).await.unwrap();
            header.push(byte[0]);
        }
        assert!(String::from_utf8_lossy(&header).starts_with("HTTP/1.1 200"));

        client.write_all(b"ping").await.unwrap();
        let mut response = [0u8; 4];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(&response, b"pong");
        client.shutdown().await.unwrap();
        handler.await.unwrap().unwrap();
        upstream_task.await.unwrap();
    }
}
