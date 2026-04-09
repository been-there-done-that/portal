# Portal Dashboard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the inspector-only UI with a unified dashboard showing service cards (live metrics, actions) above the request inspector — all on a single page.

**Architecture:** Extend the Axum backend with `/api/routes` and `/api/routes/{hostname}/stop` endpoints by threading `RouteManager` into the inspector's `AppState`. Build new Svelte components (`ServiceCards`, `FilterBar`, `StatusBar`) and recompose the page layout, removing the sidebar. The existing request inspector (feed + detail + SSE) stays intact beneath the service cards.

**Tech Stack:** Rust (Axum), SvelteKit 5, Tailwind v4, shadcn-svelte, TypeScript

---

## File Map

### Backend (Rust)
| File | Change |
|---|---|
| `src/inspector/server.rs` | Add `RouteManager` to `AppState`; add `/api/routes` and `/api/routes/{hostname}/stop` endpoints |
| `src/inspector/mod.rs` | `Inspector::start()` takes `RouteManager` parameter |
| `src/daemon/mod.rs` | Pass `manager` to `Inspector::start()` |

### Frontend (Svelte/TypeScript)
| File | Change |
|---|---|
| `ui/src/lib/api.ts` | Add `RouteInfo`, `DaemonInfo`, `RoutesResponse` types + `fetchRoutes()`, `stopRoute()` functions |
| `ui/src/lib/stores/routes.svelte.ts` | New — routes state, daemon info, per-hostname request counters |
| `ui/src/lib/components/ServiceCard.svelte` | New — individual service card with stats, sparkline, actions |
| `ui/src/lib/components/ServiceCards.svelte` | New — horizontal scrollable card strip |
| `ui/src/lib/components/FilterBar.svelte` | New — method toggles, error filter, search |
| `ui/src/lib/components/StatusBar.svelte` | New — bottom bar with daemon info |
| `ui/src/lib/components/RequestFeed.svelte` | Enhance — column headers, hostname column, size column, full-width |
| `ui/src/lib/components/RequestDetail.svelte` | Convert to slide-in overlay panel |
| `ui/src/routes/+page.svelte` | Recompose layout: cards → filter → feed → status bar. Remove sidebar. |
| `ui/src/lib/components/Sidebar.svelte` | Remove (functionality moves to ServiceCards + FilterBar) |

---

## Task 1: Backend — Thread `RouteManager` into inspector + `/api/routes` endpoint

**Files:**
- Modify: `src/inspector/server.rs`
- Modify: `src/inspector/mod.rs`
- Modify: `src/daemon/mod.rs`

### Background

The inspector's `AppState` currently has `db: Db` and `sse_tx: SseTx`. We need to add `RouteManager` so the new API endpoints can read routes and stop services. `RouteManager` is `Clone` (Arc-backed), so it can be shared in Axum state.

- [ ] **Step 1: Add `RouteManager` to `AppState` in `src/inspector/server.rs`**

Change the `AppState` struct:

```rust
use crate::route_manager::RouteManager;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub sse_tx: SseTx,
    pub routes: RouteManager,
}
```

Add the new route to the router:

```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/requests",
            get(get_requests).delete(delete_all_requests),
        )
        .route("/api/requests/{id}", delete(delete_one_request))
        .route("/api/routes", get(get_routes))
        .route("/api/routes/{hostname}/stop", axum::routing::post(stop_route))
        .route("/api/stream", get(sse_handler))
        .fallback(static_handler)
        .with_state(state)
}
```

Add the handler functions:

