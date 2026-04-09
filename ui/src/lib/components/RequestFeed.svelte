<script lang="ts">
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { store, selectRequest } from '$lib/stores/requests.svelte.js';

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

  function formatTime(ms: number): string {
    const d = new Date(ms);
    return d.toLocaleTimeString('en-US', { hour12: false });
  }
</script>

<div class="flex flex-1 min-h-0 flex-col bg-background">
  <!-- Column headers -->
  <div
    class="grid flex-shrink-0 border-b border-border px-2 py-1"
    style="grid-template-columns: 60px 50px 45px 160px 1fr 55px 55px"
  >
    {#each ['TIME', 'METHOD', 'STATUS', 'SERVICE', 'PATH', 'DURATION', 'SIZE'] as col}
      <span class="font-mono text-[9px] uppercase tracking-wider text-muted-foreground/50">{col}</span>
    {/each}
  </div>

  <ScrollArea class="flex-1">
    {#each store.filtered as req (req.id)}
      <button
        class="grid w-full border-l-2 px-2 py-1.5 text-left transition-colors hover:bg-accent/30
               {store.selectedId === req.id
                 ? 'border-blue-500 bg-accent/50'
                 : 'border-transparent'}"
        style="grid-template-columns: 60px 50px 45px 160px 1fr 55px 55px"
        onclick={() => selectRequest(req.id)}
      >
        <span class="font-mono text-[10px] text-muted-foreground">{formatTime(req.timestamp)}</span>
        <span class="font-mono text-[10px] font-medium {methodColor(req.method)}">{req.method}</span>
        <span class="font-mono text-[10px] font-medium {statusColor(req.status)}">{req.status}</span>
        <span class="truncate font-mono text-[10px] text-muted-foreground">{req.hostname}</span>
        <span class="truncate font-mono text-[10px] text-foreground">{req.path}</span>
        <span class="font-mono text-[10px] text-muted-foreground">{req.duration_ms}ms</span>
        <span class="font-mono text-[10px] text-muted-foreground">—</span>
      </button>
    {/each}

    {#if store.filtered.length === 0}
      <div class="px-4 py-8 text-center font-mono text-xs text-muted-foreground">
        No requests yet
      </div>
    {/if}
  </ScrollArea>
</div>
