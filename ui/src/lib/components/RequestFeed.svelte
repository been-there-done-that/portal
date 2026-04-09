<script lang="ts">
  import { store, selectRequest, loadOlder } from '$lib/stores/requests.svelte.js';

  function methodColor(method: string): string {
    if (method === 'GET') return 'text-green-500';
    if (method === 'POST') return 'text-yellow-500';
    if (method === 'PUT') return 'text-blue-400';
    if (method === 'DELETE') return 'text-red-500';
    if (method === 'PATCH') return 'text-orange-400';
    return 'text-muted-foreground';
  }

  function statusColor(status: number): string {
    if (status >= 500) return 'text-red-500';
    if (status >= 400) return 'text-orange-500';
    if (status >= 300) return 'text-blue-400';
    return 'text-green-500';
  }

  function formatTimestamp(ms: number): string {
    return new Date(ms).toISOString().replace('T', ' ').slice(0, 23);
  }

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

  function handleScroll(e: Event) {
    const el = e.target as HTMLElement;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 100) {
      loadOlder();
    }
  }
</script>

<div class="flex flex-1 min-h-0 flex-col bg-background overflow-hidden">
  <div class="flex-1 min-h-0 overflow-y-auto" onscroll={handleScroll}>
    {#each store.filtered as req (req.id)}
      <button
        class="group flex w-full items-start gap-2 border-l-3 px-4 py-1.5 text-left font-mono transition-colors hover:bg-accent/30
               {store.selectedId === req.id ? 'border-blue-500 bg-accent/50' : 'border-transparent'}"
        onclick={() => selectRequest(req.id)}
      >
        <span class="w-[130px] shrink-0 text-[10px] text-muted-foreground">{formatTimestamp(req.timestamp)}</span>
        <span class="w-[50px] shrink-0 text-[10px] font-semibold {methodColor(req.method)}">{req.method}</span>
        <span class="w-[35px] shrink-0 text-[10px] font-medium {statusColor(req.status)}">{req.status}</span>
        <span class="w-[160px] shrink-0 truncate text-[10px] text-muted-foreground">{req.hostname}</span>
        <div class="min-w-0 flex-1">
          <div class="text-[11px] text-foreground break-all">
            {#if req.path.includes('?')}
              {req.path.split('?')[0]}<span class="text-muted-foreground/50">?{req.path.split('?').slice(1).join('?')}</span>
            {:else}
              {req.path}
            {/if}
          </div>
          {#if req.content_type}
            <div class="mt-0.5 flex gap-3 text-[9px] text-muted-foreground/60">
              <span>{contentCategory(req.content_type, req.status)}</span>
              <span>{req.content_type}</span>
            </div>
          {/if}
        </div>
        <span class="w-[55px] shrink-0 text-right text-[10px] text-muted-foreground">{req.duration_ms}ms</span>
        <span class="w-[55px] shrink-0 text-right text-[10px] text-muted-foreground">—</span>
      </button>
    {/each}

    {#if store.filtered.length === 0}
      <div class="px-4 py-8 text-center font-mono text-xs text-muted-foreground">
        No requests yet
      </div>
    {/if}

    {#if store.loadingOlder}
      <div class="py-3 text-center text-[10px] text-muted-foreground">Loading older requests...</div>
    {/if}
  </div>
</div>
