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
  loading: false,
});

// ── Derived ────────────────────────────────────────────────────────────────
const _filtered = $derived(
  _s.requests.filter((r) => {
    if (_s.filterHostname && r.hostname !== _s.filterHostname) return false;
    if (_s.filterMethods.size > 0 && !_s.filterMethods.has(r.method)) return false;
    if (_s.filterErrors && r.status < 400) return false;
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
  get loading(): boolean { return _s.loading; },
  get filtered(): RequestMeta[] { return _filtered; },
  get hostnames(): string[] { return _hostnames; },
};

// ── Setters ────────────────────────────────────────────────────────────────
export function setFilterHostname(value: string | null) { _s.filterHostname = value; }
export function setFilterMethods(value: Set<string>) { _s.filterMethods = value; }
export function setFilterErrors(value: boolean) { _s.filterErrors = value; }

// ── Actions ────────────────────────────────────────────────────────────────
export async function loadHistory() {
  _s.loading = true;
  try {
    const res = await fetchRequests({ limit: 100, hostname: _s.filterHostname ?? undefined });
    _s.requests = res.requests;
  } finally {
    _s.loading = false;
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
