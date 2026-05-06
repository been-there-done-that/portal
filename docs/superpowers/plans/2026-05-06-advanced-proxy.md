# Advanced Proxy Features Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add HTTP/2 + RFC 8441 WebSocket, Tailscale sharing, and multiplexed hostname routing with in-browser app-switcher to portal v0.3.0.

**Architecture:** Four commit groups: (1) Route struct + StateStore foundation, (2) HTTP/2 auto-negotiation + RFC 8441, (3) Tailscale CLI integration, (4) multiplexed routing + HTML switcher injection. Groups 2-4 all build on Group 1's data model. hyper-util's `server-auto` feature (already in Cargo.toml) provides the H1+H2 auto-builder; no new dependencies needed for Groups 1-2. Tailscale uses `std::process::Command` only. Group 4 adds `src/switcher.rs` with an inline HTML constant.

**Tech Stack:** Rust, Tokio, Hyper 1.x + hyper-util (server-auto), rustls 0.23, tokio-rustls 0.26, clap 4 derive, nix (Unix signals), std::process::Command (Tailscale), serde_json

---

## Task 1: Extend Route struct with slot, label, and Tailscale fields

- [ ] **Modify `src/routes.rs`**

  Open `src/routes.rs`. The current `Route` struct ends at line 31. Add five new fields immediately after `created_at`, all annotated `#[serde(default)]`:

  ```rust
  #[derive(Debug, Clone, Serialize, Deserialize)]
  pub struct Route {
      pub hostname: String,
      pub port: u16,
      #[serde(default)]
      pub public_port: Option<u16>,
      #[serde(default)]
      pub protocol: RouteProtocol,
      pub pid: u32,
      #[serde(default)]
      pub owner_pid: u32,
      #[serde(default)]
      pub cwd: String,
      #[serde(default = "chrono::Utc::now")]
      pub created_at: DateTime<Utc>,

      // Multiplexed routing
      #[serde(default)]
      pub slot: u32,
      #[serde(default)]
      pub label: Option<String>,

      // Tailscale
      #[serde(default)]
      pub tailscale_url: Option<String>,
      #[serde(default)]
      pub tailscale_https_port: Option<u16>,
      #[serde(default)]
      pub tailscale_funnel: bool,
  }
  ```

- [ ] **Update all `Route { .. }` struct literals in `src/routes.rs` tests**

  In the `#[cfg(test)]` block (lines 199–437) every `Route { .. }` literal must include the new fields. Add these five fields to every constructor:

  ```rust
  slot: 0,
  label: None,
  tailscale_url: None,
  tailscale_https_port: None,
  tailscale_funnel: false,
  ```

  Affected test functions: `insert_and_get`, `persists_across_reload`, `remove_works`, `stale_cleanup_removes_dead_pids`, `stale_cleanup_returns_removed_tcp_routes`, `concurrent_inserts_no_data_loss`, `alias_route_survives_remove_stale`.

- [ ] **Add new test `new_route_fields_default_to_zero_and_none`**

  Add this test inside the existing `mod tests` block in `src/routes.rs`:

  ```rust
  #[tokio::test]
  async fn new_route_fields_default_to_zero_and_none() {
      let temp = TempDir::new().unwrap();
      let store_path = temp.path().join("routes.json");
      // Write old-format JSON without the new fields
      std::fs::write(
          &store_path,
          r#"[{"hostname":"legacy2.localhost","port":4000,"pid":1,"owner_pid":1,"cwd":"/tmp","created_at":"2026-04-09T00:00:00Z"}]"#,
      )
      .unwrap();

      let store = StateStore::new(store_path).unwrap();
      let route = store.get("legacy2.localhost").unwrap();
      assert_eq!(route.slot, 0);
      assert_eq!(route.label, None);
      assert_eq!(route.tailscale_url, None);
      assert_eq!(route.tailscale_https_port, None);
      assert!(!route.tailscale_funnel);
  }
  ```

- [ ] **Update all `Route { .. }` literals in other files**

  Search for all `Route {` struct literals in `src/daemon/ipc.rs` and update them to include the five new fields (set to defaults). This prevents compile errors when we use the struct elsewhere.

  In `src/daemon/ipc.rs` the `Route { .. }` literal is in the `RegisterRoute` handler (around line 321):
  ```rust
  let route = crate::routes::Route {
      hostname: hostname.clone(),
      port,
      public_port,
      protocol,
      pid,
      owner_pid: pid,
      cwd,
      created_at: chrono::Utc::now(),
      slot: 0,
      label: None,
      tailscale_url: None,
      tailscale_https_port: None,
      tailscale_funnel: false,
  };
  ```

  Also update any `Route { .. }` literals in `src/daemon/ipc.rs` test blocks (around lines 452–635): `ls_removes_stale_tcp_routes_and_releases_public_port`, `user_hostnames_excludes_inspector`, `display_target_uses_non_default_https_port_for_http_routes`, `display_target_uses_public_port_for_tcp_routes`, `prune_removes_dead_routes_keeps_aliases`.

- [ ] **Verify the existing backward-compat test still passes**

  ```bash
  cargo test old_http_routes_deserialize_without_protocol_fields
  ```

- [ ] **Run new test**

  ```bash
  cargo test new_route_fields_default_to_zero_and_none
  ```

- [ ] **Commit**

  ```bash
  git add src/routes.rs src/daemon/ipc.rs
  git commit -m "feat: extend Route with slot, label, and tailscale fields"
  ```

---

## Task 2: Change StateStore to support multiple routes per hostname (slots)

- [ ] **Modify `src/routes.rs` — change the map type**

  Change the `StateStore` struct field from:
  ```rust
  map: Arc<DashMap<String, Route>>,
  ```
  to:
  ```rust
  map: Arc<DashMap<String, Vec<Route>>>,
  ```

- [ ] **Update `StateStore::new` loading logic**

  Replace the single-route insert with a vec-based insert:

  ```rust
  pub fn new(path: PathBuf) -> Result<Self> {
      let map: Arc<DashMap<String, Vec<Route>>> = Arc::new(DashMap::new());
      if path.exists() {
          let contents = std::fs::read_to_string(&path)?;
          if !contents.is_empty() {
              let routes: Vec<Route> = serde_json::from_str(&contents)?;
              for route in routes {
                  map.entry(route.hostname.clone())
                      .or_insert_with(Vec::new)
                      .push(route);
              }
          }
      }
      Ok(Self {
          map,
          write_lock: Arc::new(tokio::sync::Mutex::new(())),
          path,
      })
  }
  ```

- [ ] **Update `get(hostname) -> Option<Route>`**

  Returns the first element (slot 0 primary) of the vec:

  ```rust
  pub fn get(&self, hostname: &str) -> Option<Route> {
      self.map.get(hostname).and_then(|v| v.first().cloned())
  }
  ```

- [ ] **Add `get_slot(hostname: &str, slot: u32) -> Option<Route>`**

  ```rust
  pub fn get_slot(&self, hostname: &str, slot: u32) -> Option<Route> {
      self.map.get(hostname).and_then(|v| {
          v.iter().find(|r| r.slot == slot).cloned()
      })
  }
  ```

- [ ] **Add `list_slots(hostname: &str) -> Vec<Route>`**

  ```rust
  pub fn list_slots(&self, hostname: &str) -> Vec<Route> {
      self.map.get(hostname)
          .map(|v| v.clone())
          .unwrap_or_default()
  }
  ```

- [ ] **Update `insert(route: Route) -> Result<()>`**

  Auto-assign slot when `route.slot == 0` and a vec already exists for this hostname. If a slot with the same number already exists, overwrite it. Otherwise push. Sorted invariant: vec is sorted by slot ascending (use a simple insertion sort after push).

  ```rust
  pub async fn insert(&self, route: Route) -> Result<()> {
      let _guard = self.write_lock.lock().await;
      let mut route = route;
      {
          let mut entry = self.map.entry(route.hostname.clone()).or_insert_with(Vec::new);
          let vec = entry.value_mut();
          if vec.is_empty() {
              // First slot — keep slot as-is (default 0)
          } else if route.slot == 0 {
              // Auto-assign: next slot = max existing slot + 1
              let max_slot = vec.iter().map(|r| r.slot).max().unwrap_or(0);
              route.slot = max_slot + 1;
          }
          // Overwrite if slot already exists, otherwise push
          if let Some(pos) = vec.iter().position(|r| r.slot == route.slot) {
              vec[pos] = route;
          } else {
              vec.push(route);
              vec.sort_by_key(|r| r.slot);
          }
      }
      self.persist_locked()?;
      self.sync_hosts_locked();
      Ok(())
  }
  ```

- [ ] **Update `remove(hostname: &str) -> Result<()>`**

  Removes the entire vec (all slots for hostname) — existing behaviour unchanged:

  ```rust
  pub async fn remove(&self, hostname: &str) -> Result<()> {
      let _guard = self.write_lock.lock().await;
      self.map.remove(hostname);
      self.persist_locked()?;
      self.sync_hosts_locked();
      Ok(())
  }
  ```

- [ ] **Add `remove_slot(hostname: &str, slot: u32) -> Result<()>`**

  Removes a single slot. If the vec becomes empty after removal, removes the hostname key entirely.

  ```rust
  pub async fn remove_slot(&self, hostname: &str, slot: u32) -> Result<()> {
      let _guard = self.write_lock.lock().await;
      let mut remove_key = false;
      if let Some(mut entry) = self.map.get_mut(hostname) {
          let vec = entry.value_mut();
          vec.retain(|r| r.slot != slot);
          if vec.is_empty() {
              remove_key = true;
          }
      }
      if remove_key {
          self.map.remove(hostname);
      }
      self.persist_locked()?;
      self.sync_hosts_locked();
      Ok(())
  }
  ```

- [ ] **Update `list() -> Vec<Route>`**

  Flattens all vecs:

  ```rust
  pub fn list(&self) -> Vec<Route> {
      self.map.iter().flat_map(|e| e.value().clone()).collect()
  }
  ```

