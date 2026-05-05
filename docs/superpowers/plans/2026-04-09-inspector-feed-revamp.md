# Inspector Feed Revamp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Revamp the request feed into a dense log-style view with content-type filtering, wider side panel without blur, infinite scroll for history, full timestamps, and bigger search.

**Architecture:** Backend adds `content_type` field to `RequestMeta` and DB schema. Frontend store adds content-type and search filters plus infinite scroll. UI components get log-style rows, content-type chips, wider panel, and scroll-triggered history loading.

**Tech Stack:** Rust (SQLite, Axum), SvelteKit 5, Tailwind v4, shadcn-svelte

---

## File Map

### Backend
| File | Change |
|---|---|
| `src/inspector/types.rs` | Add `content_type: Option<String>` to `RequestMeta`, `RequestRecord`, `CapturedRequest` |
| `src/inspector/db.rs` | Add `content_type` column, update INSERT/SELECT/row_to_record |
| `src/inspector/sse.rs` | Populate `content_type` in `to_meta()` |

### Frontend
| File | Change |
|---|---|
| `ui/src/lib/api.ts` | Add `content_type` to `RequestMeta` type |
| `ui/src/lib/stores/requests.svelte.ts` | Add `filterContentType`, `filterSearch`, `loadOlder()`, update `_filtered` |
| `ui/src/lib/components/FilterBar.svelte` | Content-type chips, bigger search, wire search to store |
| `ui/src/lib/components/RequestFeed.svelte` | Log-style flex rows, full timestamp, content details, infinite scroll |
| `ui/src/lib/components/RequestDetail.svelte` | 55vw width, no blur overlay, slide animation |

---

## Task 1: Backend — Add `content_type` field

**Files:**
- Modify: `src/inspector/types.rs`
- Modify: `src/inspector/db.rs`
- Modify: `src/inspector/sse.rs`

### Background

Add `content_type: Option<String>` to capture the response Content-Type header. This lets the frontend filter by content type (XHR, JS, CSS, Img, etc.) without fetching full headers.

- [ ] **Step 1: Add `content_type` to types**

In `src/inspector/types.rs`:

Add to `CapturedRequest` struct (after `res_body`):
```rust
    pub content_type: Option<String>,
```

Add to `RequestMeta` struct (after `timestamp`):
```rust
    pub content_type: Option<String>,
```

Add to `RequestRecord` struct (after `res_total_bytes`):
```rust
    pub content_type: Option<String>,
```

- [ ] **Step 2: Populate `content_type` in the proxy**

In `src/proxy.rs`, where `CapturedRequest` is constructed (both the streaming and sync paths), add:

```rust
content_type: res_headers.iter()
    .find(|(k, _)| k.eq_ignore_ascii_case("content-type"))
    .map(|(_, v)| v.clone()),
```

There are two places where `CapturedRequest` is built: the `should_stream` branch (spawned task, around line 474) and the sync branch (around line 499). Add the field to both.

Also update the test helper in `sse.rs` (`broadcast_send_recv`) to include the new field:
```rust
content_type: None,
```

- [ ] **Step 3: Add DB column + update queries**

In `src/inspector/db.rs`:

**Schema** — add to CREATE TABLE:
```sql
content_type    TEXT
```

Add after `res_total_bytes` column, before the closing `)`.

**Also add a migration** for existing databases. After the CREATE TABLE, add:
```rust
// Migration: add content_type column if missing (existing DBs)
conn.execute_batch(
    "ALTER TABLE requests ADD COLUMN content_type TEXT;"
).ok(); // ok() — silently ignores if column already exists
```

**INSERT** — add `content_type` as the 15th param:

Update the INSERT SQL to include `,content_type` in both column list and VALUES, and add `req.content_type` to the params.

**SELECT** — add `content_type` to all SELECT queries (it becomes the 16th column, index 15).

**`row_to_record`** — add at the end:
```rust
content_type: row.get(15).ok(),
```

- [ ] **Step 4: Populate in SSE `to_meta()`**

In `src/inspector/sse.rs`, update `to_meta()`:
```rust
pub fn to_meta(req: &CapturedRequest, id: i64) -> RequestMeta {
    RequestMeta {
        id,
        hostname: req.hostname.clone(),
        method: req.method.clone(),
        path: req.path.clone(),
        status: req.res_status,
        duration_ms: req.duration_ms,
        timestamp: req.timestamp,
        content_type: req.content_type.clone(),
    }
}
```

- [ ] **Step 5: Fix all test constructors**

Any test that constructs `CapturedRequest` or `RequestMeta` needs `content_type: None` added. Search for `CapturedRequest {` and `RequestMeta {` across the test modules in `types.rs`, `sse.rs`, `db.rs`.

- [ ] **Step 6: Build and test**

```bash
cargo build 2>&1 | grep "^error" | head -20
cargo test 2>&1 | grep -E "^test result|FAILED"
```

- [ ] **Step 7: Commit**

