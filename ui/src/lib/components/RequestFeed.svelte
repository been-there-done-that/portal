<script lang="ts">
  import { Badge } from '$lib/components/ui/badge/index.js';
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

<div class="flex flex-1 min-h-0 flex-col overflow-hidden">
  <!-- Column headers -->
  <div
    class="grid shrink-0 items-center border-b border-border bg-card px-2 py-1 font-mono text-[9px] uppercase tracking-wider text-muted-foreground/40"
    style="grid-template-columns: 140px 42px 36px 50px 150px 1fr 45px;"
  >
    <span>Time</span>
    <span>Method</span>
    <span>Status</span>
    <span>Type</span>
    <span>Service</span>
    <span>Path</span>
    <span class="text-right">Duration</span>
  </div>

  <!-- Rows -->
  <div class="flex-1 min-h-0 overflow-y-auto" onscroll={handleScroll}>
    {#each store.filtered as req (req.id)}
      <button
        class="grid w-full items-center border-b border-border/30 px-2 py-[3px] text-left font-mono transition-colors hover:bg-accent/20
               {store.selectedId === req.id ? 'bg-blue-950/30 border-l-2 border-l-blue-500 pl-[6px]' : ''}"
        style="grid-template-columns: 140px 42px 36px 50px 150px 1fr 45px;"
        onclick={() => selectRequest(req.id)}
      >
        <span class="text-[10px] text-muted-foreground/60">{formatTimestamp(req.timestamp)}</span>
        <span class="text-[10px] font-semibold {methodColor(req.method)}">{req.method}</span>
        <span class="text-[10px] font-medium {statusColor(req.status)}">{req.status}</span>
        <span class="text-[9px]">
          {#if req.content_type}
            <span class="inline-block rounded-sm bg-muted px-1 py-[1px] text-[8px] text-muted-foreground">{contentCategory(req.content_type, req.status)}</span>
          {/if}
        </span>
        <span class="truncate text-[10px] text-muted-foreground">{req.hostname}</span>
        <span class="truncate text-[10px] text-foreground/80">
          {#if req.path.includes('?')}
            {req.path.split('?')[0]}<span class="text-muted-foreground/30">?{req.path.split('?').slice(1).join('?')}</span>
          {:else}
            {req.path}
          {/if}
        </span>
        <span class="text-right text-[10px] text-muted-foreground/50">{req.duration_ms}ms</span>
      </button>
    {/each}

    {#if store.filtered.length === 0}
      <div class="px-4 py-8 text-center font-mono text-xs text-muted-foreground">
        No requests yet
      </div>
    {/if}

    {#if store.loadingOlder}
      <div class="py-2 text-center text-[9px] text-muted-foreground/50">Loading...</div>
    {/if}
  </div>
</div>