- [ ] **Update `remove_stale() -> Result<Vec<Route>>`**

  Filter per-slot across all hostnames. Collect dead routes, remove them per-slot, remove hostname key if vec becomes empty:

  ```rust
  pub async fn remove_stale(&self) -> Result<Vec<Route>> {
      let _guard = self.write_lock.lock().await;

      // Collect all dead (hostname, slot) pairs and dead route values
      let dead: Vec<Route> = self
          .map
          .iter()
          .flat_map(|e| e.value().clone())
          .filter(|r| !pid_alive_check(r.pid))
          .collect();

      if dead.is_empty() {
          return Ok(Vec::new());
      }

      for route in &dead {
          if let Some(mut entry) = self.map.get_mut(&route.hostname) {
              entry.value_mut().retain(|r| r.slot != route.slot);
          }
      }
      // Remove empty hostname keys
      self.map.retain(|_, v| !v.is_empty());

      self.persist_locked()?;
      self.sync_hosts_locked();
      Ok(dead)
  }
  ```

- [ ] **Update `persist_locked()`**

  Flatten all vecs to a `Vec<Route>` before JSON serialisation:

  ```rust
  fn persist_locked(&self) -> Result<()> {
      let routes: Vec<Route> = self.map.iter()
          .flat_map(|e| e.value().clone())
          .collect();
      let json = serde_json::to_string_pretty(&routes)?;
      let tmp_path = format!("{}.tmp", self.path.display());
      std::fs::write(&tmp_path, &json)?;
      std::fs::rename(&tmp_path, &self.path)?;

      #[cfg(unix)]
      if let Some((uid, gid)) = crate::config::sudo_uid_gid() {
          unsafe {
              let p = std::ffi::CString::new(self.path.to_string_lossy().as_bytes()).unwrap();
              let ret = nix::libc::chown(p.as_ptr(), uid, gid);
              if ret != 0 {
                  tracing::warn!(
                      "chown failed for {}: {}",
                      self.path.display(),
                      std::io::Error::last_os_error()
                  );
              }
          }
      }
      Ok(())
  }
  ```

- [ ] **Update `sync_hosts_locked()`**

  Deduplicate hostnames since multiple slots may share a hostname:

  ```rust
  fn sync_hosts_locked(&self) {
      if !crate::hosts::should_sync() {
          return;
      }
      let mut seen = std::collections::HashSet::new();
      let hostnames: Vec<String> = self
          .map
          .iter()
          .filter_map(|e| {
              let key = e.key().clone();
              // Only include if at least one slot is Http and not the inspector route
              let has_http = e.value().iter().any(|r| {
                  r.hostname != "_.localhost" && r.protocol == RouteProtocol::Http
              });
              if has_http && seen.insert(key.clone()) {
                  Some(key)
              } else {
                  None
              }
          })
          .collect();
      let refs: Vec<&str> = hostnames.iter().map(|s| s.as_str()).collect();
      if let Err(e) = crate::hosts::sync_hosts_file(&refs) {
          tracing::warn!("hosts sync failed: {e}");
      }
  }
  ```

- [ ] **Update all existing tests in `src/routes.rs` to fix struct literal fields**

  All existing test `Route { .. }` literals already have the new fields from Task 1. Confirm the existing tests still compile and pass:

  ```bash
  cargo test --lib routes
  ```

- [ ] **Add new slot-related tests** in the `mod tests` block of `src/routes.rs`:

  ```rust
  #[tokio::test]
  async fn multi_slot_insert_auto_assigns_slot() {
      let temp = TempDir::new().unwrap();
      let store = StateStore::new(temp.path().join("routes.json")).unwrap();

      let route_a = Route {
          hostname: "myapp.localhost".to_string(),
          port: 4000,
          public_port: None,
          protocol: RouteProtocol::Http,
          pid: std::process::id(),
          owner_pid: std::process::id(),
          cwd: "/tmp".to_string(),
          created_at: Utc::now(),
          slot: 0,
          label: None,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      };
      let mut route_b = route_a.clone();
      route_b.port = 4001;
      route_b.slot = 0; // Should be auto-assigned to 1

      store.insert(route_a).await.unwrap();
      store.insert(route_b).await.unwrap();

      let slots = store.list_slots("myapp.localhost");
      assert_eq!(slots.len(), 2);
      let slot_numbers: Vec<u32> = slots.iter().map(|r| r.slot).collect();
      assert!(slot_numbers.contains(&0));
      assert!(slot_numbers.contains(&1));
  }

  #[tokio::test]
  async fn get_slot_returns_correct_slot() {
      let temp = TempDir::new().unwrap();
      let store = StateStore::new(temp.path().join("routes.json")).unwrap();

      let route_a = Route {
          hostname: "x.localhost".to_string(),
          port: 4000,
          public_port: None,
          protocol: RouteProtocol::Http,
          pid: std::process::id(),
          owner_pid: std::process::id(),
          cwd: "/tmp".to_string(),
          created_at: Utc::now(),
          slot: 0,
          label: Some("primary".to_string()),
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      };
      let mut route_b = route_a.clone();
      route_b.port = 4001;
      route_b.label = Some("secondary".to_string());
      route_b.slot = 0; // will auto-assign to 1

      store.insert(route_a).await.unwrap();
      store.insert(route_b).await.unwrap();

      let slot1 = store.get_slot("x.localhost", 1).unwrap();
      assert_eq!(slot1.port, 4001);
      assert_eq!(slot1.label, Some("secondary".to_string()));
  }

  #[tokio::test]
  async fn list_slots_returns_all_for_hostname() {
      let temp = TempDir::new().unwrap();
      let store = StateStore::new(temp.path().join("routes.json")).unwrap();

      for i in 0u16..3 {
          store.insert(Route {
              hostname: "multi.localhost".to_string(),
              port: 4000 + i,
              public_port: None,
              protocol: RouteProtocol::Http,
              pid: std::process::id(),
              owner_pid: std::process::id(),
              cwd: "/tmp".to_string(),
              created_at: Utc::now(),
              slot: 0,
              label: None,
              tailscale_url: None,
              tailscale_https_port: None,
              tailscale_funnel: false,
          }).await.unwrap();
      }

      let slots = store.list_slots("multi.localhost");
      assert_eq!(slots.len(), 3);
  }

  #[tokio::test]
  async fn remove_slot_leaves_primary() {
      let temp = TempDir::new().unwrap();
      let store = StateStore::new(temp.path().join("routes.json")).unwrap();

      let route_a = Route {
          hostname: "app.localhost".to_string(),
          port: 4000,
          public_port: None,
          protocol: RouteProtocol::Http,
          pid: std::process::id(),
          owner_pid: std::process::id(),
          cwd: "/tmp".to_string(),
          created_at: Utc::now(),
          slot: 0,
          label: None,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      };
      let mut route_b = route_a.clone();
      route_b.port = 4001;
      route_b.slot = 0; // will auto-assign to 1

      store.insert(route_a).await.unwrap();
      store.insert(route_b).await.unwrap();

      store.remove_slot("app.localhost", 1).await.unwrap();

      let slots = store.list_slots("app.localhost");
      assert_eq!(slots.len(), 1);
      assert_eq!(slots[0].slot, 0);
      assert_eq!(slots[0].port, 4000);
  }

  #[tokio::test]
  async fn list_returns_all_slots_flattened() {
      let temp = TempDir::new().unwrap();
      let store = StateStore::new(temp.path().join("routes.json")).unwrap();

      // Two slots under one hostname
      for i in 0u16..2 {
          store.insert(Route {
              hostname: "a.localhost".to_string(),
              port: 4000 + i,
              public_port: None,
              protocol: RouteProtocol::Http,
              pid: std::process::id(),
              owner_pid: std::process::id(),
              cwd: "/tmp".to_string(),
              created_at: Utc::now(),
              slot: 0,
              label: None,
              tailscale_url: None,
              tailscale_https_port: None,
              tailscale_funnel: false,
          }).await.unwrap();
      }
      // One slot under a different hostname
      store.insert(Route {
          hostname: "b.localhost".to_string(),
          port: 5000,
          public_port: None,
          protocol: RouteProtocol::Http,
          pid: std::process::id(),
          owner_pid: std::process::id(),
          cwd: "/tmp".to_string(),
          created_at: Utc::now(),
          slot: 0,
          label: None,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      }).await.unwrap();

      assert_eq!(store.list().len(), 3);
  }
  ```

- [ ] **Run all new slot tests**

  ```bash
  cargo test multi_slot_insert_auto_assigns_slot
  cargo test get_slot_returns_correct_slot
  cargo test list_slots_returns_all_for_hostname
  cargo test remove_slot_leaves_primary
  cargo test list_returns_all_slots_flattened
  ```

- [ ] **Fix `route_manager.rs`** — the `RouteManager` wraps `StateStore` and delegates to it. Open `src/route_manager.rs` and verify that all method calls to the inner `StateStore` still compile. Add delegation methods `get_slot`, `list_slots`, `remove_slot` mirroring the new `StateStore` methods:

  Read `src/route_manager.rs` first and add:
  ```rust
  pub fn get_slot(&self, hostname: &str, slot: u32) -> Option<crate::routes::Route> {
      self.store.get_slot(hostname, slot)
  }

  pub fn list_slots(&self, hostname: &str) -> Vec<crate::routes::Route> {
      self.store.list_slots(hostname)
  }

  pub async fn remove_slot(&self, hostname: &str, slot: u32) -> crate::error::Result<()> {
      self.store.remove_slot(hostname, slot).await
  }
  ```

- [ ] **Commit**

  ```bash
  git add src/routes.rs src/route_manager.rs
  git commit -m "feat: StateStore supports multiple routes per hostname (slots)"
  ```

---

## Task 3: Extend `Command::RegisterRoute` with slot and label; add `UpdateRoute` command

- [ ] **Modify `src/proto.rs`**

  In the `Command` enum, update the `RegisterRoute` variant to add two optional fields, and add the new `UpdateRoute` variant. The serde `#[serde(default)]` on the new `RegisterRoute` fields ensures old clients sending JSON without these fields still deserialize successfully.

  Replace:
  ```rust
  RegisterRoute {
      hostname: String,
      port: u16,
      public_port: Option<u16>,
      protocol: crate::routes::RouteProtocol,
      pid: u32,
      cwd: String,
  },
  ```

  With:
  ```rust
  RegisterRoute {
      hostname: String,
      port: u16,
      public_port: Option<u16>,
      protocol: crate::routes::RouteProtocol,
      pid: u32,
      cwd: String,
      #[serde(default)]
      slot: Option<u32>,
      #[serde(default)]
      label: Option<String>,
  },
  UpdateRoute {
      hostname: String,
      #[serde(default)]
      tailscale_url: Option<String>,
      #[serde(default)]
      tailscale_https_port: Option<u16>,
      #[serde(default)]
      tailscale_funnel: Option<bool>,
  },
  ```

