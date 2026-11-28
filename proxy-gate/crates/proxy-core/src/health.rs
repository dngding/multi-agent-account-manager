use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{lookup_host, TcpStream};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::timeout;
use url::Url;

use crate::{CoreError, PoolEntry};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(8);
const PAGE_TIMEOUT: Duration = Duration::from_secs(6);
const MAX_PROXY_HEADER: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Available,
    HandshakeOnly,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryHealth {
    pub entry_id: String,
    pub status: HealthStatus,
    pub latency_ms: u64,
    pub message: Option<String>,
}

pub async fn probe_entries(
    entries: &[PoolEntry],
    target: &Url,
    concurrency: usize,
) -> Result<Vec<EntryHealth>, CoreError> {
    if target.host_str().is_none() {
        return Err(CoreError::Network("健康检查目标缺少主机名".to_string()));
    }
    if entries.is_empty() {
        return Ok(Vec::new());
    }

    let semaphore = Arc::new(Semaphore::new(concurrency.max(1)));
    let target = target.clone();
    let mut tasks = JoinSet::new();
    for (index, entry) in entries.iter().cloned().enumerate() {
        let semaphore = Arc::clone(&semaphore);
        let target = target.clone();
        tasks.spawn(async move {
            let _permit = semaphore
                .acquire_owned()
                .await
                .map_err(|_| CoreError::Network("健康检测调度器已关闭".to_string()))?;
            Ok::<_, CoreError>((index, probe_entry(entry, target).await))
        });
    }

    let mut results = Vec::with_capacity(entries.len());
    while let Some(task) = tasks.join_next().await {
        results.push(task.map_err(|_| CoreError::Network("健康检测任务异常结束".to_string()))??);
    }
    results.sort_by_key(|(index, _)| *index);
    Ok(results.into_iter().map(|(_, result)| result).collect())
}

pub async fn open_tunnel(
    proxy: &Url,
    target_host: &str,
    target_port: u16,
) -> Result<TcpStream, CoreError> {
    if !matches!(proxy.scheme(), "http" | "socks5") {
        return Err(CoreError::Network(
            "代理协议只支持 http:// 或 socks5://".to_string(),
        ));
    }
    if !proxy.username().is_empty() || proxy.password().is_some() {
        return Err(CoreError::Network("当前版本不支持带认证的代理".to_string()));
    }
    let proxy_host = proxy
        .host_str()
        .ok_or_else(|| CoreError::Network("代理地址缺少主机名".to_string()))?;
    let proxy_port = proxy
        .port()
        .ok_or_else(|| CoreError::Network("代理地址缺少端口".to_string()))?;

    let addresses = timeout(
        CONNECT_TIMEOUT,
        lookup_host((proxy_host.to_string(), proxy_port)),
    )
    .await
    .map_err(|_| CoreError::Network("代理服务器域名解析超时".to_string()))?
    .map_err(|_| CoreError::Network("无法解析代理服务器地址".to_string()))?
    .collect::<Vec<_>>();
    if addresses.is_empty() {
        return Err(CoreError::Network("无法解析代理服务器地址".to_string()));
    }

    let connect_deadline = Instant::now() + CONNECT_TIMEOUT;
    let mut stream = None;
    for address in addresses {
        let remaining = connect_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        if let Ok(Ok(connected)) = timeout(remaining, TcpStream::connect(address)).await {
            stream = Some(connected);
            break;
        }
    }
    let mut stream =
        stream.ok_or_else(|| CoreError::Network("代理服务器当前无法连接".to_string()))?;

    match proxy.scheme() {
        "http" => {
            timeout(
                HANDSHAKE_TIMEOUT,
                http_connect(&mut stream, target_host, target_port),
            )
            .await
            .map_err(|_| CoreError::Network("HTTP CONNECT 握手超时".to_string()))??;
        }
        "socks5" => {
            timeout(
                HANDSHAKE_TIMEOUT,
                socks5_connect(&mut stream, target_host, target_port),
            )
            .await
            .map_err(|_| CoreError::Network("SOCKS5 握手超时".to_string()))??;
        }
        _ => {
            return Err(CoreError::Network(
                "代理协议只支持 http:// 或 socks5://".to_string(),
            ))
        }
    }
    Ok(stream)
}

