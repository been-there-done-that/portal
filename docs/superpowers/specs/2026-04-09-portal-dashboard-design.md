# Portal Dashboard Design

**Goal:** Replace the inspector-only UI at `_.localhost` with a unified dashboard that shows service cards with live metrics above the existing request inspector, with filtering, actions, and a detail panel.

**Architecture:** Extend the existing SvelteKit frontend at `ui/`. Add new Svelte components for the dashboard layout. Add two new API endpoints to the Axum backend (`/api/routes`, `/api/routes/{hostname}/stop`). Service cards consume both the routes API (for service list) and the existing SSE stream (for live request counts). No tab navigation — single page, everything visible.

---

## Layout (top to bottom)

### 1. Service Cards Row

Horizontal scrollable strip of cards, one per active route. Each card shows:

- **Service name** (hostname without `.localhost`)
- **Meta line**: `.localhost · :port · pid <N>` (or `alias` for pid=0)
- **Type badge**: `HTTP` (green), `ALIAS` (indigo), `TCP` (amber) — top-right corner
- **Stats**: request count, avg latency (ms), error count (4xx/5xx)
- **Sparkline**: tiny bar chart of recent traffic (last 20 data points from SSE)
- **Actions** (visible on hover/active): `open ↗`, `copy url`, `restart`, `stop` for managed services; `copy cmd`, `remove` for aliases
- **Active state**: clicking a card filters the inspector feed below to that service (blue left border)
- **Empty card**: dashed `+` card at the end with `portal run / portal alias` hint

Alias cards (pid=0) show a connection command hint instead of request metrics (e.g. `psql -h name.localhost -p 443 sslmode=require`).

Cards have `border-radius: 6px`. Badges have `border-radius: 4px`.

### 2. Filter Bar

Below service cards. Contains:
- Method toggles: `ALL`, `GET`, `POST`, `PUT`, `PATCH`, `DELETE` — pill buttons, toggle on/off
- Error filter: `4xx/5xx` button (red when active)
- Search input: `/` to focus, filters by path or hostname
- Separator between method filters and error filter

Clicking a service card sets the hostname filter automatically. Clicking "ALL" or deselecting the card clears it.

### 3. Request Feed

Existing inspector feed, enhanced:
- **Columns**: time, method (color-coded), status, service (hostname), path, duration, size
- **Column header row** with uppercase labels
- Click a row to open the detail panel
- Selected row has blue left border + darker background
- Live updates via existing SSE (`/api/stream`)
- Respects all active filters (service card selection + method toggles + search)

### 4. Detail Panel

Slide-in panel from the right when a request is selected:
- **Header**: method + path, close button (`✕ esc`)
- **Tabs**: Headers, Request body, Response body, Timing
- **Content**: key-value pairs for headers, formatted JSON for bodies
- Dismiss with Escape key or close button

### 5. Bottom Status Bar

Replaces the removed top bar. Single thin strip:
- **Left**: `portal v{version} · mode: {full|tcp_only} · :{http_port} → :{https_port} · pid {N} · tld: {tld}`
- **Right**: `showing X of Y requests` (with filter context if active)

Branding lives here, not at the top.

---

## New Backend API Endpoints

### `GET /api/routes`

Returns active routes from `StateStore` (via `RouteManager`). Response:

```json
{
  "routes": [
    {
      "hostname": "myapp.localhost",
      "port": 4123,
      "pid": 12345,
      "protocol": "http",
      "public_port": null,
      "cwd": "/path/to/project",
      "created_at": "2026-04-09T14:00:00Z"
    }
  ],
  "daemon": {
    "version": "0.1.0",
    "mode": "full",
    "http_port": 80,
    "https_port": 443,
    "pid": 33439,
    "uptime_secs": 8040,
    "tld": "localhost"
  }
}
```

Excludes the internal `_.localhost` inspector route. Polled every 5 seconds, or refreshed on SSE route events.

### `POST /api/routes/{hostname}/stop`

Stops a managed service (sends SIGTERM to its process group) and removes the route. Returns `{ "ok": true }` or `{ "ok": false, "error": "..." }`. For aliases (pid=0), just removes the route.

