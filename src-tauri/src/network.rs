use url::Url;

#[cfg(windows)]
use std::io::{Read, Write};
#[cfg(windows)]
use std::net::TcpStream;
#[cfg(windows)]
use std::time::{Duration, Instant};

pub fn parse_account_proxy(raw: &str) -> Result<Option<Url>, String> {
  let value = raw.trim();
  if value.is_empty() {
    return Ok(None);
  }

  let mut url = Url::parse(value).map_err(|_| "代理地址格式错误".to_string())?;
  if !matches!(url.scheme(), "http" | "socks5") {
    return Err("代理协议只支持 http:// 或 socks5://".to_string());
  }
  if url.host_str().is_none() || url.port().is_none() {
    return Err("代理地址必须包含主机名和端口".to_string());
  }
  if !url.username().is_empty() || url.password().is_some() {
    return Err("为避免明文保存凭据，暂不支持在代理地址中填写用户名或密码".to_string());
  }
  if (!url.path().is_empty() && url.path() != "/")
    || url.query().is_some()
    || url.fragment().is_some()
  {
    return Err("代理地址只能填写协议、主机和端口".to_string());
  }
  if url.path().is_empty() {
    url.set_path("/");
  }

  Ok(Some(url))
}

#[cfg(windows)]
pub async fn check_proxy_route(proxy_url: Url, target_url: Url) -> Result<(), String> {
  let proxy_host = proxy_url
    .host_str()
    .ok_or_else(|| "代理地址缺少主机名".to_string())?
    .to_string();
  let proxy_port = proxy_url
    .port()
    .ok_or_else(|| "代理地址缺少端口".to_string())?;
  let addresses = tokio::time::timeout(
    Duration::from_secs(4),
    tokio::net::lookup_host((proxy_host.as_str(), proxy_port)),
  )
  .await
  .map_err(|_| "代理服务器域名解析超时".to_string())?
  .map_err(|_| "无法解析代理服务器地址".to_string())?
  .collect::<Vec<_>>();

  if addresses.is_empty() {
    return Err("无法解析代理服务器地址".to_string());
  }

  tokio::task::spawn_blocking(move || check_proxy_route_sync(&addresses, &proxy_url, &target_url))
    .await
    .map_err(|_| "代理连通性检查任务异常结束".to_string())?
}

#[cfg(windows)]
fn check_proxy_route_sync(
  addresses: &[std::net::SocketAddr],
  proxy_url: &Url,
  target_url: &Url,
) -> Result<(), String> {
  let target_host = target_url
    .host_str()
    .ok_or_else(|| "服务地址缺少主机名".to_string())?;
  let target_port = target_url
    .port_or_known_default()
    .ok_or_else(|| "服务地址缺少端口".to_string())?;

  let connect_deadline = Instant::now() + Duration::from_secs(3);
  let mut stream = None;
  for address in addresses {
    let remaining = connect_deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
      break;
    }
    if let Ok(connected) = TcpStream::connect_timeout(address, remaining) {
      stream = Some(connected);
      break;
    }
  }
  let mut stream = stream
    .ok_or_else(|| "代理服务器当前无法连接，请检查地址、端口和网络".to_string())?;
  let handshake_deadline = Instant::now() + Duration::from_secs(8);

  match proxy_url.scheme() {
    "http" => check_http_proxy_tunnel(&mut stream, target_host, target_port, handshake_deadline),
    "socks5" => {
      check_socks5_proxy_tunnel(&mut stream, target_host, target_port, handshake_deadline)
    }
    _ => Err("代理协议只支持 http:// 或 socks5://".to_string()),
  }
}

#[cfg(windows)]
fn set_handshake_timeout(stream: &TcpStream, deadline: Instant) -> Result<(), String> {
  let remaining = deadline.saturating_duration_since(Instant::now());
  if remaining.is_zero() {
    return Err("代理连通性检查超时".to_string());
  }
  let timeout = Some(remaining);
  stream
    .set_read_timeout(timeout)
    .and_then(|()| stream.set_write_timeout(timeout))
    .map_err(|_| "无法设置代理检查超时".to_string())
}

#[cfg(windows)]
fn read_exact_before_deadline(
  stream: &mut TcpStream,
  mut buffer: &mut [u8],
  deadline: Instant,
  error_message: &str,
) -> Result<(), String> {
  while !buffer.is_empty() {
    set_handshake_timeout(stream, deadline)?;
    let bytes_read = stream.read(buffer).map_err(|_| error_message.to_string())?;
    if bytes_read == 0 {
      return Err(error_message.to_string());
    }
    buffer = &mut buffer[bytes_read..];
  }
  Ok(())
}

