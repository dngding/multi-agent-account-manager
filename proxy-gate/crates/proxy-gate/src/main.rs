#[cfg(windows)]
mod autostart;
mod config;
mod gateway;
#[cfg(windows)]
mod tray;

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use config::{Assignment, GateConfig};
use gateway::Gateway;
use maam_proxy_core::{discover_sources, load_pool, probe_entries, EntryHealth, HealthStatus};

#[derive(Debug, Parser)]
#[command(name = "proxy-gate", version, about = "Independent proxy pool gate")]
struct Cli {
    #[arg(long, default_value = "proxy-gate.json", global = true)]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init {
        #[arg(long)]
        pool: Option<PathBuf>,
    },
    Discover,
    Inspect {
        #[arg(long)]
        pool: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    List {
        #[arg(long)]
        pool: Option<PathBuf>,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = 16)]
        concurrency: usize,
    },
    Assign {
        account_id: String,
        entry_id: String,
        #[arg(long)]
        pool: Option<PathBuf>,
    },
    Unassign {
        account_id: String,
    },
    Run,
    Tray,
    InstallAutostart,
    UninstallAutostart,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { pool } => init(cli.config, pool),
        Command::Discover => discover(),
        Command::Inspect { pool, json } => inspect(cli.config, pool, json),
        Command::List {
            pool,
            json,
            concurrency,
        } => list(cli.config, pool, json, concurrency).await,
        Command::Assign {
            account_id,
            entry_id,
            pool,
        } => assign(cli.config, pool, account_id, entry_id),
        Command::Unassign { account_id } => unassign(cli.config, account_id),
        Command::Run => run(cli.config).await,
        Command::Tray => run_tray(cli.config).await,
        Command::InstallAutostart => install_autostart(cli.config),
        Command::UninstallAutostart => uninstall_autostart(),
    }
}

fn init(config_path: PathBuf, pool: Option<PathBuf>) -> Result<()> {
    if config_path.exists() {
        bail!("配置文件已存在：{}", config_path.display());
    }
    let config = GateConfig {
        pool_path: match pool {
            Some(path) => Some(path),
            None => {
                let candidates = discover_sources()?;
                if candidates.len() == 1 {
                    Some(candidates[0].clone())
                } else {
                    None
                }
            }
        },
        ..GateConfig::default()
    };
    config.save(&config_path)?;
    println!("已创建 {}", config_path.display());
    if let Some(path) = config.pool_path {
        println!("代理池 {}", path.display());
    } else {
        println!("尚未确定代理池，请运行 discover 或增加 --pool");
    }
    Ok(())
}

fn discover() -> Result<()> {
    let paths = discover_sources()?;
    if paths.is_empty() {
        println!("未发现 sidecar.yaml");
    } else {
        for path in paths {
            println!("{}", path.display());
        }
    }
    Ok(())
}

fn inspect(config_path: PathBuf, pool_override: Option<PathBuf>, json: bool) -> Result<()> {
    let mut config = GateConfig::load_or_create(&config_path)?;
    if let Some(path) = pool_override {
        config.pool_path = Some(path);
        config.save(&config_path)?;
    }
    let pool_path = config.resolve_pool_path()?;
    let pool = load_pool(&pool_path)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&pool)?);
    } else {
        println!("source\t{}", pool.source_path.display());
        println!("revision\t{}", pool.revision);
        println!("entry_id\tproxy\tduplicate_count\tlabel");
        for entry in pool.entries {
            println!(
                "{}\t{}\t{}\t{}",
                entry.id, entry.proxy_url, entry.duplicate_count, entry.label
            );
        }
    }
    Ok(())
}