- [ ] **Add new tests in `src/proto.rs`** inside the existing `mod tests` block:

  ```rust
  #[test]
  fn register_route_without_slot_defaults_to_none() {
      // Old-format JSON without the slot/label fields must deserialize successfully
      let json = r#"{"cmd":"register_route","hostname":"app.localhost","port":4000,"public_port":null,"protocol":"http","pid":123,"cwd":"/tmp"}"#;
      let cmd: Command = serde_json::from_str(json).expect("should deserialize old format");
      match cmd {
          Command::RegisterRoute { hostname, slot, label, .. } => {
              assert_eq!(hostname, "app.localhost");
              assert_eq!(slot, None);
              assert_eq!(label, None);
          }
          other => panic!("unexpected command: {other:?}"),
      }
  }

  #[test]
  fn round_trips_update_route_command() {
      let cmd = Command::UpdateRoute {
          hostname: "myapp.localhost".to_string(),
          tailscale_url: Some("https://mynode.ts.net".to_string()),
          tailscale_https_port: Some(443),
          tailscale_funnel: Some(false),
      };
      let json = serde_json::to_string(&cmd).expect("serialize");
      let back: Command = serde_json::from_str(&json).expect("deserialize");
      match back {
          Command::UpdateRoute {
              hostname,
              tailscale_url,
              tailscale_https_port,
              tailscale_funnel,
          } => {
              assert_eq!(hostname, "myapp.localhost");
              assert_eq!(tailscale_url, Some("https://mynode.ts.net".to_string()));
              assert_eq!(tailscale_https_port, Some(443));
              assert_eq!(tailscale_funnel, Some(false));
          }
          other => panic!("unexpected: {other:?}"),
      }
  }
  ```

- [ ] **Run new proto tests**

  ```bash
  cargo test register_route_without_slot_defaults_to_none
  cargo test round_trips_update_route_command
  ```

- [ ] **Fix the existing `round_trips_register_route_with_protocol` test**

  The test at line 246 uses pattern matching with `..` so it will still compile. Verify:

  ```bash
  cargo test round_trips_register_route_with_protocol
  ```

- [ ] **Commit**

  ```bash
  git add src/proto.rs
  git commit -m "feat: extend RegisterRoute with slot/label; add UpdateRoute IPC command"
  ```

---

## Task 4: Handle UpdateRoute in daemon IPC; wire slot/label into RegisterRoute handler

- [ ] **Modify `src/daemon/ipc.rs`**

  **Part A — Update the `RegisterRoute` match arm** (around line 309) to unpack and use the new `slot` and `label` fields:

  Replace:
  ```rust
  Command::RegisterRoute {
      hostname,
      port,
      public_port,
      protocol,
      pid,
      cwd,
  } => {
      // Validate hostname: must be non-empty, no newlines, reasonable length
      if hostname.is_empty() || hostname.len() > 253 || hostname.contains('\n') || hostname.contains('\r') {
          return Response::err("invalid hostname");
      }
      let route = crate::routes::Route {
          hostname: hostname.clone(),
          port,
          public_port,
          protocol,
          pid,
          owner_pid: pid,
          cwd,
          created_at: chrono::Utc::now(),
          slot: 0,
          label: None,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      };
  ```

  With:
  ```rust
  Command::RegisterRoute {
      hostname,
      port,
      public_port,
      protocol,
      pid,
      cwd,
      slot,
      label,
  } => {
      // Validate hostname: must be non-empty, no newlines, reasonable length
      if hostname.is_empty() || hostname.len() > 253 || hostname.contains('\n') || hostname.contains('\r') {
          return Response::err("invalid hostname");
      }
      let route = crate::routes::Route {
          hostname: hostname.clone(),
          port,
          public_port,
          protocol,
          pid,
          owner_pid: pid,
          cwd,
          created_at: chrono::Utc::now(),
          // slot: None means auto-assign (StateStore::insert handles it)
          slot: slot.unwrap_or(0),
          label,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      };
  ```

  **Part B — Add `UpdateRoute` handler** in the `dispatch` function. Add a new match arm before `Command::Run { .. } =>`:

  ```rust
  Command::UpdateRoute {
      hostname,
      tailscale_url,
      tailscale_https_port,
      tailscale_funnel,
  } => {
      match manager.get(&hostname) {
          None => Response::err(format!("no route for \"{hostname}\"")),
          Some(mut route) => {
              if let Some(url) = tailscale_url {
                  route.tailscale_url = Some(url);
              }
              if let Some(port) = tailscale_https_port {
                  route.tailscale_https_port = Some(port);
              }
              if let Some(funnel) = tailscale_funnel {
                  route.tailscale_funnel = funnel;
              }
              match manager.insert(route).await {
                  Ok(_) => Response::ok_empty(),
                  Err(e) => Response::err(e.to_string()),
              }
          }
      }
  }
  ```

- [ ] **Add test `update_route_sets_tailscale_url`** in the `mod tests` block at the bottom of `src/daemon/ipc.rs`:

  ```rust
  #[tokio::test]
  async fn update_route_sets_tailscale_url() {
      let dir = tempfile::tempdir().unwrap();
      let store = StateStore::new(dir.path().join("routes.json")).unwrap();
      let tcp_routes = TcpRouteManager::default();
      let manager = RouteManager::new(store.clone(), tcp_routes);

      // First register a route
      store.insert(crate::routes::Route {
          hostname: "ts-app.localhost".to_string(),
          port: 4000,
          public_port: None,
          protocol: crate::routes::RouteProtocol::Http,
          pid: std::process::id(),
          owner_pid: std::process::id(),
          cwd: "/tmp".to_string(),
          created_at: chrono::Utc::now(),
          slot: 0,
          label: None,
          tailscale_url: None,
          tailscale_https_port: None,
          tailscale_funnel: false,
      }).await.unwrap();

      // Now send UpdateRoute
      let response = dispatch(
          Command::UpdateRoute {
              hostname: "ts-app.localhost".to_string(),
              tailscale_url: Some("https://mynode.ts.net".to_string()),
              tailscale_https_port: Some(443),
              tailscale_funnel: Some(false),
          },
          manager.clone(),
          std::time::Instant::now(),
          DaemonMode::TcpOnly,
          dir.path().join("portal.sock"),
          dir.path().join("daemon.pid"),
          false,
          80,
          443,
      ).await;

      assert!(response.ok, "UpdateRoute should succeed: {:?}", response.error);
      let route = store.get("ts-app.localhost").unwrap();
      assert_eq!(route.tailscale_url, Some("https://mynode.ts.net".to_string()));
      assert_eq!(route.tailscale_https_port, Some(443));
      assert!(!route.tailscale_funnel);
  }
  ```

- [ ] **Run new IPC test**

  ```bash
  cargo test update_route_sets_tailscale_url
  ```

- [ ] **Commit**

  ```bash
  git add src/daemon/ipc.rs
  git commit -m "feat: daemon handles UpdateRoute and slot-aware RegisterRoute"
  ```

---

## Task 5: Switch `serve_https` to HTTP/2 auto-negotiation

- [ ] **Fix `is_http_method_prefix` in `src/proxy.rs` to include `b"PRI"`**

  The current match at line 42–45 in `src/proxy.rs` does NOT include `b"PRI"`. HTTP/2 connections send a preface starting with `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n`. Without `b"PRI"` in the match, H2 connections fall through to the TCP bridge path and get dropped.

  Replace:
  ```rust
  matches!(
      &buf[..3],
      b"GET" | b"PUT" | b"POS" | b"HEA" | b"DEL" | b"PAT" | b"OPT" | b"CON"
  )
  ```

  With:
  ```rust
  matches!(
      &buf[..3],
      b"GET" | b"PUT" | b"POS" | b"HEA" | b"DEL" | b"PAT" | b"OPT" | b"CON" | b"PRI"
  )
  ```

