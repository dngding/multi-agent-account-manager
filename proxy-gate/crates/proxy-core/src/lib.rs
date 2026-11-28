mod health;
mod pool;

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub use health::{open_tunnel, probe_entries, EntryHealth, HealthStatus};
pub use pool::{discover_sources, load_pool, parse_pool_bytes, PoolEntry, PoolSnapshot};

pub const MUSE_TARGET_URL: &str = "https://muse.ai/";
pub const HARK_TARGET_URL: &str = "https://app.hark.com/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Muse,
    Hark,
}

impl Provider {
    pub fn target_url(self) -> &'static str {
        match self {
            Self::Muse => MUSE_TARGET_URL,
            Self::Hark => HARK_TARGET_URL,
        }
    }
}

impl FromStr for Provider {
    type Err = CoreError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "muse" => Ok(Self::Muse),
            "hark" => Ok(Self::Hark),
            _ => Err(CoreError::InvalidPool("不支持的账号服务类型".to_string())),
        }
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("{0}")]
    InvalidPool(String),
    #[error("{0}")]
    Io(String),
    #[error("{0}")]
    Network(String),
}

impl From<std::io::Error> for CoreError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