```rust
#[derive(Serialize)]
struct RoutesResponse {
    routes: Vec<RouteResponse>,
    daemon: DaemonResponse,
}

#[derive(Serialize)]
struct RouteResponse {
    hostname: String,
    port: u16,
    pid: u32,
    protocol: String,
    public_port: Option<u16>,
    cwd: String,
    created_at: String,
}

#[derive(Serialize)]
struct DaemonResponse {
    version: String,
    pid: u32,
    uptime_secs: u64,
}

static START_TIME: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

pub fn set_start_time() {
    START_TIME.get_or_init(std::time::Instant::now);
}

async fn get_routes(State(state): State<AppState>) -> impl IntoResponse {
    let routes: Vec<RouteResponse> = state
        .routes
        .list()
        .into_iter()
        .filter(|r| r.hostname != "_.localhost")
        .map(|r| RouteResponse {
            hostname: r.hostname,
            port: r.port,
            pid: r.pid,
            protocol: match r.protocol {
                crate::routes::RouteProtocol::Http => "http".to_string(),
                crate::routes::RouteProtocol::Tcp => "tcp".to_string(),
            },
            public_port: r.public_port,
            cwd: r.cwd,
            created_at: r.created_at.to_rfc3339(),
        })
        .collect();

    let uptime = START_TIME
        .get()
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);

    Json(RoutesResponse {
        routes,
        daemon: DaemonResponse {
            version: env!("CARGO_PKG_VERSION").to_string(),
            pid: std::process::id(),
            uptime_secs: uptime,
        },
    })
}

async fn stop_route(
    State(state): State<AppState>,
    Path(hostname): Path<String>,
) -> impl IntoResponse {
    match state.routes.get(&hostname) {
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "ok": false, "error": "route not found" }))),
        Some(route) => {
            // Kill process if managed (pid != 0)
            #[cfg(unix)]
            if route.pid != 0 {
                use nix::sys::signal::{killpg, Signal};
                use nix::unistd::Pid;
                killpg(Pid::from_raw(route.pid as i32), Signal::SIGTERM).ok();
            }
            if let Err(e) = state.routes.remove(&hostname).await {
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({ "ok": false, "error": e.to_string() })));
            }
            (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
        }
    }
}
```

- [ ] **Step 2: Update `Inspector::start()` in `src/inspector/mod.rs`**

Change the signature to accept `RouteManager`:

```rust
pub async fn start(db_path: PathBuf, routes: crate::route_manager::RouteManager) -> crate::error::Result<Inspector> {
```

Update `AppState` construction:

```rust
let state = AppState { db, sse_tx, routes };
```

Add the start_time call at the top:

```rust
crate::inspector::server::set_start_time();
```

- [ ] **Step 3: Update `daemon/mod.rs` to pass `manager`**

Find the `Inspector::start()` call (around line 198-200). Change:

```rust
crate::inspector::Inspector::start(state_dir.join("inspector.db")).await
```

To:

```rust
crate::inspector::Inspector::start(state_dir.join("inspector.db"), manager.clone()).await
```

- [ ] **Step 4: Build and run tests**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo build 2>&1 | grep "^error" | head -10
cargo test 2>&1 | grep -E "^test result|FAILED"
```

- [ ] **Step 5: Commit**

```bash
git add src/inspector/server.rs src/inspector/mod.rs src/daemon/mod.rs
git commit -m "feat(inspector): add /api/routes and /api/routes/{hostname}/stop endpoints"
```

---

## Task 2: Frontend — Routes API + Store

**Files:**
- Modify: `ui/src/lib/api.ts`
- Create: `ui/src/lib/stores/routes.svelte.ts`

### Background

Add TypeScript types and fetch functions for the new routes API. Create a Svelte 5 runes-based store for routes state, daemon info, and per-hostname request counters (incremented from SSE events).

- [ ] **Step 1: Add types and API functions to `ui/src/lib/api.ts`**

Append to the existing file:

```typescript
// ── Routes API ───────────────────────────────────────────────────────────────

export interface RouteInfo {
  hostname: string;
  port: number;
  pid: number;
  protocol: 'http' | 'tcp';
  public_port: number | null;
  cwd: string;
  created_at: string;
}

export interface DaemonInfo {
  version: string;
  pid: number;
  uptime_secs: number;
}

export interface RoutesResponse {
  routes: RouteInfo[];
  daemon: DaemonInfo;
}

export async function fetchRoutes(): Promise<RoutesResponse> {
  const res = await fetch('/api/routes');
  return res.json();
}

export async function stopRoute(hostname: string): Promise<{ ok: boolean; error?: string }> {
  const res = await fetch(`/api/routes/${encodeURIComponent(hostname)}/stop`, { method: 'POST' });
  return res.json();
}
```

- [ ] **Step 2: Create `ui/src/lib/stores/routes.svelte.ts`**

```typescript
import { fetchRoutes, type RouteInfo, type DaemonInfo } from '$lib/api.js';

const _s = $state({
  routes: [] as RouteInfo[],
  daemon: null as DaemonInfo | null,
  selectedHostname: null as string | null,
  /** Per-hostname request count, incremented from SSE events */
  requestCounts: {} as Record<string, number>,
  /** Per-hostname latency samples (last 20) for sparklines */
  latencySamples: {} as Record<string, number[]>,
  /** Per-hostname error counts */
  errorCounts: {} as Record<string, number>,
});