```bash
git add src/inspector/types.rs src/inspector/db.rs src/inspector/sse.rs src/proxy.rs
git commit -m "feat(inspector): add content_type field to RequestMeta, DB, and SSE"
```

---

## Task 2: Frontend store — filters + infinite scroll

**Files:**
- Modify: `ui/src/lib/api.ts`
- Modify: `ui/src/lib/stores/requests.svelte.ts`

### Background

Add `content_type` to the TypeScript `RequestMeta` type. Add `filterContentType` and `filterSearch` to the store. Add `loadOlder()` for infinite scroll that fetches older requests using `before_id`.

- [ ] **Step 1: Update `api.ts`**

Add to `RequestMeta` interface (after `timestamp`):
```typescript
  content_type: string | null;
```

Add to `RequestRecord` interface:
```typescript
  content_type: string | null;
```

- [ ] **Step 2: Update `requests.svelte.ts`**

Add new state fields:
```typescript
filterContentType: null as string | null,  // null = All
filterSearch: '',
loadingOlder: false,
hasMore: true,
```

Add to the `store` export:
```typescript
get filterContentType(): string | null { return _s.filterContentType; },
get filterSearch(): string { return _s.filterSearch; },
get loadingOlder(): boolean { return _s.loadingOlder; },
get hasMore(): boolean { return _s.hasMore; },
```

Add setters:
```typescript
export function setFilterContentType(value: string | null) { _s.filterContentType = value; }
export function setFilterSearch(value: string) { _s.filterSearch = value; }
```

Update `_filtered` derived to include content-type and search filters:
```typescript
const _filtered = $derived(
  _s.requests.filter((r) => {
    if (_s.filterHostname && r.hostname !== _s.filterHostname) return false;
    if (_s.filterMethods.size > 0 && !_s.filterMethods.has(r.method)) return false;
    if (_s.filterErrors && r.status < 400) return false;
    if (_s.filterContentType) {
      const ct = (r.content_type ?? '').toLowerCase();
      if (!matchesContentCategory(_s.filterContentType, ct, r.status)) return false;
    }
    if (_s.filterSearch) {
      const q = _s.filterSearch.toLowerCase();
      if (!r.path.toLowerCase().includes(q) && !r.hostname.toLowerCase().includes(q)) return false;
    }
    return true;
  })
);
```

Add the content-type matching helper:
```typescript
function matchesContentCategory(category: string, contentType: string, status: number): boolean {
  switch (category) {
    case 'xhr': return contentType.includes('json') || contentType.includes('xml') || contentType.includes('text/plain');
    case 'js': return contentType.includes('javascript');
    case 'css': return contentType.includes('text/css');
    case 'img': return contentType.startsWith('image/');
    case 'doc': return contentType.includes('text/html');
    case 'font': return contentType.includes('font');
    case 'ws': return status === 101;
    case 'other': return !['json','xml','text/plain','javascript','text/css','image/','text/html','font'].some(t => contentType.includes(t)) && status !== 101;
    default: return true;
  }
}
```

Add `loadOlder()`:
```typescript
export async function loadOlder() {
  if (_s.loadingOlder || !_s.hasMore || _s.requests.length === 0) return;
  _s.loadingOlder = true;
  try {
    const oldestId = _s.requests[_s.requests.length - 1].id;
    const res = await fetchRequests({
      limit: 100,
      before_id: oldestId,
      hostname: _s.filterHostname ?? undefined,
    });
    _s.requests = [..._s.requests, ...res.requests];
    _s.hasMore = res.has_more;
  } finally {
    _s.loadingOlder = false;
  }
}
```

Update `loadHistory` to set `hasMore`:
```typescript
export async function loadHistory() {
  _s.loading = true;
  try {
    const res = await fetchRequests({ limit: 100, hostname: _s.filterHostname ?? undefined });
    _s.requests = res.requests;
    _s.hasMore = res.has_more;
  } finally {
    _s.loading = false;
  }
}
```

- [ ] **Step 3: Type check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

- [ ] **Step 4: Commit**

```bash
git add ui/src/lib/api.ts ui/src/lib/stores/requests.svelte.ts
git commit -m "feat(ui): add content-type filter, search filter, and infinite scroll to store"
```

---

## Task 3: Frontend — FilterBar with content-type chips + bigger search

**Files:**
- Modify: `ui/src/lib/components/FilterBar.svelte`

### Background

Add content-type filter chips (All, XHR, JS, CSS, Img, Doc, Font, WS, Other) after the error filter. Make search input wider (320px). Wire search to `setFilterSearch`.

- [ ] **Step 1: Rewrite FilterBar**

Read the current `FilterBar.svelte`, then update:

1. Add content-type chips after the error filter chip:
```svelte
<div style="width: 1px; height: 14px;" class="bg-border"></div>
<div class="flex gap-1">
  {#each ['All','XHR','JS','CSS','Img','Doc','Font','WS','Other'] as ct}
    <button onclick={() => setFilterContentType(ct === 'All' ? null : ct.toLowerCase())}>
      <Badge
        variant={store.filterContentType === (ct === 'All' ? null : ct.toLowerCase()) ? 'secondary' : 'outline'}
        class="cursor-pointer rounded px-2 py-0.5 text-[9px] uppercase tracking-wide leading-none"
      >
        {ct}
      </Badge>
    </button>
  {/each}
</div>
```