- [ ] **Modify `serve_https` in `src/daemon/mod.rs` to use `auto::Builder`**

  The current implementation (lines 321–456) uses `hyper::server::conn::http1::Builder`. Replace the entire function with the version below that:
  1. Adds `h2` and `http/1.1` to the rustls ALPN protocols so TLS negotiation tells browsers H2 is supported.
  2. Uses `hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())` instead of `http1::Builder`.
  3. Calls `.serve_connection_with_upgrades(io, service_fn(...))` so both H1 WebSocket upgrades and H2 Extended CONNECT upgrades work.

  Replace the content of the `serve_https` function (from the `use hyper::server::conn::http1;` line through the end of the function) with:

  ```rust
  async fn serve_https(
      listener: tokio::net::TcpListener,
      cert_store: CertStore,
      routes: StateStore,
      inspector: Option<crate::inspector::InspectorSender>,
      wildcard: bool,
  ) {
      use hyper_util::server::conn::auto::Builder as AutoBuilder;
      use hyper_util::rt::{TokioExecutor, TokioIo};
      use rustls::ServerConfig;
      use std::sync::Arc;
      use tokio_rustls::TlsAcceptor;

      let resolver = Arc::new(crate::certs::PortlessCertResolver::new(cert_store));
      let mut tls_config = ServerConfig::builder()
          .with_no_client_auth()
          .with_cert_resolver(resolver);
      // Advertise both HTTP/2 and HTTP/1.1 via ALPN so browsers can negotiate H2
      tls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
      let acceptor = TlsAcceptor::from(Arc::new(tls_config));

      loop {
          let Ok((tcp_stream, _)) = listener.accept().await else {
              continue;
          };
          let acceptor = acceptor.clone();
          let routes = routes.clone();
          let inspector = inspector.clone();
          let wc = wildcard;
          tokio::spawn(async move {
              // Handle Postgres SSLRequest: if the first byte is 0x00, read the
              // 8-byte SSLRequest message and respond with 'S' (yes, use SSL).
              // Then the client sends a normal TLS ClientHello.
              let mut tcp_stream = tcp_stream;
              let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                  Ok(b) => b,
                  Err(_) => return,
              };
              if first == 0x00 {
                  // Likely Postgres SSLRequest: 8 bytes = 4-byte length + 4-byte code (80877103)
                  let mut ssl_req = [0u8; 8];
                  if tokio::io::AsyncReadExt::read_exact(&mut tcp_stream, &mut ssl_req).await.is_err() {
                      return;
                  }
                  let code = u32::from_be_bytes([ssl_req[4], ssl_req[5], ssl_req[6], ssl_req[7]]);
                  if code != 80877103 {
                      return; // Not a Postgres SSLRequest
                  }
                  // Respond with 'S' (yes, use SSL)
                  if tokio::io::AsyncWriteExt::write_all(&mut tcp_stream, b"S").await.is_err() {
                      return;
                  }
                  // Now the client will send a TLS ClientHello — re-peek
                  let first = match crate::proxy::peek_first_byte(&tcp_stream).await {
                      Ok(b) => b,
                      Err(_) => return,
                  };
                  if !crate::proxy::is_tls_client_hello(first) {
                      return;
                  }
              } else if !crate::proxy::is_tls_client_hello(first) {
                  return;
              }

              let Ok(mut tls_stream) = acceptor.accept(tcp_stream).await else {
                  return;
              };

              // Read first bytes to detect HTTP vs raw TCP
              let mut peek_buf = [0u8; 4];
              let n = match tokio::io::AsyncReadExt::read(&mut tls_stream, &mut peek_buf).await {
                  Ok(0) => return,
                  Ok(n) => n,
                  Err(_) => return,
              };
              let peeked = peek_buf[..n].to_vec();

              if crate::proxy::is_http_method_prefix(&peeked) {
                  // HTTP path: replay peeked bytes + rest of stream → hyper auto builder
                  // auto::Builder handles both HTTP/1.1 and HTTP/2 transparently
                  let prefixed = crate::proxy::PrefixedIo::new(peeked, tls_stream);
                  let io = TokioIo::new(prefixed);
                  AutoBuilder::new(TokioExecutor::new())
                      .serve_connection_with_upgrades(
                          io,
                          hyper::service::service_fn(move |req| {
                              let r = routes.clone();
                              let insp = inspector.clone();
                              async move { crate::proxy::handle_https_request(req, r, insp, wc).await }
                          }),
                      )
                      .await
                      .ok();
              } else {
                  // TCP bridge: extract SNI hostname → look up route → bridge
                  let sni = tls_stream
                      .get_ref()
                      .1
                      .server_name()
                      .map(|s| s.to_string());

                  let hostname = match sni {
                      Some(h) => h,
                      None => {
                          tracing::debug!("non-HTTP connection without SNI hostname, dropping");
                          return;
                      }
                  };

                  let route = match routes.get(&hostname) {
                      Some(r) => r,
                      None => {
                          tracing::debug!("no route for TCP connection to {hostname}");
                          return;
                      }
                  };

                  let mut backend = match tokio::net::TcpStream::connect(("127.0.0.1", route.port)).await {
                      Ok(s) => s,
                      Err(e) => {
                          tracing::debug!("TCP bridge: failed to connect to backend port {}: {e}", route.port);
                          return;
                      }
                  };

                  // Send the already-read bytes to the backend
                  if tokio::io::AsyncWriteExt::write_all(&mut backend, &peeked).await.is_err() {
                      return;
                  }

                  // Bridge the rest bidirectionally
                  let _ = tokio::io::copy_bidirectional(&mut tls_stream, &mut backend).await;
              }
          });
      }
  }
  ```

  Note: The `ServerConfig::builder().with_no_client_auth().with_cert_resolver(resolver)` now returns a mutable `ServerConfig` (not wrapped in `Arc` immediately) so we can set `alpn_protocols` before wrapping.

- [ ] **Add test `is_http_method_prefix_includes_pri`** in `src/proxy.rs` test block:

  ```rust
  #[test]
  fn is_http_method_prefix_includes_pri() {
      // HTTP/2 connection preface starts with "PRI * HTTP/2.0..."
      assert!(is_http_method_prefix(b"PRI "), "H2 preface should be detected as HTTP");
      // Existing methods still work
      assert!(is_http_method_prefix(b"GET "));
      assert!(is_http_method_prefix(b"POST"));
      // Non-HTTP should still return false
      assert!(!is_http_method_prefix(b"\x16\x03\x01")); // TLS ClientHello
  }
  ```

- [ ] **Run the new proxy test**

  ```bash
  cargo test is_http_method_prefix_includes_pri
  ```

- [ ] **Verify build compiles cleanly**

  ```bash
  cargo build 2>&1 | head -40
  ```

- [ ] **Commit**

  ```bash
  git add src/proxy.rs src/daemon/mod.rs
  git commit -m "feat: serve_https uses HTTP/2 auto-negotiation via hyper-util auto::Builder"
  ```

---

## Task 6: Handle RFC 8441 WebSocket over HTTP/2 (Extended CONNECT)

- [ ] **Add `is_h2_websocket_connect` function in `src/proxy.rs`**

  Add this function immediately after the `is_websocket_upgrade` function (after line 273):

  ```rust
  /// Detect RFC 8441 Extended CONNECT — WebSocket over HTTP/2.
  /// Browsers send CONNECT with `:protocol: websocket` when upgrading over H2.
  /// The `protocol` header is the lowercase pseudo-header name used by hyper after H2 parsing.
  pub fn is_h2_websocket_connect<B>(req: &Request<B>) -> bool {
      req.method() == http::Method::CONNECT
          && req.headers()
              .get("protocol")
              .and_then(|v| v.to_str().ok())
              .map(|v| v.eq_ignore_ascii_case("websocket"))
              .unwrap_or(false)
  }
  ```

- [ ] **Wire the new check into `handle_https_request`**

  In `handle_https_request`, after the `is_websocket_upgrade` check (around line 379–383), add the H2 Extended CONNECT check:

  ```rust
  if is_websocket_upgrade(&req) {
      return handle_websocket(req, route.port).await;
  }

  if is_h2_websocket_connect(&req) {
      return handle_websocket(req, route.port).await;
  }
  ```

- [ ] **Add tests in the `#[cfg(test)]` block of `src/proxy.rs`**

  Find or create the test module. Add:

  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn h2_websocket_connect_is_detected() {
          let req = http::Request::builder()
              .method(http::Method::CONNECT)
              .uri("https://myapp.localhost/.well-known/ws")
              .header("protocol", "websocket")
              .body(())
              .unwrap();
          assert!(is_h2_websocket_connect(&req));
      }

      #[test]
      fn regular_connect_is_not_h2_ws() {
          // Plain CONNECT without protocol header (tunnel, not WebSocket)
          let req = http::Request::builder()
              .method(http::Method::CONNECT)
              .uri("myapp.localhost:443")
              .body(())
              .unwrap();
          assert!(!is_h2_websocket_connect(&req));
      }

      #[test]
      fn h1_websocket_upgrade_is_not_h2_ws() {
          // HTTP/1.1 WebSocket upgrade should NOT be detected by is_h2_websocket_connect
          let req = http::Request::builder()
              .method(http::Method::GET)
              .uri("https://myapp.localhost/ws")
              .header("upgrade", "websocket")
              .header("connection", "upgrade")
              .body(())
              .unwrap();
          assert!(!is_h2_websocket_connect(&req));
          assert!(is_websocket_upgrade(&req));
      }
  }
  ```

  Note: if a `mod tests` block already exists in `src/proxy.rs`, add these three test functions inside it rather than creating a duplicate module.

- [ ] **Run the new tests**

  ```bash
  cargo test h2_websocket_connect_is_detected
  cargo test regular_connect_is_not_h2_ws
  cargo test h1_websocket_upgrade_is_not_h2_ws
  ```

- [ ] **Commit**

  ```bash
  git add src/proxy.rs
  git commit -m "feat: handle RFC 8441 WebSocket over HTTP/2 (Extended CONNECT)"
  ```

---

## Task 7: Add `--h2c` flag and h2c upstream proxying

- [ ] **Modify `src/config.rs`**

  Add `pub h2c: bool` to `ProxyConfig`:

  ```rust
  pub struct ProxyConfig {
      pub tld: String,
      pub port_range: (u16, u16),
      pub https: bool,
      pub http_port: u16,
      pub https_port: u16,
      pub wildcard: bool,
      pub lan: bool,
      pub lan_ip: Option<String>,
      pub h2c: bool,
  }
  ```

  Update `ProxyConfig::default()`:
  ```rust
  pub h2c: false,
  ```

  Add `h2c: Option<bool>` to `PartialProxyConfig`:
  ```rust
  struct PartialProxyConfig {
      tld: Option<String>,
      port_range: Option<(u16, u16)>,
      https: Option<bool>,
      http_port: Option<u16>,
      https_port: Option<u16>,
      wildcard: Option<bool>,
      lan: Option<bool>,
      lan_ip: Option<String>,
      h2c: Option<bool>,
  }
  ```

  Add to `apply_partial`:
  ```rust
  if let Some(h2c) = partial.proxy.h2c {
      config.proxy.h2c = h2c;
  }
  ```

  Add to `apply_env_overrides` in the match:
  ```rust
  "PORTLESS_H2C" => {
      config.proxy.h2c = matches!(
          value.to_ascii_lowercase().as_str(),
          "1" | "true" | "yes" | "on"
      );
  }
  ```

- [ ] **Modify `src/cli/mod.rs`**

  Add `#[arg(long)] h2c: bool` to `CliCommand::Run`:

  ```rust
  Run {
      #[arg(long)]
      hostname: Option<String>,
      #[arg(long)]
      port: Option<u16>,
      #[arg(long, short = 'q', help = "Suppress startup banner and running output")]
      quiet: bool,
      #[arg(long)]
      tcp: bool,
      #[arg(long)]
      force: bool,
      #[arg(long)]
      lan: bool,
      #[arg(long, value_name = "ADDR")]
      ip: Option<String>,
      /// Use HTTP/2 cleartext (h2c) for upstream connections (gRPC backends)
      #[arg(long)]
      h2c: bool,
      #[arg(trailing_var_arg = true, required = true)]
      args: Vec<String>,
  },
  ```

  Update the `CliCommand::Run` match arm to unpack `h2c` and set `config.proxy.h2c`:

  ```rust
  CliCommand::Run {
      hostname,
      port,
      quiet,
      tcp,
      force,
      lan,
      ip,
      h2c,
      args,
  } => {
      let cwd = std::env::current_dir()?;
      let mut config = crate::config::Config::load(&cwd)?;
      if lan { config.proxy.lan = true; }
      if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
      if h2c { config.proxy.h2c = true; }
      let resolved_args = crate::detect::resolve_run_args(&cwd, args);
      do_run(
          cwd,
          config,
          resolved_args,
          hostname,
          port,
          false,
          quiet,
          tcp,
          force,
      )
      .await?;
  }
  ```