export const routeStore = {
  get routes(): RouteInfo[] { return _s.routes; },
  get daemon(): DaemonInfo | null { return _s.daemon; },
  get selectedHostname(): string | null { return _s.selectedHostname; },
  get requestCounts(): Record<string, number> { return _s.requestCounts; },
  get latencySamples(): Record<string, number[]> { return _s.latencySamples; },
  get errorCounts(): Record<string, number> { return _s.errorCounts; },
};

export function setSelectedHostname(hostname: string | null) {
  _s.selectedHostname = hostname;
}

export async function loadRoutes() {
  const res = await fetchRoutes();
  _s.routes = res.routes;
  _s.daemon = res.daemon;
}

/** Called from SSE handler to track per-hostname metrics */
export function trackRequest(hostname: string, durationMs: number, status: number) {
  // Increment count
  _s.requestCounts = {
    ..._s.requestCounts,
    [hostname]: (_s.requestCounts[hostname] ?? 0) + 1,
  };

  // Track latency (last 20 samples)
  const samples = [...(_s.latencySamples[hostname] ?? []), durationMs].slice(-20);
  _s.latencySamples = { ..._s.latencySamples, [hostname]: samples };

  // Track errors
  if (status >= 400) {
    _s.errorCounts = {
      ..._s.errorCounts,
      [hostname]: (_s.errorCounts[hostname] ?? 0) + 1,
    };
  }
}
```

- [ ] **Step 3: Run type check**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/ui && bun run check 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/api.ts ui/src/lib/stores/routes.svelte.ts
git commit -m "feat(ui): add routes API types, fetch functions, and routes store"
```

---

## Task 3: Frontend — ServiceCard + ServiceCards Components

**Files:**
- Create: `ui/src/lib/components/ServiceCard.svelte`
- Create: `ui/src/lib/components/ServiceCards.svelte`

### Background

`ServiceCard` renders one service: name, port/pid meta, type badge (HTTP/ALIAS/TCP), request count, avg latency, error count, sparkline, and hover actions (open, copy, stop/remove). When clicked, it sets the hostname filter.

`ServiceCards` renders a horizontal scrollable row of `ServiceCard` instances plus an empty "+" card.

- [ ] **Step 1: Create `ServiceCard.svelte`**

Create `ui/src/lib/components/ServiceCard.svelte`. The component receives:
- `route: RouteInfo` — the route data
- `requestCount: number` — from `routeStore.requestCounts`
- `latencySamples: number[]` — for sparkline
- `errorCount: number` — error badge
- `active: boolean` — whether this card is the active filter
- `onclick: () => void` — click handler to toggle filter

Build the card with:
- Top row: service name (hostname minus `.localhost`) + type badge
- Meta line: `.localhost · :port · pid N` or `alias`
- Stats row: request count, avg latency (computed from samples), error count
- Sparkline: 20 bars using the latency samples array, rendered as inline `div` elements
- Actions row (shown on hover via CSS `group-hover`): buttons for `open ↗`, `copy url`, `stop`/`remove`
- For alias routes (pid=0): show connection hint instead of stats

Use shadcn `Badge` for the type badge. Use Tailwind for all styling. `border-radius: rounded-md` (6px). Active state: `border-blue-500 bg-blue-950/20`.

The stop button calls `stopRoute(hostname)` from the API, then `loadRoutes()` to refresh.

- [ ] **Step 2: Create `ServiceCards.svelte`**

Create `ui/src/lib/components/ServiceCards.svelte`:

```svelte
<script lang="ts">
  import ServiceCard from './ServiceCard.svelte';
  import { routeStore, setSelectedHostname } from '$lib/stores/routes.svelte.js';
  import { setFilterHostname } from '$lib/stores/requests.svelte.js';

  function toggleFilter(hostname: string) {
    if (routeStore.selectedHostname === hostname) {
      setSelectedHostname(null);
      setFilterHostname(null);
    } else {
      setSelectedHostname(hostname);
      setFilterHostname(hostname);
    }
  }
</script>

<div class="border-b border-border px-5 py-3">
  <div class="mb-2 flex items-center justify-between">
    <h2 class="text-xs font-medium text-muted-foreground">Services</h2>
    <span class="text-[10px] text-muted-foreground/50">click to filter · hover for actions</span>
  </div>
  <div class="flex gap-2 overflow-x-auto pb-1">
    {#each routeStore.routes as route (route.hostname)}
      <ServiceCard
        {route}
        requestCount={routeStore.requestCounts[route.hostname] ?? 0}
        latencySamples={routeStore.latencySamples[route.hostname] ?? []}
        errorCount={routeStore.errorCounts[route.hostname] ?? 0}
        active={routeStore.selectedHostname === route.hostname}
        onclick={() => toggleFilter(route.hostname)}
      />
    {/each}
    <!-- Empty add card -->
    <div class="flex min-w-[120px] items-center justify-center rounded-md border border-dashed border-border">
      <div class="text-center py-4 px-3">
        <div class="text-lg text-muted-foreground/30">+</div>
        <div class="text-[9px] text-muted-foreground/40 font-mono">portal run<br>portal alias</div>
      </div>
    </div>
  </div>
</div>
```