2. Change search width from `w-48` to `w-80` and wire to store:
```svelte
<input
  class="ml-auto h-7 w-80 rounded border border-border bg-transparent px-3 font-mono text-[11px] text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
  placeholder="⌘F  Filter by path, hostname..."
  oninput={(e) => setFilterSearch(e.currentTarget.value)}
  value={store.filterSearch}
/>
```

3. Import `setFilterContentType` and `setFilterSearch` from the store.

- [ ] **Step 2: Type check**

```bash
cd ui && bun run check 2>&1 | tail -10
```

- [ ] **Step 3: Commit**

```bash
git add ui/src/lib/components/FilterBar.svelte
git commit -m "feat(ui): add content-type chips and bigger search to FilterBar"
```

---

## Task 4: Frontend — Log-style feed + wider panel + infinite scroll

**Files:**
- Modify: `ui/src/lib/components/RequestFeed.svelte`
- Modify: `ui/src/lib/components/RequestDetail.svelte`

### Background

Convert the grid-based feed to log-style flex rows with full timestamps and content details. Add scroll-to-top detection for infinite scroll. Make the detail panel 55vw wide with no blur overlay.

- [ ] **Step 1: Rewrite RequestFeed**

Key changes:
- Replace the CSS grid with flex rows
- **Full timestamp**: format `YYYY-MM-DD HH:MM:SS.mmm` using `new Date(timestamp).toISOString().replace('T', ' ').slice(0, 23)`
- Each row: `[timestamp] [METHOD] [STATUS] [hostname] | path?query | [duration] [size]`
- Sub-row: content-type category + content-type header value
- Query params in muted color: split path on `?`, render query part with `text-muted-foreground/50`
- Selected row: `border-l-3 border-blue-500 bg-accent/50`
- **Infinite scroll**: use Svelte's `onMount` + scroll event listener on the feed container. When `scrollTop + clientHeight >= scrollHeight - 100`, call `loadOlder()`. Show a loading indicator at the bottom when `store.loadingOlder` is true.

Content-type category helper (reuse the same categories from the store):
```typescript
function contentCategory(ct: string | null, status: number): string {
  if (!ct) return '';
  const c = ct.toLowerCase();
  if (status === 101) return 'ws';
  if (c.includes('json') || c.includes('xml') || c.includes('text/plain')) return 'xhr';
  if (c.includes('javascript')) return 'js';
  if (c.includes('text/css')) return 'css';
  if (c.startsWith('image/')) return 'img';
  if (c.includes('text/html')) return 'doc';
  if (c.includes('font')) return 'font';
  return 'other';
}
```

- [ ] **Step 2: Update RequestDetail**

Changes to the Sheet component usage:
- Width: change `w-[600px] sm:max-w-[600px]` to `w-[55vw] sm:max-w-[55vw]`
- Overlay: set the sheet overlay to transparent or remove it. In shadcn-svelte, the `Sheet.Overlay` can be styled with `class="bg-transparent"` or removed entirely.
- Ensure slide animation works (shadcn-svelte sheet already animates).

- [ ] **Step 3: Build frontend**

```bash
cd ui && bun run check && bun run build 2>&1 | tail -10
```

- [ ] **Step 4: Build Rust binary**

```bash
cd /Users/__deesh_reddy__/projects/personal_git/rust_builds/portless && cargo build 2>&1 | tail -5
```

- [ ] **Step 5: Run Rust tests**

```bash
cargo test 2>&1 | grep -E "^test result|FAILED"
```

- [ ] **Step 6: Commit**

```bash
git add -A ui/src/ src/
git commit -m "feat(ui): log-style feed with full timestamps, content details, infinite scroll, wider panel"
```

---

## Self-Review

**Spec coverage:**

- ✅ Log-style flex rows — Task 4
- ✅ Full timestamp `YYYY-MM-DD HH:MM:SS.mmm` — Task 4
- ✅ Content details sub-row — Task 4
- ✅ Content-type filter chips — Task 3
- ✅ `content_type` field in backend — Task 1
- ✅ `content_type` in SSE meta — Task 1
- ✅ Bigger search (320px) — Task 3
- ✅ Search wired to store — Task 3
- ✅ `filterSearch` in store — Task 2
- ✅ `filterContentType` in store — Task 2
- ✅ Infinite scroll `loadOlder()` — Task 2 + Task 4
- ✅ Side panel 55vw — Task 4
- ✅ No blur overlay — Task 4
- ✅ Selected row highlight — Task 4
- ✅ `_.localhost` excluded — already done (proxy.rs skip)

**No placeholders found.**

**Type consistency:** `content_type: Option<String>` in Rust maps to `content_type: string | null` in TS. `matchesContentCategory` in store matches `contentCategory` in feed component.