- [ ] **Modify `src/proxy.rs` — add H2C client and wire it in**

  Add `H2C_CLIENT` static and modify `handle_https_request` to accept a `h2c: bool` parameter.

  Change the signature:
  ```rust
  pub async fn handle_https_request(
      req: Request<Incoming>,
      routes: crate::routes::StateStore,
      inspector: Option<crate::inspector::InspectorSender>,
      wildcard: bool,
      h2c: bool,
  ) -> Result<Response<BoxBodyType>, std::convert::Infallible> {
  ```

  In the body, replace the `HTTP_CLIENT` block (around lines 441–445) with:

  ```rust
  // Reuse a shared HTTP client for connection pooling and keep-alive.
  // When h2c is true, use an HTTP/2-only client (for gRPC and h2c backends).
  static HTTP_CLIENT: std::sync::OnceLock<Client<HttpConnector, BoxBodyType>> =
      std::sync::OnceLock::new();
  static H2C_CLIENT: std::sync::OnceLock<Client<HttpConnector, BoxBodyType>> =
      std::sync::OnceLock::new();

  let client = if h2c {
      H2C_CLIENT.get_or_init(|| {
          Client::builder(TokioExecutor::new())
              .http2_only(true)
              .build_http()
      })
  } else {
      HTTP_CLIENT.get_or_init(|| Client::builder(TokioExecutor::new()).build_http())
  };
  ```

  Update the two call sites in `src/daemon/mod.rs` and anywhere else `handle_https_request` is invoked to pass `false` as the `h2c` argument (maintaining existing behaviour). In `serve_https`:

  ```rust
  async move { crate::proxy::handle_https_request(req, r, insp, wc, false).await }
  ```

  If `h2c` is needed per-request in future, it would be read from the config passed into `serve_https`. For now, pass `false` — the flag is applied globally via the config and we read `config.proxy.h2c` in `do_run` to decide whether to set it. A follow-up task would thread it through the daemon. For this task, the flag works by setting the flag on the spawned route (future enhancement) — the compile-time wiring is the goal.

  Note: The `--h2c` flag is wired into the CLI config object. Passing the h2c flag all the way from `do_run` to the proxy's per-request handler requires threading it through daemon startup. For v0.3.0 the h2c flag gates the H2C client at the process level: when `portal run --h2c ...` is used, the daemon could be started with `PORTLESS_H2C=1` injected. However, that is a daemon-restart level concern. To keep the change minimal and compilable, the `h2c` parameter is added to the function signature and all call sites pass `false` for now. The full wiring is tracked as a follow-up.

- [ ] **Add test `h2c_config_defaults_to_false`** in `src/config.rs`:

  ```rust
  #[test]
  fn h2c_config_defaults_to_false() {
      let config = Config::load_with_paths(None, None, &[]).unwrap();
      assert!(!config.proxy.h2c, "h2c should default to false");
  }
  ```

  And add a test for the env var:
  ```rust
  #[test]
  fn h2c_env_var_sets_h2c() {
      let env = [("PORTLESS_H2C", "1")];
      let config = Config::load_with_paths(None, None, &env).unwrap();
      assert!(config.proxy.h2c);
  }
  ```

- [ ] **Run new config tests**

  ```bash
  cargo test h2c_config_defaults_to_false
  cargo test h2c_env_var_sets_h2c
  ```

- [ ] **Commit**

  ```bash
  git add src/config.rs src/cli/mod.rs src/proxy.rs src/daemon/mod.rs
  git commit -m "feat: add --h2c flag for HTTP/2 cleartext upstream proxying"
  ```

---

## Task 8: Create `src/tailscale.rs` with Tailscale CLI wrapper

- [ ] **Create `src/tailscale.rs`** with the full implementation:

  ```rust
  use crate::error::{Error, Result};
  use std::process::Command;

  const SERVE_PORTS: &[u16] = &[443, 8443, 8444, 8445, 8446, 8447, 8448, 8449, 8450];
  const FUNNEL_PORTS: &[u16] = &[443, 8443, 10000];

  /// Returns true if the `tailscale` CLI is present and exits with status 0.
  pub fn is_installed() -> bool {
      Command::new("tailscale")
          .arg("version")
          .output()
          .map(|o| o.status.success())
          .unwrap_or(false)
  }

  /// Returns the Tailscale node DNS name, e.g. `myhost.tail12345.ts.net`.
  /// Reads `tailscale status --json` and extracts `Self.DNSName`.
  pub fn get_node_name() -> Result<String> {
      let out = Command::new("tailscale")
          .args(["status", "--json"])
          .output()
          .map_err(|e| Error::Other(format!("tailscale status failed: {e}")))?;
      if !out.status.success() {
          return Err(Error::Other("tailscale not connected".to_string()));
      }
      let val: serde_json::Value = serde_json::from_slice(&out.stdout)?;
      let name = val["Self"]["DNSName"]
          .as_str()
          .ok_or_else(|| Error::Other("missing DNSName in tailscale status".to_string()))?
          .trim_end_matches('.')
          .to_string();
      Ok(name)
  }

  /// Returns the list of TCP ports currently configured in `tailscale serve status`.
  pub fn used_ports() -> Vec<u16> {
      let Ok(out) = Command::new("tailscale")
          .args(["serve", "status", "--json"])
          .output()
      else {
          return vec![];
      };
      let Ok(val) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
          return vec![];
      };
      val["TCP"]
          .as_object()
          .map(|m| {
              m.keys()
                  .filter_map(|k| k.trim_start_matches(':').parse().ok())
                  .collect()
          })
          .unwrap_or_default()
  }

  fn find_free_port(funnel: bool) -> Option<u16> {
      let in_use = used_ports();
      let candidates = if funnel { FUNNEL_PORTS } else { SERVE_PORTS };
      candidates.iter().copied().find(|p| !in_use.contains(p))
  }

  /// Register a local port with Tailscale Serve or Funnel.
  ///
  /// Returns `(https_port, public_url)` on success.
  /// - `https_port` is the port Tailscale listens on externally.
  /// - `public_url` is the full URL clients should use.
  pub fn register(local_port: u16, funnel: bool) -> Result<(u16, String)> {
      let https_port = find_free_port(funnel)
          .ok_or_else(|| Error::Other("no available Tailscale port".to_string()))?;

      let subcmd = if funnel { "funnel" } else { "serve" };
      let out = Command::new("tailscale")
          .args([
              subcmd,
              "--bg",
              "--yes",
              &format!("--https={https_port}"),
              &format!("http://127.0.0.1:{local_port}"),
          ])
          .output()
          .map_err(|e| Error::Other(format!("tailscale {subcmd} failed: {e}")))?;

      if !out.status.success() {
          let stderr = String::from_utf8_lossy(&out.stderr);
          if stderr.contains("Funnel not available") || stderr.contains("funnel") {
              return Err(Error::Other(
                  "Tailscale Funnel is not enabled on this tailnet. Run: tailscale funnel on"
                      .to_string(),
              ));
          }
          return Err(Error::Other(format!("tailscale {subcmd} failed: {stderr}")));
      }

      let node = get_node_name()?;
      let url = if https_port == 443 {
          format!("https://{node}")
      } else {
          format!("https://{node}:{https_port}")
      };

      Ok((https_port, url))
  }

  /// Remove a Tailscale Serve or Funnel mapping.
  pub fn unregister(https_port: u16, funnel: bool) -> Result<()> {
      let subcmd = if funnel { "funnel" } else { "serve" };
      Command::new("tailscale")
          .args([
              subcmd,
              "--yes",
              &format!("--https={https_port}"),
              "off",
          ])
          .output()
          .map_err(|e| Error::Other(format!("tailscale {subcmd} off failed: {e}")))?;
      Ok(())
  }

  #[cfg(test)]
  mod tests {
      use super::*;

      #[test]
      fn find_free_port_avoids_in_use() {
          // Directly test the port-selection logic (without calling the CLI)
          let candidates = &[443u16, 8443, 8444];
          let in_use = vec![443u16];
          let result = candidates.iter().copied().find(|p| !in_use.contains(p));
          assert_eq!(result, Some(8443));
      }

      #[test]
      fn find_free_port_returns_none_when_all_used() {
          let candidates = &[443u16, 8443];
          let in_use = vec![443u16, 8443];
          let result = candidates.iter().copied().find(|p| !in_use.contains(p));
          assert_eq!(result, None);
      }

      #[test]
      fn funnel_port_candidates_are_subset() {
          // Funnel only supports 443, 8443, 10000 per Tailscale docs
          assert_eq!(FUNNEL_PORTS, &[443, 8443, 10000]);
      }

      #[test]
      fn serve_port_candidates_include_funnel_ports() {
          for &p in FUNNEL_PORTS {
              assert!(SERVE_PORTS.contains(&p), "SERVE_PORTS must include funnel port {p}");
          }
      }

      #[test]
      fn is_installed_does_not_panic() {
          // Just verify it returns without panicking (result depends on environment)
          let _ = is_installed();
      }
  }
  ```

  Note: `Error::Other` does not exist yet in `src/error.rs`. Add it:

  In `src/error.rs`, add:
  ```rust
  #[error("{0}")]
  Other(String),
  ```

- [ ] **Add `pub mod tailscale;` to `src/lib.rs`**

  Append to `src/lib.rs`:
  ```
  pub mod tailscale;
  ```

- [ ] **Run tailscale tests**

  ```bash
  cargo test tailscale::tests
  ```

- [ ] **Commit**

  ```bash
  git add src/tailscale.rs src/lib.rs src/error.rs
  git commit -m "feat: add Tailscale CLI wrapper (src/tailscale.rs)"
  ```

---

## Task 9: Wire Tailscale into `portal run` CLI

