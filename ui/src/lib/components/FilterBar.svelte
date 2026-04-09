<script lang="ts">
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import {
    store,
    loadHistory,
    setFilterHostname,
    setFilterMethods,
    setFilterErrors,
    setFilterContentType,
    setFilterSearch,
  } from '$lib/stores/requests.svelte.js';
  import { deleteAllRequests } from '$lib/api.js';

  const METHODS = ['ALL', 'GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

  function toggleMethod(m: string) {
    if (m === 'ALL') {
      setFilterMethods(new Set());
      return;
    }
    const next = new Set(store.filterMethods);
    next.has(m) ? next.delete(m) : next.add(m);
    setFilterMethods(next);
  }

  function isMethodActive(m: string): boolean {
    if (m === 'ALL') return store.filterMethods.size === 0;
    return store.filterMethods.has(m);
  }

  async function clearHistory() {
    await deleteAllRequests(store.filterHostname ?? undefined);
    await loadHistory();
  }
</script>

<div class="flex h-9 items-center gap-2 border-t border-border bg-card px-3 font-mono text-[10px]">
  <!-- Label -->
  <span class="shrink-0 uppercase tracking-widest text-muted-foreground">Filter</span>

  <!-- Method pills -->
  <div class="flex items-center gap-1">
    {#each METHODS as method}
      <button onclick={() => toggleMethod(method)}>
        <Badge
          variant={isMethodActive(method) ? 'default' : 'outline'}
          class="cursor-pointer rounded px-2 py-0.5 text-[10px] leading-none"
        >
          {method}
        </Badge>
      </button>
    {/each}
  </div>

  <!-- Separator -->
  <div class="h-4 w-px shrink-0 bg-border"></div>

  <!-- Error pill -->
  <button onclick={() => setFilterErrors(!store.filterErrors)}>
    <Badge
      variant={store.filterErrors ? 'destructive' : 'outline'}
      class="cursor-pointer rounded px-2 py-0.5 text-[10px] leading-none"
    >
      4xx/5xx
    </Badge>
  </button>

  <!-- Content-type chips -->
  <div style="width: 1px; height: 14px;" class="bg-border"></div>
  <div class="flex gap-1">
    {#each ['All','XHR','JS','CSS','Img','Doc','Font','WS','Other'] as ct}
      <button onclick={() => setFilterContentType(ct === 'All' ? null : ct.toLowerCase())}>
        <Badge
          variant={(ct === 'All' && store.filterContentType === null) || store.filterContentType === ct.toLowerCase() ? 'secondary' : 'outline'}
          class="cursor-pointer rounded px-2 py-0.5 text-[9px] uppercase tracking-wide leading-none"
        >
          {ct}
        </Badge>
      </button>
    {/each}
  </div>

  <!-- Search input — right-aligned -->
  <input
    class="ml-auto h-7 w-80 rounded border border-border bg-transparent px-3 font-mono text-[11px] text-foreground placeholder:text-muted-foreground focus:outline-none focus:ring-1 focus:ring-ring"
    placeholder="⌘F  Filter by path, hostname..."
    oninput={(e) => setFilterSearch(e.currentTarget.value)}
    value={store.filterSearch}
  />

  <!-- Clear history -->
  <Button
    variant="ghost"
    size="sm"
    class="h-6 shrink-0 px-2 text-[10px] text-destructive hover:text-destructive"
    onclick={clearHistory}
  >
    Clear history
  </Button>
</div>