### `POST /api/routes/{hostname}/restart`

Stops the service, waits for port to free, then re-registers it. Returns same format. Only for managed services (not aliases).

---

## New Svelte Components

| Component | Responsibility |
|---|---|
| `ServiceCards.svelte` | Horizontal card strip, per-card stats, sparklines, actions |
| `ServiceCard.svelte` | Individual card: name, badge, stats, spark, action buttons |
| `FilterBar.svelte` | Method toggles, error filter, search input |
| `StatusBar.svelte` | Bottom bar with daemon info + request count |

### Modified Components

| Component | Change |
|---|---|
| `+page.svelte` | New layout: `ServiceCards` → `FilterBar` → `RequestFeed` → `StatusBar`. Detail panel as overlay. |
| `RequestFeed.svelte` | Add column header row, service hostname column, size column. Support card-based hostname filtering. |
| `Sidebar.svelte` | Remove — its functionality moves to `ServiceCards` + `FilterBar` |
| `RequestDetail.svelte` | Become a slide-in overlay panel instead of a third column |

### New Store

`ui/src/lib/stores/routes.svelte.ts` — routes state:

```typescript
interface RouteInfo {
  hostname: string;
  port: number;
  pid: number;
  protocol: 'http' | 'tcp';
  public_port: number | null;
  cwd: string;
  created_at: string;
}

interface DaemonInfo {
  version: string;
  mode: string;
  http_port: number;
  https_port: number;
  pid: number;
  uptime_secs: number;
  tld: string;
}
```

State: `routes[]`, `daemon`, `selectedHostname` (for card filter), `requestCounts` (per-hostname, incremented from SSE).

### New shadcn Components to Add

- `tooltip` — for action button labels
- `popover` — for alias connection hints
- `command` — for ⌘K palette (future)

---

## Design Tokens

- `border-radius`: 6px for cards, 4px for badges/buttons/chips, 2px for small elements
- No top bar — branding in bottom status bar
- Color palette: zinc-900 background, zinc-800 borders, green for HTTP/success, indigo for alias, amber for TCP, red for errors, blue for active/selected
- Font: system-ui for UI, monospace for data (ports, times, paths, sizes)
- Sparklines: 20 bars, 3px wide, 1px gap, height proportional to request rate

## Files Changed

### Backend (Rust)
| File | Change |
|---|---|
| `src/inspector/server.rs` | Add `GET /api/routes` and `POST /api/routes/{hostname}/stop` and `POST /api/routes/{hostname}/restart` endpoints |

### Frontend (Svelte)
| File | Change |
|---|---|
| `ui/src/routes/+page.svelte` | New layout composition |
| `ui/src/lib/components/ServiceCards.svelte` | New — card strip |
| `ui/src/lib/components/ServiceCard.svelte` | New — individual card |
| `ui/src/lib/components/FilterBar.svelte` | New — filter controls |
| `ui/src/lib/components/StatusBar.svelte` | New — bottom bar |
| `ui/src/lib/components/RequestFeed.svelte` | Enhanced with columns, hostname, size |
| `ui/src/lib/components/RequestDetail.svelte` | Slide-in overlay instead of third column |
| `ui/src/lib/components/Sidebar.svelte` | Remove |
| `ui/src/lib/stores/routes.svelte.ts` | New — routes + daemon state |
| `ui/src/lib/stores/requests.svelte.ts` | Add per-hostname counters from SSE |
| `ui/src/lib/api.ts` | Add `fetchRoutes()`, `stopRoute()`, `restartRoute()` |

## Testing

- `/api/routes` returns correct route list excluding `_.localhost`
- `/api/routes/{hostname}/stop` removes the route and kills the process
- `/api/routes/{hostname}/stop` on alias removes without SIGTERM
- Service cards render for HTTP, alias, and TCP routes
- Clicking a card filters the request feed
- Filter bar toggles work (method, errors, search)
- SSE updates increment per-service request counters
- Detail panel opens on row click, closes on Esc
- Status bar shows correct daemon info