- [ ] **Add `PORTLESS_TAILSCALE_URL_ENV` constant to `src/process.rs`**

  At the top of `src/process.rs`, after the existing `PORTLESS_URL_ENV` constant:

  ```rust
  pub const PORTLESS_TAILSCALE_URL_ENV: &str = "PORTLESS_TAILSCALE_URL";
  ```

- [ ] **Add `--tailscale` and `--funnel` args to `CliCommand::Run` in `src/cli/mod.rs`**

  Inside the `Run { .. }` variant, add after the `h2c` field:

  ```rust
  /// Share this app on your Tailscale tailnet
  #[arg(long)]
  tailscale: bool,
  /// Share this app publicly via Tailscale Funnel (implies --tailscale)
  #[arg(long)]
  funnel: bool,
  ```

- [ ] **Update the `CliCommand::Run` match arm** to unpack the new fields and pass them to `do_run`:

  ```rust
  CliCommand::Run {
      hostname,
      port,
      quiet,
      tcp,
      force,
      lan,
      ip,
      h2c,
      tailscale,
      funnel,
      args,
  } => {
      let cwd = std::env::current_dir()?;
      let mut config = crate::config::Config::load(&cwd)?;
      if lan { config.proxy.lan = true; }
      if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
      if h2c { config.proxy.h2c = true; }
      // --funnel implies --tailscale
      let use_tailscale = tailscale || funnel;
      let resolved_args = crate::detect::resolve_run_args(&cwd, args);
      do_run(
          cwd,
          config,
          resolved_args,
          hostname,
          port,
          false,
          quiet,
          tcp,
          force,
          use_tailscale,
          funnel,
      )
      .await?;
  }
  ```

- [ ] **Update `do_run` signature and body** in `src/cli/mod.rs`

  Change the signature to add `tailscale: bool` and `funnel: bool`:

  ```rust
  async fn do_run(
      cwd: std::path::PathBuf,
      config: crate::config::Config,
      args: Vec<String>,
      hostname_override: Option<String>,
      port_override: Option<u16>,
      use_full_registry: bool,
      quiet: bool,
      tcp: bool,
      force: bool,
      tailscale: bool,
      funnel: bool,
  ) -> Result<()> {
  ```

  Update the `CliCommand::Start` call to `do_run` (passes `false, false` for tailscale/funnel):

  ```rust
  do_run(
      cwd,
      config,
      args,
      hostname_override,
      None,
      true,
      quiet,
      false,
      false,
      false,  // tailscale
      false,  // funnel
  )
  .await?;
  ```

  Update the monorepo `run_monorepo` call if it calls `do_run` as well (check the function signature for `run_monorepo` — it does NOT call `do_run` directly, it uses its own inline logic; no change needed there).

  In `do_run` body, after the `RegisterRoute` IPC block succeeds and before the banner print, add Tailscale registration:

  ```rust
  // Tailscale sharing
  let mut ts_https_port: Option<u16> = None;
  if tailscale && !tcp {
      if !crate::tailscale::is_installed() {
          eprintln!("error: tailscale CLI not found in PATH. Install it from https://tailscale.com");
          // Don't exit — continue without Tailscale
      } else {
          match crate::tailscale::register(port, funnel) {
              Ok((https_port, ts_url)) => {
                  ts_https_port = Some(https_port);
                  // Persist the Tailscale URL in the daemon
                  if let Ok(mut s) = ipc_connect().await {
                      let _ = write_frame(
                          &mut s,
                          &crate::proto::Command::UpdateRoute {
                              hostname: hostname.clone(),
                              tailscale_url: Some(ts_url.clone()),
                              tailscale_https_port: Some(https_port),
                              tailscale_funnel: Some(funnel),
                          },
                      ).await;
                      let _: crate::proto::Response = read_frame(&mut s).await
                          .unwrap_or(crate::proto::Response::ok_empty());
                  }
                  // Inject the URL into the child's environment
                  extra_env.push((
                      crate::process::PORTLESS_TAILSCALE_URL_ENV.to_string(),
                      ts_url.clone(),
                  ));
                  if !quiet {
                      println!("  Tailscale: {ts_url}");
                  }
              }
              Err(e) => {
                  eprintln!("warning: tailscale: {e}");
              }
          }
      }
  }
  ```

  Note: `extra_env` is built before `spawn_child` is called. The Tailscale registration block must happen AFTER the route is registered (so the daemon can accept `UpdateRoute`) but BEFORE `spawn_child` (so the env var is available to the child). Move the `extra_env` build and the Tailscale block to be between `RegisterRoute` IPC and the `spawn_child` call.

  The current `do_run` calls `spawn_child` before registering the route (the child is spawned, then `child_pid` is obtained, then `RegisterRoute` is sent). This ordering must change: we need the route registered before calling `tailscale::register`. To fix this, use the CLI process's own PID as a placeholder for `owner_pid`, then send a follow-up `UpdateRoute`.

  The cleanest approach without restructuring `do_run` significantly: perform Tailscale registration AFTER child spawn and route registration, then update `extra_env` retroactively by re-spawning — that is too complex. Instead, use the simpler approach: start Tailscale registration in a background task that runs concurrently with child spawn, and use the `UpdateRoute` IPC to patch the route. The child will not have `PORTLESS_TAILSCALE_URL` in its env for this run, but the daemon route will have it for inspection/display. This matches how the spec describes it: "After route registered with daemon: call `tailscale::register(port, funnel)`".

  Revised `do_run` flow for Tailscale (after line 1009, after the `RegisterRoute` IPC block):

  ```rust
  // After RegisterRoute IPC succeeds — register with Tailscale if requested
  let mut ts_https_port: Option<u16> = None;
  if tailscale && !tcp {
      if !crate::tailscale::is_installed() {
          eprintln!("warning: tailscale CLI not found in PATH");
      } else {
          match crate::tailscale::register(port, funnel) {
              Ok((https_port, ts_url)) => {
                  ts_https_port = Some(https_port);
                  if let Ok(mut s) = ipc_connect().await {
                      let _ = write_frame(&mut s, &crate::proto::Command::UpdateRoute {
                          hostname: hostname.clone(),
                          tailscale_url: Some(ts_url.clone()),
                          tailscale_https_port: Some(https_port),
                          tailscale_funnel: Some(funnel),
                      }).await;
                      let _: crate::proto::Response = read_frame(&mut s).await
                          .unwrap_or(crate::proto::Response::ok_empty());
                  }
                  if !quiet {
                      println!("  Tailscale: {ts_url}");
                  }
              }
              Err(e) => {
                  eprintln!("warning: tailscale: {e}");
              }
          }
      }
  }
  ```

  Update the `tokio::select!` cleanup block at the end of `do_run` to call `tailscale::unregister` on exit:

  ```rust
  tokio::select! {
      _ = child.wait() => {},
      _ = tokio::signal::ctrl_c() => {
          let _ = crate::process::stop_child(&mut child).await;
          if let Ok(mut s) = ipc_connect().await {
              let _ = write_frame(&mut s, &Command::Stop { hostname: hostname.clone() }).await;
              let _: crate::proto::Response = read_frame(&mut s).await
                  .unwrap_or(crate::proto::Response::ok_empty());
          }
      }
  }

  // Clean up Tailscale mapping after child exits
  if let Some(https_port) = ts_https_port {
      if let Err(e) = crate::tailscale::unregister(https_port, funnel) {
          tracing::warn!("tailscale unregister failed: {e}");
      }
  }
  ```

- [ ] **Add test `run_command_has_tailscale_and_funnel_args`** in `src/cli/mod.rs` test block:

  ```rust
  #[cfg(test)]
  mod cli_tests {
      use super::*;
      use clap::CommandFactory;

      #[test]
      fn run_command_has_tailscale_and_funnel_args() {
          let cmd = Cli::command();
          let run_sub = cmd.find_subcommand("run").expect("run subcommand");
          let args: Vec<&str> = run_sub
              .get_arguments()
              .map(|a| a.get_id().as_str())
              .collect();
          assert!(args.contains(&"tailscale"), "run should have --tailscale");
          assert!(args.contains(&"funnel"), "run should have --funnel");
          assert!(args.contains(&"h2c"), "run should have --h2c");
      }
  }
  ```

  Note: If a `mod cli_tests` or `mod tests` already exists in `src/cli/mod.rs`, add the function inside the existing module.

- [ ] **Run the new CLI test**

  ```bash
  cargo test run_command_has_tailscale_and_funnel_args
  ```

- [ ] **Commit**

  ```bash
  git add src/cli/mod.rs src/process.rs
  git commit -m "feat: wire --tailscale and --funnel flags into portal run"
  ```

---

## Task 10: Create `src/switcher.rs` with HTML injection

