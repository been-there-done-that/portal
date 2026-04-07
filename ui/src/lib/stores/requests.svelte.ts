import { fetchRequests, type RequestMeta, type RequestRecord } from '$lib/api.js';

const MAX_IN_MEMORY = 2000;

// ── State ──────────────────────────────────────────────────────────────────
export let requests = $state<RequestMeta[]>([]);
export let selectedId = $state<number | null>(null);
export let selectedRecord = $state<RequestRecord | null>(null);
export let filterHostname = $state<string | null>(null);
export let filterMethods = $state<Set<string>>(new Set());
export let filterErrors = $state(false);
export let loading = $state(false);

// ── Derived ────────────────────────────────────────────────────────────────
export const filtered = $derived(
  requests.filter((r) => {
    if (filterHostname && r.hostname !== filterHostname) return false;
    if (filterMethods.size > 0 && !filterMethods.has(r.method)) return false;
    if (filterErrors && r.status < 400) return false;
    return true;
  })
);

export const hostnames = $derived([...new Set(requests.map((r) => r.hostname))]);

// ── Setters ────────────────────────────────────────────────────────────────
export function setFilterHostname(value: string | null) { filterHostname = value; }
export function setFilterMethods(value: Set<string>) { filterMethods = value; }
export function setFilterErrors(value: boolean) { filterErrors = value; }

// ── Actions ────────────────────────────────────────────────────────────────
export async function loadHistory() {
  loading = true;
  try {
    const res = await fetchRequests({ limit: 100, hostname: filterHostname ?? undefined });
    requests = res.requests;
  } finally {
    loading = false;
  }
}

export function prependRequest(meta: RequestMeta) {
  requests = [meta, ...requests].slice(0, MAX_IN_MEMORY);
}

export async function selectRequest(id: number) {
  selectedId = id;
  selectedRecord = null;
  const res = await fetchRequests({ id });
  if (res.requests.length > 0) {
    selectedRecord = res.requests[0];
  }
}

export function clearSelected() {
  selectedId = null;
  selectedRecord = null;
}
