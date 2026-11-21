use url::Url;

pub fn parse_account_proxy(raw: &str) -> Result<Option<Url>, String> {
  let value = raw.trim();
  if value.is_empty() {
    return Ok(None);
  }

  let url = Url::parse(value).map_err(|_| "代理地址格式错误".to_string())?;
  if !matches!(url.scheme(), "http" | "socks5") {
    return Err("代理协议只支持 http:// 或 socks5://".to_string());
  }
  if url.host_str().is_none() || url.port().is_none() {
    return Err("代理地址必须包含主机名和端口".to_string());
  }
  if !url.username().is_empty() || url.password().is_some() {
    return Err("为避免明文保存凭据，暂不支持在代理地址中填写用户名或密码".to_string());
  }
  if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
    return Err("代理地址只能填写协议、主机和端口".to_string());
  }

  Ok(Some(url))
}

#[cfg(test)]
mod tests {
  use super::parse_account_proxy;

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
}