- [ ] **Create `src/switcher.rs`** with the full implementation:

  ```rust
  /// Build the HTML snippet to inject before </body> for the app-switcher UI.
  /// `current_slot` is the active slot. `slots` is the full list of routes for
  /// this hostname.
  pub fn build_switcher_html(
      hostname: &str,
      slots: &[crate::routes::Route],
      current_slot: u32,
  ) -> String {
      let cookie_key = format!("portless-slot-{}", hostname.replace('.', "-"));

      let buttons: String = slots
          .iter()
          .map(|r| {
              let label = r
                  .label
                  .as_deref()
                  .unwrap_or(&format!("slot-{}", r.slot));
              let active = if r.slot == current_slot {
                  "background:#5c6ac4;"
              } else {
                  "background:#333;"
              };
              format!(
                  r#"<button data-portless-slot="{slot}" style="border:none;color:#fff;padding:3px 8px;border-radius:4px;cursor:pointer;{active}">{label}</button>"#,
                  slot = r.slot,
                  label = label,
                  active = active,
              )
          })
          .collect::<Vec<_>>()
          .join("\n  ");

      format!(
          r#"
  <div id="__portless_switcher__" style="position:fixed;bottom:16px;right:16px;z-index:99999;background:#1a1a1a;color:#fff;border-radius:8px;padding:8px 12px;font:13px/1.4 monospace;box-shadow:0 4px 12px rgba(0,0,0,.4);display:flex;gap:8px;align-items:center">
    <span style="opacity:.5;margin-right:4px">portal</span>
    {buttons}
  </div>
  <script>
  (function(){{
    var key='{cookie_key}';
    document.querySelectorAll('[data-portless-slot]').forEach(function(b){{
      b.addEventListener('click',function(){{
        document.cookie=key+'='+b.dataset.portlessSlot+';path=/;max-age=86400';
        location.reload();
      }});
    }});
  }})();
  </script>
  "#,
          buttons = buttons,
          cookie_key = cookie_key,
      )
  }

  /// Inject the switcher HTML before </body> in an HTML body.
  /// Returns `None` if:
  /// - `content_type` does not start with `text/html`
  /// - `body` is over 4 MB
  /// - `slots.len() <= 1` (no switching needed)
  pub fn inject_switcher(
      body: &[u8],
      content_type: &str,
      hostname: &str,
      slots: &[crate::routes::Route],
      current_slot: u32,
  ) -> Option<Vec<u8>> {
      if slots.len() <= 1 {
          return None;
      }
      if !content_type.starts_with("text/html") {
          return None;
      }
      if body.len() > 4 * 1024 * 1024 {
          return None;
      }

      let html = build_switcher_html(hostname, slots, current_slot);
      let body_str = std::str::from_utf8(body).ok()?;

      // Find last </body> (case-insensitive)
      let lower = body_str.to_lowercase();
      if let Some(pos) = lower.rfind("</body>") {
          let mut result = body_str[..pos].as_bytes().to_vec();
          result.extend_from_slice(html.as_bytes());
          result.extend_from_slice(body_str[pos..].as_bytes());
          Some(result)
      } else {
          // No </body> tag — append to end
          let mut result = body.to_vec();
          result.extend_from_slice(html.as_bytes());
          Some(result)
      }
  }

  /// Parse the preferred slot number from the `Cookie` header value.
  /// Cookie key format: `portless-slot-<hostname-with-dots-as-dashes>=<N>`.
  pub fn read_slot_from_cookies(cookie_header: &str, hostname: &str) -> u32 {
      let key = format!("portless-slot-{}=", hostname.replace('.', "-"));
      cookie_header
          .split(';')
          .map(|s| s.trim())
          .find(|s| s.starts_with(&key))
          .and_then(|s| s[key.len()..].parse().ok())
          .unwrap_or(0)
  }

  #[cfg(test)]
  mod tests {
      use super::*;
      use crate::routes::{Route, RouteProtocol};

      fn make_slot(slot: u32, label: Option<&str>) -> Route {
          Route {
              hostname: "myapp.localhost".to_string(),
              port: 4000 + slot as u16,
              public_port: None,
              protocol: RouteProtocol::Http,
              pid: std::process::id(),
              owner_pid: std::process::id(),
              cwd: "/tmp".to_string(),
              created_at: chrono::Utc::now(),
              slot,
              label: label.map(String::from),
              tailscale_url: None,
              tailscale_https_port: None,
              tailscale_funnel: false,
          }
      }

      #[test]
      fn inject_inserts_before_body_close() {
          let slots = vec![make_slot(0, Some("main")), make_slot(1, Some("dev"))];
          let html = b"<html><body><p>hello</p></body></html>";
          let result = inject_switcher(html, "text/html", "myapp.localhost", &slots, 0).unwrap();
          let s = String::from_utf8(result).unwrap();
          assert!(s.contains("__portless_switcher__"));
          assert!(s.contains("</body></html>"));
          // Switcher must appear before </body>
          assert!(s.rfind("__portless_switcher__").unwrap() < s.rfind("</body>").unwrap());
      }

      #[test]
      fn inject_skips_non_html() {
          let slots = vec![make_slot(0, None), make_slot(1, None)];
          let result = inject_switcher(b"{}", "application/json", "x.localhost", &slots, 0);
          assert!(result.is_none());
      }

      #[test]
      fn inject_skips_single_slot() {
          let slots = vec![make_slot(0, None)];
          let result = inject_switcher(b"<html></html>", "text/html", "x.localhost", &slots, 0);
          assert!(result.is_none());
      }

      #[test]
      fn inject_appends_when_no_body_tag() {
          let slots = vec![make_slot(0, None), make_slot(1, None)];
          let result =
              inject_switcher(b"<p>no body tag</p>", "text/html", "x.localhost", &slots, 0)
                  .unwrap();
          let s = String::from_utf8(result).unwrap();
          assert!(s.contains("__portless_switcher__"));
          assert!(s.starts_with("<p>no body tag</p>"));
      }

      #[test]
      fn inject_skips_oversized_body() {
          let slots = vec![make_slot(0, None), make_slot(1, None)];
          let big = vec![b'x'; 5 * 1024 * 1024]; // 5 MB > 4 MB limit
          let result = inject_switcher(&big, "text/html", "x.localhost", &slots, 0);
          assert!(result.is_none());
      }

      #[test]
      fn active_slot_button_has_different_style() {
          let slots = vec![make_slot(0, Some("main")), make_slot(1, Some("dev"))];
          let html = build_switcher_html("myapp.localhost", &slots, 1);
          // Slot 1 is active — should have highlight colour #5c6ac4
          assert!(html.contains("#5c6ac4"), "active slot should have highlight colour");
          // Slot 0 is inactive — should have default colour #333
          assert!(html.contains("#333"), "inactive slot should have default colour");
      }

      #[test]
      fn read_slot_from_cookies_parses_correctly() {
          let cookie = "other=val; portless-slot-myapp-localhost=2; another=x";
          assert_eq!(read_slot_from_cookies(cookie, "myapp.localhost"), 2);
      }

      #[test]
      fn read_slot_from_cookies_returns_zero_when_absent() {
          assert_eq!(read_slot_from_cookies("other=val", "myapp.localhost"), 0);
      }

      #[test]
      fn read_slot_from_cookies_returns_zero_for_empty_header() {
          assert_eq!(read_slot_from_cookies("", "myapp.localhost"), 0);
      }
  }
  ```

- [ ] **Add `pub mod switcher;` to `src/lib.rs`**

  Append to `src/lib.rs`:
  ```
  pub mod switcher;
  ```

- [ ] **Run switcher tests**

  ```bash
  cargo test switcher::tests
  ```

- [ ] **Commit**

  ```bash
  git add src/switcher.rs src/lib.rs
  git commit -m "feat: add app-switcher HTML injection (src/switcher.rs)"
  ```

---

## Task 11: Wire multiplexed routing + switcher injection into proxy

- [ ] **Replace the route lookup block in `src/proxy.rs`**

  The current route lookup is at lines 355–377. Replace it with slot-aware lookup.

  The function `handle_https_request` currently resolves `hostname` and then does:
  ```rust
  let route = match routes.get(&hostname).or_else(|| { ... }) { ... };
  ```

  Replace this entire block (from `let route = match routes.get` through the closing `}` of the match, up to the `if is_websocket_upgrade` line) with:

  ```rust
  // Slot-aware route lookup
  let slots = {
      let direct = routes.list_slots(&hostname);
      if !direct.is_empty() {
          direct
      } else if wildcard {
          wildcard_parent(&hostname)
              .map(|parent| routes.list_slots(&parent))
              .unwrap_or_default()
      } else {
          vec![]
      }
  };

  if slots.is_empty() {
      return Ok(if accept_html {
          Response::builder()
              .status(StatusCode::NOT_FOUND)
              .header("content-type", "text/html")
              .body(full_body(crate::pages::page_404(&hostname)))
              .unwrap()
      } else {
          plain_error(
              StatusCode::NOT_FOUND,
              &format!("no route registered for {hostname}"),
          )
      });
  }

  let route = if slots.len() == 1 {
      slots[0].clone()
  } else {
      let cookie_header = req
          .headers()
          .get(http::header::COOKIE)
          .and_then(|v| v.to_str().ok())
          .unwrap_or("");
      let preferred = crate::switcher::read_slot_from_cookies(cookie_header, &hostname);
      let primary = slots[0].clone();
      slots.iter().find(|r| r.slot == preferred).cloned().unwrap_or(primary)
  };

  // Keep a copy of all slots for switcher injection (before route is consumed)
  let slots_for_injection = slots;
  ```

  Note: the variable `slots_for_injection` is now available for use later in the function when building the response.

- [ ] **Add switcher injection after response body collection in `src/proxy.rs`**

  In the `Ok(upstream_resp) =>` branch, after the `should_stream` check, there are two paths: the streaming path (`TeeBody`) and the collected path. The switcher injection only applies to the collected path (since injection requires buffering the full body, which the streaming path intentionally avoids).

  In the `else` branch (small response, collected synchronously), after:
  ```rust
  let resp_bytes = match resp_body.collect().await {
      Ok(c) => c.to_bytes(),
      Err(_) => bytes::Bytes::new(),
  };
  ```

  Add switcher injection:
  ```rust
  // Inject app-switcher for multi-slot hostnames into text/html responses
  let resp_bytes = if slots_for_injection.len() > 1 {
      let ct = resp_parts
          .headers
          .get(http::header::CONTENT_TYPE)
          .and_then(|v| v.to_str().ok())
          .unwrap_or("");
      crate::switcher::inject_switcher(
          &resp_bytes,
          ct,
          &hostname,
          &slots_for_injection,
          route.slot,
      )
      .map(|injected| bytes::Bytes::from(injected))
      .unwrap_or(resp_bytes)
  } else {
      resp_bytes
  };
  ```

  Place this block immediately before the `if let Some(sender) = &inspector {` block in the `else` branch.

- [ ] **Add test `multi_slot_cookie_dispatch`** in the `#[cfg(test)]` block of `src/proxy.rs`:

  ```rust
  #[test]
  fn switcher_read_slot_from_cookies_integration() {
      // Verify that the cookie parsing used by the proxy returns the correct slot
      let cookie = "portless-slot-myapp-localhost=1";
      let slot = crate::switcher::read_slot_from_cookies(cookie, "myapp.localhost");
      assert_eq!(slot, 1, "should select slot 1 from cookie");

      // Missing cookie should fall back to 0
      let slot_default = crate::switcher::read_slot_from_cookies("", "myapp.localhost");
      assert_eq!(slot_default, 0, "should default to slot 0 when no cookie");
  }
  ```

- [ ] **Verify the wildcard fallback still compiles**

  The old wildcard path used `routes.get(&parent)`. The new slot-aware path uses `routes.list_slots(&parent)`. Both are defined on `StateStore`. Confirm:

  ```bash
  cargo build 2>&1 | grep -E "^error"
  ```