#[cfg(windows)]
fn check_http_proxy_tunnel(
  stream: &mut TcpStream,
  target_host: &str,
  target_port: u16,
  deadline: Instant,
) -> Result<(), String> {
  set_handshake_timeout(stream, deadline)?;
  write!(
    stream,
    "CONNECT {target_host}:{target_port} HTTP/1.1\r\nHost: {target_host}:{target_port}\r\nProxy-Connection: Keep-Alive\r\n\r\n"
  )
  .map_err(|_| "无法向 HTTP 代理发送连接请求".to_string())?;

  let mut header = Vec::with_capacity(256);
  let mut chunk = [0u8; 1024];
  while header.len() < 16 * 1024 {
    set_handshake_timeout(stream, deadline)?;
    let read_len = (16 * 1024 - header.len()).min(chunk.len());
    let bytes_read = stream
      .read(&mut chunk[..read_len])
      .map_err(|_| "HTTP 代理未能建立目标站点连接".to_string())?;
    if bytes_read == 0 {
      return Err("HTTP 代理未能建立目标站点连接".to_string());
    }
    header.extend_from_slice(&chunk[..bytes_read]);
    if let Some(header_end) = header.windows(4).position(|window| window == b"\r\n\r\n") {
      header.truncate(header_end + 4);
      break;
    }
  }

  if !header.ends_with(b"\r\n\r\n") {
    return Err("HTTP 代理返回的响应头过大".to_string());
  }
  let response =
    std::str::from_utf8(&header).map_err(|_| "HTTP 代理返回了无效响应".to_string())?;
  let mut status_parts = response.lines().next().unwrap_or_default().split_whitespace();
  let version = status_parts.next();
  let status_code = status_parts.next();
  if matches!(version, Some("HTTP/1.0" | "HTTP/1.1")) && status_code == Some("200") {
    Ok(())
  } else if status_code == Some("407") {
    Err("HTTP 代理要求身份验证；当前版本不支持带认证的代理".to_string())
  } else {
    Err("HTTP 代理未能连接到目标站点".to_string())
  }
}

