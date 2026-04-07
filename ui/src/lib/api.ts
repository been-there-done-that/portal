export interface RequestMeta {
  id: number;
  hostname: string;
  method: string;
  path: string;
  status: number;
  duration_ms: number;
  timestamp: number;
}

export interface RequestRecord extends RequestMeta {
  req_headers: [string, string][];
  req_body: string;
  req_truncated: boolean;
  req_total_bytes: number;
  res_headers: [string, string][];
  res_body: string;
  res_truncated: boolean;
  res_total_bytes: number;
}

export interface RequestsResponse {
  requests: RequestRecord[];
  has_more: boolean;
}

export async function fetchRequests(params: {
  hostname?: string;
  limit?: number;
  before_id?: number;
  id?: number;
}): Promise<RequestsResponse> {
  const url = new URL('/api/requests', window.location.origin);
  if (params.hostname) url.searchParams.set('hostname', params.hostname);
  if (params.limit) url.searchParams.set('limit', String(params.limit));
  if (params.before_id) url.searchParams.set('before_id', String(params.before_id));
  if (params.id) url.searchParams.set('id', String(params.id));
  const res = await fetch(url.toString());
  return res.json();
}

export async function deleteAllRequests(hostname?: string): Promise<void> {
  const url = new URL('/api/requests', window.location.origin);
  if (hostname) url.searchParams.set('hostname', hostname);
  await fetch(url.toString(), { method: 'DELETE' });
}

export async function deleteRequest(id: number): Promise<void> {
  await fetch(`/api/requests/${id}`, { method: 'DELETE' });
}
