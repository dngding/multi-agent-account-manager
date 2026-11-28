use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PORT_START: u16 = 19100;
pub const DEFAULT_PORT_END: u16 = 19199;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_path: Option<PathBuf>,
    #[serde(default = "default_listen_host")]
    pub listen_host: String,
    #[serde(default = "default_port_start")]
    pub port_start: u16,
    #[serde(default = "default_port_end")]
    pub port_end: u16,
    #[serde(default = "default_health_target")]
    pub health_target: String,
    #[serde(default = "default_health_interval_seconds")]
    pub health_interval_seconds: u64,
    #[serde(default = "default_allow_handshake_only")]
    pub allow_handshake_only: bool,
    #[serde(default)]
    pub assignments: Vec<Assignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Assignment {
    pub account_id: String,
    pub entry_id: String,
    pub fingerprint: String,
    pub label: String,
    pub port: u16,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            pool_path: None,
            listen_host: default_listen_host(),
            port_start: DEFAULT_PORT_START,
            port_end: DEFAULT_PORT_END,
            health_target: default_health_target(),
            health_interval_seconds: default_health_interval_seconds(),
            allow_handshake_only: default_allow_handshake_only(),
            assignments: Vec::new(),
        }
    }
}

impl GateConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let bytes =
            fs::read(path).with_context(|| format!("无法读取配置文件 {}", path.display()))?;
        let config: Self = serde_json::from_slice(&bytes)
            .with_context(|| format!("配置文件格式无效 {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            return Self::load(path);
        }
        let config = Self::default();
        config.save(path)?;
        Ok(config)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("无法创建配置目录 {}", parent.display()))?;
        }
        let payload = serde_json::to_vec_pretty(self)?;
        fs::write(path, payload).with_context(|| format!("无法写入配置文件 {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("不支持的配置版本 {}，当前仅支持 1", self.version);
        }
        if self.listen_host != "127.0.0.1" && self.listen_host != "::1" {
            bail!("listenHost 只允许 127.0.0.1 或 ::1");
        }
        if self.port_start > self.port_end {
            bail!("portStart 不能大于 portEnd");
        }
        if self.health_interval_seconds == 0 {
            bail!("healthIntervalSeconds 必须大于 0");
        }
        url::Url::parse(&self.health_target).context("healthTarget 不是有效 URL")?;

        let mut accounts = std::collections::HashSet::new();
        let mut ports = std::collections::HashSet::new();
        for assignment in &self.assignments {
            if assignment.account_id.trim().is_empty() {
                bail!("accountId 不能为空");
            }
            if assignment.entry_id.trim().is_empty() {
                bail!("entryId 不能为空");
            }
            if assignment.fingerprint.trim().is_empty() {
                bail!("fingerprint 不能为空");
            }
            if assignment.port < self.port_start || assignment.port > self.port_end {
                bail!(
                    "账号 {} 的端口 {} 不在 {}..={} 范围内",
                    assignment.account_id,
                    assignment.port,
                    self.port_start,
                    self.port_end
                );
            }
            if !accounts.insert(assignment.account_id.clone()) {
                bail!("账号 {} 重复绑定", assignment.account_id);
            }
            if !ports.insert(assignment.port) {
                bail!("端口 {} 被多个账号重复使用", assignment.port);
            }
        }
        Ok(())
    }

    pub fn allocate_port(&self) -> Result<u16> {
        let used = self
            .assignments
            .iter()
            .map(|assignment| assignment.port)
            .collect::<std::collections::HashSet<_>>();
        (self.port_start..=self.port_end)
            .find(|port| !used.contains(port))
            .context("代理网关端口范围已用尽")
    }

    pub fn resolve_pool_path(&mut self) -> Result<PathBuf> {
        if let Some(path) = &self.pool_path {
            return Ok(path.clone());
        }
        let discovered = maam_proxy_core::discover_sources()?;
        match discovered.as_slice() {
            [] => bail!("未发现 sidecar.yaml，请使用 --pool 指定绝对路径，或设置配置中的 poolPath"),
            [path] => {
                self.pool_path = Some(path.clone());
                Ok(path.clone())
            }
            _ => bail!(
                "发现多个 sidecar.yaml，请显式选择：{}",
                discovered
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
        }
    }
}

fn default_version() -> u32 {
    1
}

fn default_listen_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port_start() -> u16 {
    DEFAULT_PORT_START
}

fn default_port_end() -> u16 {
    DEFAULT_PORT_END
}

fn default_health_target() -> String {
    "https://muse.ai/".to_string()
}

fn default_health_interval_seconds() -> u64 {
    60
}

fn default_allow_handshake_only() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::{Assignment, GateConfig};

    fn assignment(account: &str, port: u16) -> Assignment {
        Assignment {
            account_id: account.to_string(),
            entry_id: "entry".to_string(),
            fingerprint: "fingerprint".to_string(),
            label: "entry".to_string(),
            port,
        }
    }

    #[test]
    fn allocates_first_unused_port() {
        let config = GateConfig {
            assignments: vec![assignment("a", 19100), assignment("b", 19102)],
            ..GateConfig::default()
        };
        assert_eq!(config.allocate_port().unwrap(), 19101);
    }

    #[test]
    fn rejects_duplicate_accounts_and_ports() {
        let mut config = GateConfig {
            assignments: vec![assignment("a", 19100), assignment("a", 19101)],
            ..GateConfig::default()
        };
        assert!(config.validate().is_err());

        config.assignments = vec![assignment("a", 19100), assignment("b", 19100)];
        assert!(config.validate().is_err());
    }
}
