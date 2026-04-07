<script lang="ts">
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { store, selectRequest } from '$lib/stores/requests.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  function methodVariant(method: string): 'default' | 'secondary' | 'destructive' | 'outline' {
    if (method === 'GET') return 'secondary';
    if (method === 'DELETE') return 'destructive';
    return 'default';
  }

  function statusColor(status: number): string {
    if (status >= 500) return 'text-destructive';
    if (status >= 400) return 'text-orange-500';
    if (status >= 300) return 'text-blue-400';
    return 'text-green-500';
  }

  function formatTime(ms: number): string {
    const d = new Date(ms);
    return d.toLocaleTimeString('en-US', { hour12: false });
  }
</script>

<div class="flex h-full w-[300px] flex-shrink-0 flex-col border-r border-border bg-background">
  <!-- Header -->
  <div class="flex items-center justify-between border-b border-border px-3 py-2">
    <span class="font-mono text-[11px] text-muted-foreground">{store.filtered.length} requests</span>
  </div>

  <ScrollArea class="flex-1">
    {#each store.filtered as req (req.id)}
      <button
        class="w-full border-l-2 px-3 py-2 text-left font-mono transition-colors hover:bg-accent/30
               {store.selectedId === req.id ? 'border-primary bg-accent/50' : 'border-transparent'}"
        onclick={() => selectRequest(req.id)}
      >
        <div class="flex items-center gap-2">
          <Badge variant={methodVariant(req.method)} class="px-1.5 py-0 text-[10px] font-mono">
            {req.method}
          </Badge>
          <span class="flex-1 truncate text-[11px] text-foreground">{req.path}</span>
          <span class="text-[10px] font-medium {statusColor(req.status)}">{req.status}</span>
        </div>
        <div class="mt-0.5 flex gap-3 text-[10px] text-muted-foreground">
          <span>{formatTime(req.timestamp)}</span>
          <span>{req.duration_ms}ms</span>
        </div>
      </button>
    {/each}

    {#if store.filtered.length === 0}
      <div class="px-4 py-8 text-center font-mono text-xs text-muted-foreground">
        No requests yet
      </div>
    {/if}
  </ScrollArea>
</div>
