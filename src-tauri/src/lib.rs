#[cfg(any(windows, test))]
mod app_paths;
mod network;
mod platform;
mod providers;
mod proxy_gate_adapter;
mod proxy_pool;
mod session_manager;
mod storage;
mod updater;

use providers::ProviderId;
use proxy_gate_adapter::{
  ProxyGateAdapter, ProxyGateProbeSnapshot, ProxyGateSnapshot, ProxyGateStatus,
};
use proxy_pool::{ProxyPool, ProxyPoolBinding, ProxyPoolSnapshot};
use session_manager::{ManagerSnapshot, SessionManager, APP_TITLE};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use updater::{InstallUpdateOutcome, InstallUpdateRequest, UpdateCheckResult};

#[tauri::command]
async fn get_manager_state(mgr: State<'_, Arc<SessionManager>>) -> Result<ManagerSnapshot, String> {
  Ok(mgr.get_snapshot().await)
}

#[tauri::command]
async fn get_proxy_pool(pool: State<'_, ProxyPool>) -> Result<ProxyPoolSnapshot, String> {
  if !cfg!(windows) {
    return Err("本地代理池目前仅支持 Windows 桌面版".to_string());
  }
  Ok(pool.snapshot())
}

#[tauri::command]
async fn set_proxy_pool_source(
  pool: State<'_, ProxyPool>,
  source_path: String,
) -> Result<ProxyPoolSnapshot, String> {
  if !cfg!(windows) {
    return Err("本地代理池目前仅支持 Windows 桌面版".to_string());
  }
  pool.set_source(&source_path)
}

#[tauri::command]
async fn probe_proxy_pool(
  pool: State<'_, ProxyPool>,
  provider_id: String,
  force: Option<bool>,
) -> Result<proxy_pool::ProxyPoolHealthSnapshot, String> {
  if !cfg!(windows) {
    return Err("代理可用性检测目前仅支持 Windows 桌面版".to_string());
  }
  let provider_id =
    ProviderId::parse(&provider_id).ok_or_else(|| "不支持的账号服务类型".to_string())?;
  pool.probe_health(provider_id, force.unwrap_or(false)).await
}

#[tauri::command]
async fn get_proxy_gate_snapshot(
  adapter: State<'_, ProxyGateAdapter>,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  Ok(adapter.snapshot())
}

#[tauri::command]
async fn get_proxy_gate_status(
  adapter: State<'_, ProxyGateAdapter>,
) -> Result<Option<ProxyGateStatus>, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  adapter.status()
}

#[tauri::command]
async fn probe_proxy_gate_pool(
  adapter: State<'_, ProxyGateAdapter>,
) -> Result<ProxyGateProbeSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  Ok(adapter.probe_pool())
}

#[tauri::command]
async fn start_proxy_gate(
  adapter: State<'_, ProxyGateAdapter>,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  adapter.start()?;
  Ok(adapter.snapshot())
}

#[tauri::command]
async fn stop_proxy_gate(
  adapter: State<'_, ProxyGateAdapter>,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  adapter.stop()?;
  Ok(adapter.snapshot())
}

#[tauri::command]
async fn set_proxy_gate_pool_source(
  adapter: State<'_, ProxyGateAdapter>,
  source_path: String,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  adapter.set_pool_source(&source_path)?;
  Ok(adapter.snapshot())
}

#[tauri::command]
async fn assign_proxy_gate_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  adapter: State<'_, ProxyGateAdapter>,
  provider_id: String,
  uuid: String,
  account_alias: Option<String>,
  entry_id: String,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  #[cfg(windows)]
  {
    let assignment = adapter.assign(&provider_id, &uuid, &entry_id)?;
    if let Err(error) = adapter.start() {
      let _ = adapter.unassign(&provider_id, &uuid);
      return Err(format!("Proxy Gate 启动失败，已回滚绑定：{error}"));
    }
    if let Err(error) = mgr
      .set_account_proxy(&app, &provider_id, &uuid, &assignment.endpoint, None)
      .await
    {
      let _ = adapter.unassign(&provider_id, &uuid);
      return Err(format!("账号代理保存失败，已回滚 Proxy Gate 绑定：{error}"));
    }
    let mut snapshot = adapter.snapshot();
    if let Some(item) = snapshot
      .assignments
      .iter_mut()
      .find(|item| item.account_id == assignment.account_id)
    {
      item.account_alias = account_alias;
    }
    Ok(snapshot)
  }
  #[cfg(not(windows))]
  {
    let _ = (
      app,
      mgr,
      provider_id,
      uuid,
      account_alias,
      entry_id,
      adapter,
    );
    Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string())
  }
}