- [ ] **Step 3: Build**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/ui && bun run check 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/components/ServiceCard.svelte ui/src/lib/components/ServiceCards.svelte
git commit -m "feat(ui): add ServiceCard and ServiceCards components"
```

---

## Task 4: Frontend — FilterBar + StatusBar Components

**Files:**
- Create: `ui/src/lib/components/FilterBar.svelte`
- Create: `ui/src/lib/components/StatusBar.svelte`

### Background

`FilterBar` replaces the filter functionality from the removed Sidebar. Method toggles as pill buttons, error filter, search input with `/` to focus. All filters are wired to the existing `requests.svelte.ts` store.

`StatusBar` is a thin bar at the bottom showing portal branding, version, daemon mode, ports, pid, tld, and filtered request count.

- [ ] **Step 1: Create `FilterBar.svelte`**

Create `ui/src/lib/components/FilterBar.svelte`:

Uses `Badge` from shadcn for method pills. Wires to `setFilterMethods`, `setFilterErrors` from requests store. Search input filters by path (add `filterSearch` to the requests store or do client-side filtering in the feed).

The component has:
- "FILTER" label
- Method pills: ALL, GET, POST, PUT, PATCH, DELETE — clicking toggles
- Separator (vertical line)
- Error pill: `4xx/5xx` — red when active
- Search input right-aligned with placeholder `/ search path...`
- Clear history button (existing from sidebar)

- [ ] **Step 2: Create `StatusBar.svelte`**

Create `ui/src/lib/components/StatusBar.svelte`:

```svelte
<script lang="ts">
  import { routeStore } from '$lib/stores/routes.svelte.js';
  import { store } from '$lib/stores/requests.svelte.js';

  function formatUptime(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }
</script>