#[cfg(windows)]
fn check_socks5_proxy_tunnel(
  stream: &mut TcpStream,
  target_host: &str,
  target_port: u16,
  deadline: Instant,
) -> Result<(), String> {
  set_handshake_timeout(stream, deadline)?;
  stream
    .write_all(&[0x05, 0x01, 0x00])
    .map_err(|_| "无法向 SOCKS5 代理发送握手请求".to_string())?;
  let mut method = [0u8; 2];
  read_exact_before_deadline(
    stream,
    &mut method,
    deadline,
    "SOCKS5 代理未能完成握手",
  )?;
  if method[0] != 0x05 || method[1] != 0x00 {
    return Err("SOCKS5 代理要求身份验证；当前版本不支持带认证的代理".to_string());
  }

  let target_host = target_host.as_bytes();
  let host_len = u8::try_from(target_host.len())
    .map_err(|_| "目标站点域名过长，无法检查代理".to_string())?;
  let mut request = Vec::with_capacity(target_host.len() + 7);
  request.extend_from_slice(&[0x05, 0x01, 0x00, 0x03, host_len]);
  request.extend_from_slice(target_host);
  request.extend_from_slice(&target_port.to_be_bytes());
  set_handshake_timeout(stream, deadline)?;
  stream
    .write_all(&request)
    .map_err(|_| "无法向 SOCKS5 代理发送目标站点请求".to_string())?;

  let mut response = [0u8; 4];
  read_exact_before_deadline(
    stream,
    &mut response,
    deadline,
    "SOCKS5 代理未能连接到目标站点",
  )?;
  if response[0] != 0x05 || response[2] != 0x00 {
    return Err("SOCKS5 代理返回了无效响应".to_string());
  }
  if response[1] != 0x00 {
    return Err("SOCKS5 代理未能连接到目标站点".to_string());
  }

  let address_len = match response[3] {
    0x01 => 4,
    0x04 => 16,
    0x03 => {
      let mut length = [0u8; 1];
      read_exact_before_deadline(
        stream,
        &mut length,
        deadline,
        "SOCKS5 代理返回了无效响应",
      )?;
      length[0] as usize
    }
    _ => return Err("SOCKS5 代理返回了无效响应".to_string()),
  };
  let mut address_and_port = vec![0u8; address_len + 2];
  read_exact_before_deadline(
    stream,
    &mut address_and_port,
    deadline,
    "SOCKS5 代理返回了无效响应",
  )?;
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::parse_account_proxy;
  #[cfg(windows)]
  use url::Url;

  #[test]
  fn accepts_http_and_socks5_endpoints() {
    assert_eq!(
      parse_account_proxy("http://127.0.0.1:8080").unwrap().unwrap().as_str(),
      "http://127.0.0.1:8080/"
    );
    assert_eq!(
      parse_account_proxy("socks5://proxy.example:1080").unwrap().unwrap().as_str(),
      "socks5://proxy.example:1080/"
    );
  }

  #[test]
  fn empty_value_disables_account_proxy() {
    assert!(parse_account_proxy("  ").unwrap().is_none());
  }

  #[test]
  fn rejects_unsupported_schemes_and_incomplete_endpoints() {
    assert!(parse_account_proxy("https://proxy.example:443").is_err());
    assert!(parse_account_proxy("http://proxy.example").is_err());
  }

  #[test]
  fn rejects_credentials_and_proxy_paths() {
    assert!(parse_account_proxy("http://user:secret@proxy.example:8080").is_err());
    assert!(parse_account_proxy("http://proxy.example:8080/path").is_err());
  }

  #[cfg(windows)]
  #[test]
  fn rejects_failed_incomplete_and_oversized_http_tunnels() {
    use super::check_http_proxy_tunnel;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    let mut oversized_response = b"HTTP/1.1 200 OK\r\nX-Padding: ".to_vec();
    oversized_response.extend(vec![b'x'; 16 * 1024]);
    let responses = vec![
      b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n".to_vec(),
      b"HTTP/1.1 502 Bad Gateway\r\n\r\n".to_vec(),
      b"HTTP/1.1 200 Connection established\r\n".to_vec(),
      oversized_response,
    ];
    for response in responses {
      let listener = TcpListener::bind("127.0.0.1:0").unwrap();
      let address = listener.local_addr().unwrap();
      let proxy = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
          stream.read_exact(&mut byte).unwrap();
          request.push(byte[0]);
        }
        let _ = stream.write_all(&response);
      });
      let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
      assert!(check_http_proxy_tunnel(
        &mut stream,
        "muse.ai",
        443,
        Instant::now() + Duration::from_secs(2),
      )
      .is_err());
      proxy.join().unwrap();
    }
  }

  #[cfg(windows)]
  #[test]
  fn rejects_socks5_authentication_and_expired_handshake() {
    use super::check_socks5_proxy_tunnel;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
      let mut greeting = [0u8; 3];
      stream.read_exact(&mut greeting).unwrap();
      stream.write_all(&[0x05, 0xff]).unwrap();
    });
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    assert!(check_socks5_proxy_tunnel(
      &mut stream,
      "muse.ai",
      443,
      Instant::now() + Duration::from_secs(2),
    )
    .is_err());
    assert!(
      check_socks5_proxy_tunnel(&mut stream, "muse.ai", 443, Instant::now()).is_err()
    );
    proxy.join().unwrap();
  }

  #[cfg(windows)]
  #[test]
  fn checks_http_proxy_tunnel_before_opening_account() {
    use super::check_proxy_route_sync;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut request = Vec::new();
      let mut byte = [0u8; 1];
      while !request.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        request.push(byte[0]);
      }
      assert!(String::from_utf8(request)
        .unwrap()
        .starts_with("CONNECT muse.ai:443 "));
      stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
    });

    let proxy_url = Url::parse(&format!("http://{address}")).unwrap();
    let target_url = Url::parse("https://muse.ai/").unwrap();
    assert!(check_proxy_route_sync(&[address], &proxy_url, &target_url).is_ok());
    proxy.join().unwrap();
  }

  #[cfg(windows)]
  #[test]
  fn rejects_proxy_that_disconnects_during_http_tunnel() {
    use super::check_proxy_route_sync;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut request = Vec::new();
      let mut byte = [0u8; 1];
      while !request.ends_with(b"\r\n\r\n") {
        if stream.read_exact(&mut byte).is_err() {
          return;
        }
        request.push(byte[0]);
      }
      let _ = stream.write_all(b"HTTP/1.1 200 Connection established\r\n");
      drop(stream);
    });

    let proxy_url = Url::parse(&format!("http://{address}")).unwrap();
    let target_url = Url::parse("https://muse.ai/").unwrap();
    assert!(check_proxy_route_sync(&[address], &proxy_url, &target_url).is_err());
    proxy.join().unwrap();
  }

  #[cfg(windows)]
  #[test]
  fn checks_socks5_remote_dns_and_target_connect() {
    use super::check_proxy_route_sync;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = thread::spawn(move || {
      let (mut stream, _) = listener.accept().unwrap();
      let mut greeting = [0u8; 3];
      stream.read_exact(&mut greeting).unwrap();
      assert_eq!(greeting, [0x05, 0x01, 0x00]);
      stream.write_all(&[0x05, 0x00]).unwrap();

      let mut request_header = [0u8; 5];
      stream.read_exact(&mut request_header).unwrap();
      assert_eq!(&request_header, &[0x05, 0x01, 0x00, 0x03, 7]);
      let mut target = vec![0u8; request_header[4] as usize + 2];
      stream.read_exact(&mut target).unwrap();
      assert_eq!(&target[..7], b"muse.ai");
      assert_eq!(&target[7..], &[0x01, 0xbb]);
      stream.write_all(&[0x05, 0x00, 0x00, 0x01, 127, 0, 0, 1, 0, 0]).unwrap();
    });

    let proxy_url = Url::parse(&format!("socks5://{address}")).unwrap();
    let target_url = Url::parse("https://muse.ai/").unwrap();
    assert!(check_proxy_route_sync(&[address], &proxy_url, &target_url).is_ok());
    proxy.join().unwrap();
  }
}
