import { fetchRequests, type RequestMeta, type RequestRecord } from '$lib/api.js';

const MAX_IN_MEMORY = 2000;

// ── Internal state object — properties are mutated, not the binding itself ──
const _s = $state({
  requests: [] as RequestMeta[],
  selectedId: null as number | null,
  selectedRecord: null as RequestRecord | null,
  filterHostname: null as string | null,
  filterMethods: new Set<string>(),
  filterErrors: false,
  filterContentType: null as string | null,
  filterSearch: '',
  loadingOlder: false,
  hasMore: true,
  loading: false,
});

// ── Helpers ────────────────────────────────────────────────────────────────
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

// ── Derived ────────────────────────────────────────────────────────────────
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

const _hostnames = $derived([...new Set(_s.requests.map((r) => r.hostname))]);

// ── Exported store object with reactive getters ────────────────────────────
// Components import `store` and access store.requests, store.filtered, etc.
export const store = {
  get requests(): RequestMeta[] { return _s.requests; },
  get selectedId(): number | null { return _s.selectedId; },
  get selectedRecord(): RequestRecord | null { return _s.selectedRecord; },
  get filterHostname(): string | null { return _s.filterHostname; },
  get filterMethods(): Set<string> { return _s.filterMethods; },
  get filterErrors(): boolean { return _s.filterErrors; },
  get filterContentType(): string | null { return _s.filterContentType; },
  get filterSearch(): string { return _s.filterSearch; },
  get loadingOlder(): boolean { return _s.loadingOlder; },
  get hasMore(): boolean { return _s.hasMore; },
  get loading(): boolean { return _s.loading; },
  get filtered(): RequestMeta[] { return _filtered; },
  get hostnames(): string[] { return _hostnames; },
};

// ── Setters ────────────────────────────────────────────────────────────────
export function setFilterHostname(value: string | null) { _s.filterHostname = value; }
export function setFilterMethods(value: Set<string>) { _s.filterMethods = value; }
export function setFilterErrors(value: boolean) { _s.filterErrors = value; }
export function setFilterContentType(value: string | null) { _s.filterContentType = value; }
export function setFilterSearch(value: string) { _s.filterSearch = value; }

// ── Actions ────────────────────────────────────────────────────────────────
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

export function prependRequest(meta: RequestMeta) {
  _s.requests = [meta, ..._s.requests].slice(0, MAX_IN_MEMORY);
}

export async function selectRequest(id: number) {
  _s.selectedId = id;
  _s.selectedRecord = null;
  const res = await fetchRequests({ id });
  if (res.requests.length > 0) {
    _s.selectedRecord = res.requests[0];
  }
}

export function clearSelected() {
  _s.selectedId = null;
  _s.selectedRecord = null;
}
