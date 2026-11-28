#![allow(dead_code)]

#[path = "../../../src-tauri/src/proxy_gate_adapter.rs"]
mod proxy_gate_adapter;

#[cfg(test)]
mod tests {
  use std::fs;
  use std::os::unix::fs::PermissionsExt;
  use std::path::PathBuf;
  use std::time::{SystemTime, UNIX_EPOCH};

  use crate::proxy_gate_adapter::ProxyGateAdapter;

  #[test]
  fn adapter_executes_inspect_assign_and_unassign_contract() {
    let nonce = SystemTime::now()
      .duration_since(UNIX_EPOCH)
      .unwrap()
      .as_nanos();
    let root = std::env::temp_dir().join(format!("proxy-gate-adapter-{nonce}"));
    fs::create_dir_all(&root).unwrap();

    let exe = root.join("proxy-gate.exe");
    let config = root.join("proxy-gate.json");
    let pool = root.join("sidecar.yaml");
    fs::write(&pool, "proxies: []\nlisteners: []\n").unwrap();
    fs::write(
      &config,
      serde_json::to_vec_pretty(&serde_json::json!({
          "version": 1,
          "poolPath": pool,
          "listenHost": "127.0.0.1",
          "portStart": 19100,
          "portEnd": 19199,
          "healthTarget": "https://muse.ai/",
          "healthIntervalSeconds": 60,
          "allowHandshakeOnly": true,
          "assignments": []
      }))
      .unwrap(),
    )
    .unwrap();
    fs::write(
      &exe,
      r#"#!/usr/bin/env python3
import json
import sys

args = sys.argv[1:]
config_path = args[args.index("--config") + 1]
command = args[args.index("--config") + 2]
with open(config_path, "r", encoding="utf-8") as handle:
    config = json.load(handle)

if command == "inspect":
    print(json.dumps({
        "sourcePath": config["poolPath"],
        "revision": "revision-1",
        "entries": [{
            "id": "socks5:7891",
            "label": "fixed · 7891",
            "proxyUrl": "socks5://127.0.0.1:7891",
            "fingerprint": "fingerprint-1",
            "duplicateCount": 1
        }]
    }))
elif command == "assign":
    account_id, entry_id = args[-2:]
    assignment = next(
        (item for item in config["assignments"] if item["accountId"] == account_id),
        None,
    )
    port = assignment["port"] if assignment else 19100
    config["assignments"] = [
        item for item in config["assignments"] if item["accountId"] != account_id
    ]
    config["assignments"].append({
        "accountId": account_id,
        "entryId": entry_id,
        "fingerprint": "fingerprint-1",
        "label": "fixed · 7891",
        "port": port
    })
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump(config, handle)
elif command == "unassign":
    account_id = args[-1]
    config["assignments"] = [
        item for item in config["assignments"] if item["accountId"] != account_id
    ]
    with open(config_path, "w", encoding="utf-8") as handle:
        json.dump(config, handle)
else:
    raise SystemExit(2)
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&exe).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&exe, permissions).unwrap();

    std::env::set_var("PROXY_GATE_EXE", &exe);
    std::env::set_var("PROXY_GATE_CONFIG", &config);
    let adapter = ProxyGateAdapter::new(PathBuf::from(&root));

    let assigned = adapter
      .assign("muse", "account-uuid", "socks5:7891")
      .unwrap();
    assert_eq!(assigned.account_id, "muse:account-uuid");
    assert_eq!(assigned.port, 19100);
    assert_eq!(assigned.endpoint, "http://127.0.0.1:19100");
    assert!(assigned.binding_valid);

    let snapshot = adapter.snapshot();
    assert!(snapshot.available);
    assert_eq!(snapshot.entries.len(), 1);
    assert_eq!(snapshot.assignments.len(), 1);

    adapter.unassign("muse", "account-uuid").unwrap();
    assert!(adapter.snapshot().assignments.is_empty());

    std::env::remove_var("PROXY_GATE_EXE");
    std::env::remove_var("PROXY_GATE_CONFIG");
    fs::remove_dir_all(root).unwrap();
  }
}