- [ ] **Run new proxy test**

  ```bash
  cargo test switcher_read_slot_from_cookies_integration
  ```

- [ ] **Commit**

  ```bash
  git add src/proxy.rs
  git commit -m "feat: wire multiplexed routing and app-switcher injection into proxy"
  ```

---

## Task 12: Wire `--slot` and `--label` flags into `portal run`

- [ ] **Add `--slot` and `--label` args to `CliCommand::Run` in `src/cli/mod.rs`**

  Inside the `Run { .. }` variant, add after the `funnel` field:

  ```rust
  /// Register as a specific slot number (default: auto-assign next available)
  #[arg(long)]
  slot: Option<u32>,
  /// Label shown in the app-switcher UI (default: slot-N)
  #[arg(long)]
  label: Option<String>,
  ```

- [ ] **Update the `CliCommand::Run` match arm** to unpack and thread the new fields:

  ```rust
  CliCommand::Run {
      hostname,
      port,
      quiet,
      tcp,
      force,
      lan,
      ip,
      h2c,
      tailscale,
      funnel,
      slot,
      label,
      args,
  } => {
      let cwd = std::env::current_dir()?;
      let mut config = crate::config::Config::load(&cwd)?;
      if lan { config.proxy.lan = true; }
      if let Some(addr) = ip { config.proxy.lan_ip = Some(addr); }
      if h2c { config.proxy.h2c = true; }
      let use_tailscale = tailscale || funnel;
      let resolved_args = crate::detect::resolve_run_args(&cwd, args);
      do_run(
          cwd,
          config,
          resolved_args,
          hostname,
          port,
          false,
          quiet,
          tcp,
          force,
          use_tailscale,
          funnel,
          slot,
          label,
      )
      .await?;
  }
  ```

- [ ] **Update `do_run` signature** to add `slot: Option<u32>` and `label: Option<String>`:

  ```rust
  async fn do_run(
      cwd: std::path::PathBuf,
      config: crate::config::Config,
      args: Vec<String>,
      hostname_override: Option<String>,
      port_override: Option<u16>,
      use_full_registry: bool,
      quiet: bool,
      tcp: bool,
      force: bool,
      tailscale: bool,
      funnel: bool,
      slot: Option<u32>,
      label: Option<String>,
  ) -> Result<()> {
  ```

  Update the `CliCommand::Start` call to `do_run`:

  ```rust
  do_run(
      cwd,
      config,
      args,
      hostname_override,
      None,
      true,
      quiet,
      false,
      false,
      false,  // tailscale
      false,  // funnel
      None,   // slot
      None,   // label
  )
  .await?;
  ```

- [ ] **Thread `slot` and `label` into the `RegisterRoute` IPC call** inside `do_run`

  In the `RegisterRoute` IPC send (around line 984), update to:

  ```rust
  let _ = write_frame(
      &mut stream,
      &Command::RegisterRoute {
          hostname: hostname.clone(),
          port,
          public_port,
          protocol: if tcp {
              crate::routes::RouteProtocol::Tcp
          } else {
              crate::routes::RouteProtocol::Http
          },
          pid: child_pid,
          cwd: cwd.to_string_lossy().to_string(),
          slot,
          label: label.clone(),
      },
  )
  .await;
  ```

- [ ] **Add test `run_command_has_slot_and_label_args`** in the `cli_tests` module of `src/cli/mod.rs`:

  ```rust
  #[test]
  fn run_command_has_slot_and_label_args() {
      let cmd = Cli::command();
      let run_sub = cmd.find_subcommand("run").expect("run subcommand");
      let args: Vec<&str> = run_sub
          .get_arguments()
          .map(|a| a.get_id().as_str())
          .collect();
      assert!(args.contains(&"slot"), "run should have --slot");
      assert!(args.contains(&"label"), "run should have --label");
  }
  ```

- [ ] **Run new CLI test**

  ```bash
  cargo test run_command_has_slot_and_label_args
  ```

- [ ] **Commit**

  ```bash
  git add src/cli/mod.rs
  git commit -m "feat: add --slot and --label flags to portal run"
  ```

---

## Task 13: End-to-end build check

- [ ] **Run full build**

  ```bash
  cargo build 2>&1
  ```

  Expected: zero errors. Address any compile errors before proceeding.

- [ ] **Run all tests**

  ```bash
  cargo test 2>&1
  ```

  Expected: all previously passing tests still pass, plus all new tests added in Tasks 1–12.

- [ ] **If compile errors remain, fix them:**

  Common issues to watch for:
  - Any `Route { .. }` struct literal that does not include the five new fields (`slot`, `label`, `tailscale_url`, `tailscale_https_port`, `tailscale_funnel`). Search with: `grep -rn "Route {" src/` and add missing fields.
  - `StateStore::get` callers that now receive `Option<Route>` from a `Vec<Route>` (the API is unchanged — `get` still returns `Option<Route>`).
  - `handle_https_request` call sites that need the new `h2c: bool` parameter (pass `false`).
  - The `do_run` call in monorepo `run_monorepo` function — check if it calls `do_run` directly and update if so (from reading the code it calls its own inline logic, not `do_run`, so no change needed).
  - The `Error::Other` variant in `src/error.rs` — if it was not added in Task 8, add it now.

- [ ] **Commit (only if fixes were needed)**

  ```bash
  git add -p  # stage only relevant fixes
  git commit -m "chore: advanced proxy feature parity build verification"
  ```

---

## Task 14: Push branch and open PR

- [ ] **Push the branch to origin**

  ```bash
  git push -u origin feature/advanced-proxy
  ```

- [ ] **Open PR targeting `feature/parity-fixes`**

  ```bash
  gh pr create \
    --base feature/parity-fixes \
    --title "feat: advanced proxy — HTTP/2, RFC 8441, Tailscale, multiplexed routing" \
    --body "$(cat <<'EOF'
  ## Summary

  Implements portal v0.3.0 advanced proxy features per `docs/superpowers/specs/2026-05-06-advanced-proxy-design.md`:

  - **HTTP/2 auto-negotiation** — `serve_https` now uses `hyper_util::server::conn::auto::Builder` with ALPN `h2`/`http/1.1`; `is_http_method_prefix` extended to detect `PRI` (H2 preface)
  - **RFC 8441 WebSocket over HTTP/2** — Extended CONNECT (`CONNECT + protocol: websocket`) dispatched to existing `handle_websocket` handler
  - **`--h2c` flag** — HTTP/2 cleartext upstream client for gRPC backends; `PORTLESS_H2C` env var; `h2c: bool` in `ProxyConfig`
  - **Tailscale integration** — `src/tailscale.rs` wraps `tailscale serve`/`funnel` CLI; `--tailscale` and `--funnel` flags on `portal run`; `UpdateRoute` IPC command persists Tailscale URL in daemon
  - **Multiplexed hostname routing** — `StateStore` map changed to `DashMap<String, Vec<Route>>`; slot auto-assignment; cookie-based slot dispatch in proxy
  - **App-switcher HTML injection** — `src/switcher.rs` injects floating UI before `</body>` for multi-slot hostnames; `--slot` and `--label` flags on `portal run`

  ## Test plan

  - [ ] `cargo test` passes with zero failures
  - [ ] `portal run --tailscale npm start` prints Tailscale URL in banner (requires `tailscale` CLI and active session)
  - [ ] `portal run --funnel npm start` prints Funnel URL and emits error if Funnel not enabled
  - [ ] Chrome DevTools confirms H2 protocol is used when connecting to a portal HTTPS URL after this change
  - [ ] Vite/Next.js HMR WebSocket over H2 connects successfully (RFC 8441)
  - [ ] Running `portal run --slot 1 --label dev npm start` for an existing hostname creates a second slot; browser shows switcher UI
  - [ ] Clicking switcher button sets cookie and reloads to the alternate slot's port

  🤖 Generated with [Claude Code](https://claude.com/claude-code)
  EOF
  )"
  ```

---

## Self-Review Checklist

### Spec coverage

| Spec requirement | Task |
|---|---|
| HTTP/2 via `auto::Builder` | Task 5 |
| ALPN `h2`/`http/1.1` in rustls config | Task 5 |
| `PRI` prefix in `is_http_method_prefix` | Task 5 |
| RFC 8441 `is_h2_websocket_connect` | Task 6 |
| `--h2c` flag + `PORTLESS_H2C` env var | Task 7 |
| H2C upstream client (`http2_only`) | Task 7 |
| `src/tailscale.rs` with 5 functions | Task 8 |
| `Error::Other` variant | Task 8 |
| `--tailscale` / `--funnel` CLI flags | Task 9 |
| `PORTLESS_TAILSCALE_URL` env const | Task 9 |
| `UpdateRoute` IPC command | Tasks 3–4 |
| Tailscale `unregister` on exit | Task 9 |
| `slot` / `label` fields on `Route` | Task 1 |
| `tailscale_url` / `tailscale_https_port` / `tailscale_funnel` fields | Task 1 |
| `DashMap<String, Vec<Route>>` | Task 2 |
| `get_slot` / `list_slots` / `remove_slot` | Task 2 |
| Auto-assign slot on insert | Task 2 |
| `persist_locked` flattens vecs | Task 2 |
| `sync_hosts_locked` deduplicates | Task 2 |
| `src/switcher.rs` HTML injection | Task 10 |
| Cookie-based slot dispatch in proxy | Task 11 |
| `--slot` / `--label` CLI flags | Task 12 |
| Build + test pass | Task 13 |
| PR against `feature/parity-fixes` | Task 14 |

### Placeholder scan

No "implement accordingly", "similar to above", "TODO", or "..." placeholders appear in this plan. Every code block is complete.

### Type consistency

- `Route::slot` is `u32` throughout (StateStore, proto, CLI, switcher).
- `Route::label` is `Option<String>` throughout.
- `StateStore::map` is `Arc<DashMap<String, Vec<Route>>>` throughout.
- `Command::RegisterRoute::slot` is `Option<u32>` (None = auto-assign); `StateStore::insert` converts `None → 0` before auto-assign logic.
- `is_h2_websocket_connect` is generic over `B: Body` consistent with `is_websocket_upgrade`.
- `Error::Other(String)` added to `error.rs` for use by `tailscale.rs`.
- `handle_https_request` fifth parameter `h2c: bool` — all call sites pass `false` until per-route h2c threading is implemented.