#[tauri::command]
async fn unassign_proxy_gate_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  adapter: State<'_, ProxyGateAdapter>,
  provider_id: String,
  uuid: String,
) -> Result<ProxyGateSnapshot, String> {
  if !cfg!(windows) {
    return Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string());
  }
  #[cfg(windows)]
  {
    adapter.unassign(&provider_id, &uuid)?;
    mgr
      .set_account_proxy(&app, &provider_id, &uuid, "", None)
      .await?;
    Ok(adapter.snapshot())
  }
  #[cfg(not(windows))]
  {
    let _ = (app, mgr, provider_id, uuid, adapter);
    Err("Proxy Gate 目前仅支持 Windows 桌面版".to_string())
  }
}

#[tauri::command]
async fn select_provider(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
) -> Result<ManagerSnapshot, String> {
  mgr.select_provider(&app, &provider_id).await
}

#[tauri::command]
async fn add_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  alias: String,
  proxy_url: Option<String>,
  open_after_save: Option<bool>,
  proxy_pool_binding: Option<ProxyPoolBinding>,
) -> Result<ManagerSnapshot, String> {
  mgr
    .add_account(
      &app,
      &provider_id,
      &alias,
      proxy_url.as_deref().unwrap_or(""),
      open_after_save.unwrap_or(true),
      proxy_pool_binding,
    )
    .await
}

#[tauri::command]
async fn rename_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  uuid: String,
  alias: String,
) -> Result<ManagerSnapshot, String> {
  mgr.rename_account(&app, &provider_id, &uuid, &alias).await
}

#[tauri::command]
async fn set_account_proxy(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  uuid: String,
  proxy_url: String,
  proxy_pool_binding: Option<ProxyPoolBinding>,
) -> Result<ManagerSnapshot, String> {
  mgr
    .set_account_proxy(&app, &provider_id, &uuid, &proxy_url, proxy_pool_binding)
    .await
}

#[tauri::command]
async fn copy_account_cookie(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  webview: tauri::Webview,
  provider_id: String,
  uuid: String,
) -> Result<(), String> {
  if webview.label() != "manager" {
    return Err("仅账号管理页可以复制 Cookie".to_string());
  }
  mgr.copy_account_cookie(&app, &provider_id, &uuid).await
}

#[tauri::command]
async fn remove_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  uuid: String,
) -> Result<ManagerSnapshot, String> {
  mgr.remove_account(&app, &provider_id, &uuid).await
}

#[tauri::command]
async fn switch_account(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  uuid: String,
) -> Result<ManagerSnapshot, String> {
  mgr.switch_account(&app, &provider_id, &uuid).await
}

#[tauri::command]
async fn set_modal_active(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  active: bool,
) -> Result<ManagerSnapshot, String> {
  mgr.set_modal_active(&app, active).await
}

#[tauri::command]
async fn open_official_site(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
) -> Result<ManagerSnapshot, String> {
  mgr.open_official_site(&app, &provider_id).await
}

#[tauri::command]
async fn toggle_muse_account_management(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
  provider_id: String,
  uuid: String,
) -> Result<ManagerSnapshot, String> {
  mgr
    .toggle_muse_account_management(&app, &provider_id, &uuid)
    .await
}

#[tauri::command]
async fn resolve_uuid_conflict(
  mgr: State<'_, Arc<SessionManager>>,
  keep_provider_id: String,
  uuid: String,
) -> Result<ManagerSnapshot, String> {
  mgr.resolve_uuid_conflict(&keep_provider_id, &uuid).await
}

#[tauri::command]
async fn dismiss_notice(mgr: State<'_, Arc<SessionManager>>) -> Result<ManagerSnapshot, String> {
  Ok(mgr.dismiss_notice().await)
}

