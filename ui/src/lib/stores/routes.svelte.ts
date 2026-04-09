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
  _s.requestCounts = {
    ..._s.requestCounts,
    [hostname]: (_s.requestCounts[hostname] ?? 0) + 1,
  };

  const samples = [...(_s.latencySamples[hostname] ?? []), durationMs].slice(-20);
  _s.latencySamples = { ..._s.latencySamples, [hostname]: samples };

  if (status >= 400) {
    _s.errorCounts = {
      ..._s.errorCounts,
      [hostname]: (_s.errorCounts[hostname] ?? 0) + 1,
    };
  }
}