<div class="flex shrink-0 items-center justify-between border-t border-border bg-card px-5 py-1 font-mono text-[10px] text-muted-foreground/60">
  <div class="flex items-center gap-2">
    <span class="font-semibold text-foreground/60">portal</span>
    {#if routeStore.daemon}
      <span>v{routeStore.daemon.version}</span>
      <span>·</span>
      <span>:{routeStore.daemon.pid}</span>
      <span>·</span>
      <span>uptime {formatUptime(routeStore.daemon.uptime_secs)}</span>
    {/if}
  </div>
  <span>
    {store.filtered.length} requests
    {#if store.filterHostname}
      (filtered to {store.filterHostname})
    {/if}
  </span>
</div>
```

- [ ] **Step 3: Build**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/ui && bun run check 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/components/FilterBar.svelte ui/src/lib/components/StatusBar.svelte
git commit -m "feat(ui): add FilterBar and StatusBar components"
```

---

## Task 5: Frontend — Enhance RequestFeed + Convert RequestDetail to Overlay

**Files:**
- Modify: `ui/src/lib/components/RequestFeed.svelte`
- Modify: `ui/src/lib/components/RequestDetail.svelte`

### Background

`RequestFeed` currently renders as a 300px-wide sidebar column. Convert it to a full-width feed with column headers: time, method, status, service, path, duration, size. Each row is a grid.

`RequestDetail` currently renders as a third column. Convert it to a fixed-position slide-in panel from the right, dismissible with Escape or close button.

- [ ] **Step 1: Rewrite `RequestFeed.svelte`**

Replace the current component with a full-width table-style feed. Key changes:
- Remove the fixed 300px width — it now takes full width from the parent
- Add a column header row (sticky)
- Each request row uses CSS grid with columns: time(60px), method(45px), status(50px), hostname(140px), path(1fr), duration(50px), size(55px)
- Add hostname column using `req.hostname`
- Status codes are color-coded: 2xx green, 3xx blue, 4xx orange, 5xx red
- Method colors: GET green, POST yellow, PUT blue, DELETE red
- Selected row: blue left border + darker background

- [ ] **Step 2: Convert `RequestDetail.svelte` to slide-in overlay**

Wrap the existing detail content in a fixed-position container:
- Position: `fixed right-0 top-0 bottom-0 w-[420px]`
- Background with border-left and shadow
- Close button (`✕ esc`) in header
- Add `keydown` handler on window: Escape closes the panel
- Show/hide based on `store.selectedId !== null`

- [ ] **Step 3: Build**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/ui && bun run check 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/components/RequestFeed.svelte ui/src/lib/components/RequestDetail.svelte
git commit -m "feat(ui): enhance RequestFeed with columns; convert RequestDetail to overlay"
```

---

## Task 6: Frontend — Recompose Page Layout

**Files:**
- Modify: `ui/src/routes/+page.svelte`
- Delete: `ui/src/lib/components/Sidebar.svelte`

### Background

Replace the current three-column layout (`Sidebar | RequestFeed | RequestDetail`) with the new vertical layout: `ServiceCards → FilterBar → RequestFeed → StatusBar`, with `RequestDetail` as a slide-in overlay.

- [ ] **Step 1: Rewrite `+page.svelte`**

```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import ServiceCards from '$lib/components/ServiceCards.svelte';
  import FilterBar from '$lib/components/FilterBar.svelte';
  import RequestFeed from '$lib/components/RequestFeed.svelte';
  import RequestDetail from '$lib/components/RequestDetail.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
  import { loadHistory, prependRequest } from '$lib/stores/requests.svelte.js';
  import { loadRoutes, trackRequest } from '$lib/stores/routes.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  onMount(() => {
    loadHistory();
    loadRoutes();

    // Refresh routes every 5 seconds
    const routeInterval = setInterval(loadRoutes, 5000);

    // Connect SSE for live request updates
    const es = new EventSource('/api/stream');
    es.addEventListener('request', (e: MessageEvent) => {
      const meta: RequestMeta = JSON.parse(e.data);
      prependRequest(meta);
      trackRequest(meta.hostname, meta.duration_ms, meta.status);
    });
    es.onerror = () => {};

    return () => {
      clearInterval(routeInterval);
      es.close();
    };
  });
</script>

<svelte:head>
  <title>Portal</title>
</svelte:head>

<div class="flex h-full flex-col bg-background">
  <ServiceCards />
  <FilterBar />
  <RequestFeed />
  <StatusBar />
  <RequestDetail />
</div>
```

- [ ] **Step 2: Delete `Sidebar.svelte`**

```bash
rm ui/src/lib/components/Sidebar.svelte
```

- [ ] **Step 3: Build the frontend**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless/ui && bun run build 2>&1 | tail -5
```

- [ ] **Step 4: Build the Rust binary (embeds the new UI)**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo build 2>&1 | tail -5
```

- [ ] **Step 5: Run all Rust tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

- [ ] **Step 6: Commit**

```bash
git add -A ui/src/ src/
git commit -m "feat(ui): recompose dashboard layout — service cards, filter bar, status bar"
```

---

## Self-Review

**Spec coverage:**

- ✅ Service cards row with stats, sparklines, actions — Task 3
- ✅ Card types: HTTP (green), ALIAS (indigo), TCP (amber) — Task 3
- ✅ Click card to filter inspector — Task 3 (toggleFilter)
- ✅ Hover actions: open, copy, stop/remove — Task 3
- ✅ Filter bar: method toggles, error filter, search — Task 4
- ✅ Request feed: columns with hostname, size — Task 5
- ✅ Detail panel: slide-in overlay — Task 5
- ✅ Status bar: branding, version, daemon info, request count — Task 4
- ✅ No top bar — branding in status bar — Task 4/6
- ✅ `/api/routes` endpoint — Task 1
- ✅ `/api/routes/{hostname}/stop` endpoint — Task 1
- ✅ `RouteManager` in `AppState` — Task 1
- ✅ Routes store with per-hostname counters — Task 2
- ✅ SSE feeds trackRequest for live metrics — Task 6
- ✅ Sidebar removed — Task 6
- ✅ Rounded corners (6px cards, 4px badges) — Task 3

**Restart endpoint:** The spec mentioned restart, but portal doesn't store the original start command in the route — we can't restart from the dashboard. Omitted intentionally. Can be added later when routes store their start command.

**No placeholders found.**

**Type consistency:** `RouteInfo` matches `RouteResponse` from the Rust backend. `DaemonInfo` matches `DaemonResponse`. `routeStore` properties match the Svelte component props.