async fn probe_entry(entry: PoolEntry, target: Url) -> EntryHealth {
    let started = Instant::now();
    let target_host = match target.host_str() {
        Some(host) => host.to_string(),
        None => {
            return EntryHealth {
                entry_id: entry.id,
                status: HealthStatus::Unavailable,
                latency_ms: 0,
                message: Some("健康检查目标缺少主机名".to_string()),
            }
        }
    };
    let proxy = match Url::parse(&entry.proxy_url) {
        Ok(proxy) => proxy,
        Err(_) => {
            return EntryHealth {
                entry_id: entry.id,
                status: HealthStatus::Unavailable,
                latency_ms: 0,
                message: Some("代理入口地址无效".to_string()),
            }
        }
    };

    let handshake = open_tunnel(&proxy, &target_host, 443).await.map(drop);
    let page = probe_http_page(&proxy, &target_host).await;
    let status = match (&handshake, &page) {
        (Ok(()), Ok(())) => HealthStatus::Available,
        (Ok(()), Err(_)) => HealthStatus::HandshakeOnly,
        _ => HealthStatus::Unavailable,
    };
    let message = match status {
        HealthStatus::Available => None,
        HealthStatus::HandshakeOnly => Some(format!(
            "443 隧道可用，但页面请求失败：{}",
            page.err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "未知错误".to_string())
        )),
        HealthStatus::Unavailable if handshake.is_err() => Some(format!(
            "代理握手失败：{}",
            handshake
                .err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "未知错误".to_string())
        )),
        HealthStatus::Unavailable => Some(format!(
            "页面请求失败：{}",
            page.err()
                .map(|error| error.to_string())
                .unwrap_or_else(|| "未知错误".to_string())
        )),
    };

    EntryHealth {
        entry_id: entry.id,
        status,
        latency_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        message,
    }
}

async fn probe_http_page(proxy: &Url, target_host: &str) -> Result<(), CoreError> {
    let mut stream = timeout(PAGE_TIMEOUT, open_tunnel(proxy, target_host, 80))
        .await
        .map_err(|_| CoreError::Network("页面探测连接超时".to_string()))??;
    let request = format!("GET / HTTP/1.1\r\nHost: {target_host}\r\nConnection: close\r\n\r\n");
    timeout(PAGE_TIMEOUT, stream.write_all(request.as_bytes()))
        .await
        .map_err(|_| CoreError::Network("页面请求写入超时".to_string()))?
        .map_err(|_| CoreError::Network("无法通过代理发送页面请求".to_string()))?;

    let header = timeout(PAGE_TIMEOUT, read_header(&mut stream))
        .await
        .map_err(|_| CoreError::Network("页面响应超时".to_string()))??;
    let response = std::str::from_utf8(&header)
        .map_err(|_| CoreError::Network("页面响应不是有效 HTTP".to_string()))?;
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| CoreError::Network("页面响应缺少 HTTP 状态码".to_string()))?;
    match status {
        407 => Err(CoreError::Network(
            "代理要求身份验证；当前版本不支持带认证的代理".to_string(),
        )),
        200..=499 => Ok(()),
        _ => Err(CoreError::Network(format!(
            "代理页面请求返回 HTTP {status}"
        ))),
    }
}

async fn http_connect(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<(), CoreError> {
    let authority = authority(target_host, target_port);
    let request = format!(
        "CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|_| CoreError::Network("无法向 HTTP 代理发送连接请求".to_string()))?;
    let header = read_header(stream).await?;
    let response = std::str::from_utf8(&header)
        .map_err(|_| CoreError::Network("HTTP 代理返回了无效响应".to_string()))?;
    let mut parts = response
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    let version = parts.next();
    let status = parts.next();
    if matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) && status == Some("200") {
        return Ok(());
    }
    if status == Some("407") {
        return Err(CoreError::Network(
            "HTTP 代理要求身份验证；当前版本不支持带认证的代理".to_string(),
        ));
    }
    Err(CoreError::Network(
        "HTTP 代理未能连接到目标站点".to_string(),
    ))
}

async fn socks5_connect(
    stream: &mut TcpStream,
    target_host: &str,
    target_port: u16,
) -> Result<(), CoreError> {
    stream
        .write_all(&[0x05, 0x01, 0x00])
        .await
        .map_err(|_| CoreError::Network("无法向 SOCKS5 代理发送握手请求".to_string()))?;
    let mut method = [0u8; 2];
    stream
        .read_exact(&mut method)
        .await
        .map_err(|_| CoreError::Network("SOCKS5 代理未能完成握手".to_string()))?;
    if method != [0x05, 0x00] {
        return Err(CoreError::Network(
            "SOCKS5 代理要求身份验证；当前版本不支持带认证的代理".to_string(),
        ));
    }

    let host_bytes = target_host.as_bytes();
    let host_len = u8::try_from(host_bytes.len())
        .map_err(|_| CoreError::Network("目标域名过长".to_string()))?;
    let mut request = Vec::with_capacity(host_bytes.len() + 7);
    request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_len]);
    request.extend_from_slice(host_bytes);
    request.extend_from_slice(&target_port.to_be_bytes());
    stream
        .write_all(&request)
        .await
        .map_err(|_| CoreError::Network("无法向 SOCKS5 代理发送目标请求".to_string()))?;

    let mut response = [0u8; 4];
    stream
        .read_exact(&mut response)
        .await
        .map_err(|_| CoreError::Network("SOCKS5 代理未能连接目标站点".to_string()))?;
    if response[0] != 0x05 || response[1] != 0x00 {
        return Err(CoreError::Network(
            "SOCKS5 代理未能连接目标站点".to_string(),
        ));
    }
    let address_len = match response[3] {
        0x01 => 4,
        0x04 => 16,
        0x03 => {
            let mut length = [0u8; 1];
            stream
                .read_exact(&mut length)
                .await
                .map_err(|_| CoreError::Network("SOCKS5 代理返回了无效响应".to_string()))?;
            length[0] as usize
        }
        _ => return Err(CoreError::Network("SOCKS5 代理返回了无效响应".to_string())),
    };
    let mut address_and_port = vec![0u8; address_len + 2];
    stream
        .read_exact(&mut address_and_port)
        .await
        .map_err(|_| CoreError::Network("SOCKS5 代理返回了无效响应".to_string()))?;
    Ok(())
}