#[tauri::command]
async fn trigger_quota_sync(
  app: AppHandle,
  mgr: State<'_, Arc<SessionManager>>,
) -> Result<ManagerSnapshot, String> {
  mgr.trigger_silent_sync(&app, true).await;
  Ok(mgr.get_snapshot().await)
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<UpdateCheckResult, String> {
  updater::check_for_updates(&app).await
}

#[tauri::command]
async fn download_and_install_update(
  app: AppHandle,
  request: InstallUpdateRequest,
) -> Result<InstallUpdateOutcome, String> {
  updater::download_and_install_update(&app, request).await
}

#[tauri::command]
async fn open_external_url(app: AppHandle, url: String) -> Result<(), String> {
  updater::open_external_url(&app, &url).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  let builder = tauri::Builder::default();

  #[cfg(target_os = "android")]
  let builder = builder.plugin(platform::android::init());

  builder
    .setup(|app| {
      let app_data_dir = app.path().app_data_dir()?;
      #[cfg(windows)]
      let app_data_dir =
        app_paths::resolve_data_dir(app_data_dir, std::env::var_os("DIEHAO_DATA_DIR"))?;
      #[cfg(windows)]
      std::fs::create_dir_all(&app_data_dir)?;
      app.manage(ProxyPool::new(app_data_dir.clone()));
      app.manage(ProxyGateAdapter::new(app_data_dir.clone()));
      let session_mgr = Arc::new(SessionManager::new(app_data_dir.clone()));
      app.manage(Arc::clone(&session_mgr));

      #[cfg(desktop)]
      {
        let main_window = tauri::window::WindowBuilder::new(app, "main")
          .title(APP_TITLE)
          .inner_size(1240.0, 800.0)
          .min_inner_size(760.0, 520.0)
          .build()?;

        let manager_builder = tauri::webview::WebviewBuilder::new(
          "manager",
          tauri::WebviewUrl::App("index.html".into()),
        )
        .on_navigation(|url| {
          url.scheme() == "tauri"
            || url.domain() == Some("tauri.localhost")
            || (cfg!(dev) && url.host_str() == Some("localhost"))
        });

        // Keep the local management WebView in the same selected data root as accounts.
        #[cfg(windows)]
        let manager_builder = manager_builder.data_directory(app_data_dir.join("manager-webview"));

        main_window.add_child(
          manager_builder,
          tauri::LogicalPosition::new(0.0, 0.0),
          tauri::LogicalSize::new(1240.0, 800.0),
        )?;

        let app_for_resize = app.handle().clone();
        let mgr_for_resize = Arc::clone(&session_mgr);
        main_window.on_window_event(move |event| match event {
          tauri::WindowEvent::Resized(_) | tauri::WindowEvent::ScaleFactorChanged { .. } => {
            let app_inner = app_for_resize.clone();
            let mgr_inner = Arc::clone(&mgr_for_resize);
            tauri::async_runtime::spawn(async move {
              mgr_inner.recompute_desktop_bounds(&app_inner).await;
            });
          }
          tauri::WindowEvent::Focused(focused) if *focused => {
            let app_inner = app_for_resize.clone();
            let mgr_inner = Arc::clone(&mgr_for_resize);
            tauri::async_runtime::spawn(async move {
              mgr_inner.trigger_silent_sync(&app_inner, false).await;
            });
          }
          _ => {}
        });
      }

      #[cfg(mobile)]
      {
        let _ = APP_TITLE;
        tauri::WebviewWindowBuilder::new(
          app,
          "manager",
          tauri::WebviewUrl::App("index.html".into()),
        )
        .build()?;
      }

      let app_for_boot = app.handle().clone();
      let mgr_for_boot = Arc::clone(&session_mgr);
      tauri::async_runtime::spawn(async move {
        mgr_for_boot.run_startup_tasks(&app_for_boot).await;
      });

      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      get_manager_state,
      get_proxy_pool,
      set_proxy_pool_source,
      probe_proxy_pool,
      get_proxy_gate_snapshot,
      get_proxy_gate_status,
      probe_proxy_gate_pool,
      start_proxy_gate,
      stop_proxy_gate,
      set_proxy_gate_pool_source,
      assign_proxy_gate_account,
      unassign_proxy_gate_account,
      select_provider,
      add_account,
      rename_account,
      set_account_proxy,
      copy_account_cookie,
      remove_account,
      switch_account,
      set_modal_active,
      open_official_site,
      toggle_muse_account_management,
      resolve_uuid_conflict,
      dismiss_notice,
      trigger_quota_sync,
      check_for_updates,
      download_and_install_update,
      open_external_url,
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}

#[cfg(test)]
mod capability_tests {
  #[test]
  fn proxy_pool_commands_are_allowed_for_manager_webview() {
    let capability = include_str!("../capabilities/manager.json");
    for permission in [
      "allow-get-proxy-pool",
      "allow-set-proxy-pool-source",
      "allow-probe-proxy-pool",
    ] {
      assert!(
        capability.contains(&format!("\"{permission}\"")),
        "manager capability must allow {permission}"
      );
    }
  }

  #[test]
  fn cookie_copy_command_is_allowed_for_manager_webview() {
    let capability = include_str!("../capabilities/manager.json");
    assert!(
      capability.contains("\"allow-copy-account-cookie\""),
      "manager capability must allow cookie copying"
    );
  }

  #[test]
  fn account_management_command_is_allowed_for_manager_webview() {
    let capability = include_str!("../capabilities/manager.json");
    assert!(
      capability.contains("\"allow-toggle-muse-account-management\""),
      "manager capability must allow Meta account management"
    );
  }
}