async fn list(
    config_path: PathBuf,
    pool_override: Option<PathBuf>,
    json: bool,
    concurrency: usize,
) -> Result<()> {
    if concurrency == 0 {
        bail!("--concurrency 必须大于 0");
    }
    let mut config = GateConfig::load_or_create(&config_path)?;
    if let Some(path) = pool_override {
        config.pool_path = Some(path);
        config.save(&config_path)?;
    }
    let pool_path = config.resolve_pool_path()?;
    let pool = load_pool(&pool_path)?;
    let target = url::Url::parse(&config.health_target).context("healthTarget 无效")?;
    let mut health = probe_entries(&pool.entries, &target, concurrency).await?;
    health.sort_by(health_sort);

    if json {
        let output = serde_json::json!({
            "sourcePath": pool.source_path,
            "revision": pool.revision,
            "entries": pool.entries,
            "health": health,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    println!("source\t{}", pool.source_path.display());
    println!("revision\t{}", pool.revision);
    println!("status\tlatency_ms\tentry_id\tproxy\tlabel");
    for item in &health {
        println!(
            "{}\t{}\t{}\t{}\t{}",
            health_label(&item.status),
            item.latency_ms,
            item.entry_id,
            pool.entries
                .iter()
                .find(|entry| entry.id == item.entry_id)
                .map(|entry| entry.proxy_url.as_str())
                .unwrap_or("-"),
            pool.entries
                .iter()
                .find(|entry| entry.id == item.entry_id)
                .map(|entry| entry.label.as_str())
                .unwrap_or("-")
        );
    }
    Ok(())
}

fn assign(
    config_path: PathBuf,
    pool_override: Option<PathBuf>,
    account_id: String,
    entry_id: String,
) -> Result<()> {
    let mut config = GateConfig::load_or_create(&config_path)?;
    if let Some(path) = pool_override {
        config.pool_path = Some(path);
    }
    let pool_path = config.resolve_pool_path()?;
    let pool = load_pool(&pool_path)?;
    let entry = pool
        .entries
        .iter()
        .find(|entry| entry.id == entry_id)
        .with_context(|| format!("代理池中不存在入口 {entry_id}"))?;

    let port = match config
        .assignments
        .iter()
        .find(|assignment| assignment.account_id == account_id)
    {
        Some(assignment) => assignment.port,
        None => config.allocate_port()?,
    };
    let replacement = Assignment {
        account_id: account_id.clone(),
        entry_id: entry.id.clone(),
        fingerprint: entry.fingerprint.clone(),
        label: entry.label.clone(),
        port,
    };
    if let Some(existing) = config
        .assignments
        .iter_mut()
        .find(|assignment| assignment.account_id == account_id)
    {
        *existing = replacement;
    } else {
        config.assignments.push(replacement);
    }
    config.assignments.sort_by(|left, right| {
        left.account_id
            .cmp(&right.account_id)
            .then(left.port.cmp(&right.port))
    });
    config.save(&config_path)?;
    println!(
        "{account_id}\thttp://{}:{}\t{}",
        config.listen_host, port, entry.label
    );
    Ok(())
}

fn unassign(config_path: PathBuf, account_id: String) -> Result<()> {
    let mut config = GateConfig::load(&config_path)?;
    let previous = config.assignments.len();
    config
        .assignments
        .retain(|assignment| assignment.account_id != account_id);
    if previous == config.assignments.len() {
        bail!("账号 {account_id} 没有绑定");
    }
    config.save(&config_path)?;
    println!("已解除 {account_id}");
    Ok(())
}

async fn run(config_path: PathBuf) -> Result<()> {
    let gateway = Gateway::start(config_path).await?;
    println!("Proxy Gate 已启动，按 Ctrl+C 停止");
    gateway.wait_forever().await
}

#[cfg(windows)]
async fn run_tray(config_path: PathBuf) -> Result<()> {
    let gateway = Gateway::start(config_path.clone()).await?;
    let config_path = config_path
        .canonicalize()
        .with_context(|| format!("无法解析配置文件路径 {}", config_path.display()))?;
    tokio::task::spawn_blocking(move || tray::run(&config_path))
        .await
        .context("托盘线程异常退出")??;
    drop(gateway);
    Ok(())
}

#[cfg(not(windows))]
async fn run_tray(_config_path: PathBuf) -> Result<()> {
    bail!("tray 子命令仅支持 Windows")
}

#[cfg(windows)]
fn install_autostart(config_path: PathBuf) -> Result<()> {
    autostart::install(&config_path)
}

#[cfg(not(windows))]
fn install_autostart(_config_path: PathBuf) -> Result<()> {
    bail!("install-autostart 子命令仅支持 Windows")
}

#[cfg(windows)]
fn uninstall_autostart() -> Result<()> {
    autostart::uninstall()
}

#[cfg(not(windows))]
fn uninstall_autostart() -> Result<()> {
    bail!("uninstall-autostart 子命令仅支持 Windows")
}

fn health_sort(left: &EntryHealth, right: &EntryHealth) -> std::cmp::Ordering {
    health_rank(&left.status)
        .cmp(&health_rank(&right.status))
        .then(left.latency_ms.cmp(&right.latency_ms))
        .then(left.entry_id.cmp(&right.entry_id))
}

fn health_rank(status: &HealthStatus) -> u8 {
    match status {
        HealthStatus::Available => 0,
        HealthStatus::HandshakeOnly => 1,
        HealthStatus::Unavailable => 2,
    }
}

fn health_label(status: &HealthStatus) -> &'static str {
    match status {
        HealthStatus::Available => "available",
        HealthStatus::HandshakeOnly => "handshake_only",
        HealthStatus::Unavailable => "unavailable",
    }
}