async fn read_header(stream: &mut TcpStream) -> Result<Vec<u8>, CoreError> {
    let mut header = Vec::with_capacity(256);
    while header.len() < MAX_PROXY_HEADER {
        let mut byte = [0u8; 1];
        stream
            .read_exact(&mut byte)
            .await
            .map_err(|_| CoreError::Network("代理提前关闭连接".to_string()))?;
        header.push(byte[0]);
        if header.ends_with(b"\r\n\r\n") {
            return Ok(header);
        }
    }
    Err(CoreError::Network("代理响应头超过 16 KiB".to_string()))
}

fn authority(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

#[cfg(test)]
mod tests {
    use super::{probe_entries, HealthStatus};
    use crate::PoolEntry;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use url::Url;

    fn entry(proxy_url: String) -> PoolEntry {
        PoolEntry {
            id: "entry-1".to_string(),
            label: "entry-1".to_string(),
            proxy_url,
            fingerprint: "fingerprint".to_string(),
            duplicate_count: 1,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probes_http_proxy_page_and_tunnel() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&request);
                if text.contains(":80 ") {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                    let mut page = Vec::new();
                    while !page.ends_with(b"\r\n\r\n") {
                        stream.read_exact(&mut byte).await.unwrap();
                        page.push(byte[0]);
                    }
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                } else {
                    stream
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                }
            }
        });

        let results = probe_entries(
            &[entry(format!("http://{address}"))],
            &Url::parse("https://muse.ai/").unwrap(),
            2,
        )
        .await
        .unwrap();
        assert_eq!(results[0].status, HealthStatus::Available);
        server.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn probes_socks5_proxy_page_and_tunnel() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut greeting = [0u8; 3];
                stream.read_exact(&mut greeting).await.unwrap();
                stream.write_all(&[0x05, 0x00]).await.unwrap();
                let mut head = [0u8; 4];
                stream.read_exact(&mut head).await.unwrap();
                let host_len = match head[3] {
                    0x01 => 4,
                    0x04 => 16,
                    0x03 => {
                        let mut length = [0u8; 1];
                        stream.read_exact(&mut length).await.unwrap();
                        length[0] as usize
                    }
                    _ => panic!("invalid SOCKS5 address type"),
                };
                let mut host = vec![0u8; host_len];
                stream.read_exact(&mut host).await.unwrap();
                let mut port = [0u8; 2];
                stream.read_exact(&mut port).await.unwrap();
                let target_port = u16::from_be_bytes(port);
                stream
                    .write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0])
                    .await
                    .unwrap();
                if target_port == 80 {
                    let mut page = Vec::new();
                    let mut byte = [0u8; 1];
                    while !page.ends_with(b"\r\n\r\n") {
                        stream.read_exact(&mut byte).await.unwrap();
                        page.push(byte[0]);
                    }
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await
                        .unwrap();
                }
            }
        });

        let results = probe_entries(
            &[entry(format!("socks5://{address}"))],
            &Url::parse("https://muse.ai/").unwrap(),
            2,
        )
        .await
        .unwrap();
        assert_eq!(results[0].status, HealthStatus::Available);
        server.await.unwrap();
    }
}
