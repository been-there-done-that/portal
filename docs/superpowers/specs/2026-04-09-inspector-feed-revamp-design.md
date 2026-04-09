# Inspector Feed Revamp Design

**Goal:** Revamp the request feed into a dense log-style view with content-type filtering, wider side panel without blur, infinite scroll for history, and full timestamps.

**Architecture:** Modify existing Svelte components (`RequestFeed`, `RequestDetail`, `FilterBar`) and the requests store. Add content-type filter to the backend response. No new files — all changes to existing components.

---

## Feed Rows — Log Style

Replace the fixed 7-column CSS grid with a flex layout where content wraps naturally:

```
[timestamp] [METHOD] [STATUS] [hostname]  path?query
                                          type: xhr · content: application/json
                                                                      [duration] [size]
```

- **Timestamp**: full `YYYY-MM-DD HH:MM:SS.mmm` format (e.g. `2026-04-09 17:59:32.123`)
- **Path**: wraps freely, query params in muted color
- **Details row**: content type category (xhr/js/css/img/doc/font/ws/other), content-type header value
- **Selected row**: blue left border (`border-l-3 border-blue-500`) + `bg-accent/50`
- **`_.localhost` excluded**: filtered out in the store (not shown)

## Content-Type Filter

Like Chrome Network tab. Chips in the filter bar after the error filter:

| Chip | Matches Content-Type |
|---|---|
| All | no filter |
| XHR | `application/json`, `text/plain`, `application/xml` (+ fetch/XMLHttpRequest) |
| JS | `application/javascript`, `text/javascript` |
| CSS | `text/css` |
| Img | `image/*` |
| Doc | `text/html` |
| Font | `font/*`, `application/font-*` |
| WS | WebSocket upgrade (status 101) |
| Other | everything else |

The content type comes from the **response** `Content-Type` header. Since `RequestMeta` (the SSE payload) doesn't include headers, we need to add a `content_type` field to `RequestMeta` so the feed can filter without fetching full details.

### Backend change

Add `content_type: Option<String>` to `RequestMeta` and `RequestRecord`. Populated from the response `Content-Type` header when the captured request is inserted into the DB. Add a `content_type TEXT` column to the SQLite schema.

### Frontend change

Add `filterContentType: string | null` to the requests store (`null` = All). The `_filtered` derived applies the content-type check by categorizing each request's `content_type` string.

## Side Panel

- **Width**: `w-[55vw]` (55% of viewport, was `w-[600px]`)
- **No backdrop blur**: set sheet overlay to `bg-transparent` or remove it entirely. The panel casts a shadow instead.
- **Slide animation**: CSS `transition: transform 300ms ease` on the panel. Svelte `transition:fly={{ x: 100, duration: 200 }}` or sheet's built-in animation.
- **Close**: `✕` button + `Escape` key (already implemented)

## Infinite Scroll (Load Older)

When the user scrolls to the top of the feed, automatically load older requests:

1. Detect scroll position: when `scrollTop < 50px` and not already loading
2. Call `fetchRequests({ before_id: oldest_request_id, limit: 100 })`
3. Prepend results to the request list (at the bottom of the array, since requests are newest-first)
4. Restore scroll position so the view doesn't jump
5. Stop when `has_more === false`

The existing `loadHistory` fetches the latest 100. Infinite scroll loads backwards from the oldest loaded request.

## Search

- **Width**: `w-80` (320px, was `w-48`)
- **Placeholder**: `⌘F  Filter by path, hostname...`
- **Behavior**: client-side filter on `req.path` and `req.hostname` (already partially implemented — needs to be wired to the store)

Add `filterSearch: string` to the requests store. The `_filtered` derived checks if path or hostname includes the search string (case-insensitive).

## Files Changed

| File | Change |
|---|---|
| `src/inspector/types.rs` | Add `content_type: Option<String>` to `RequestMeta` and `RequestRecord` |
| `src/inspector/db.rs` | Add `content_type` column to schema + queries |
| `src/inspector/mod.rs` | Populate `content_type` from response headers in `to_meta` |
| `ui/src/lib/api.ts` | Add `content_type` to `RequestMeta` type |
| `ui/src/lib/stores/requests.svelte.ts` | Add `filterContentType`, `filterSearch`, infinite scroll `loadOlder()`, exclude `_.localhost` |
| `ui/src/lib/components/RequestFeed.svelte` | Log-style flex rows, full timestamp, content details, infinite scroll trigger |
| `ui/src/lib/components/RequestDetail.svelte` | 55vw width, no blur overlay, slide animation |
| `ui/src/lib/components/FilterBar.svelte` | Content-type chips, wider search, search wired to store |

## Testing

- Content-type filter: XHR chip hides JS/CSS/Img requests
- Search: typing "api" filters to paths/hostnames containing "api"
- Infinite scroll: scrolling up loads older requests
- `_.localhost` excluded from feed
- Side panel: 55vw wide, no blur, closes on Esc
- Full timestamp shown in each row
- Selected row highlighted with blue border
