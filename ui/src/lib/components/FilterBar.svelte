<script lang="ts">
  import { Badge } from '$lib/components/ui/badge/index.js';
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
  const CONTENT_TYPES = ['All', 'XHR', 'JS', 'CSS', 'Img', 'Doc', 'Font', 'WS', 'Other'];

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

  function isContentActive(ct: string): boolean {
    if (ct === 'All') return store.filterContentType === null;
    return store.filterContentType === ct.toLowerCase();
  }

  async function clearHistory() {
    await deleteAllRequests(store.filterHostname ?? undefined);
    await loadHistory();
  }
</script>

<div class="flex h-8 items-center gap-1.5 border-b border-border bg-card/50 px-3 font-mono text-[9px]">
  <!-- Method chips -->
  {#each METHODS as method}
    <button
      class="rounded-sm px-1.5 py-[2px] text-[9px] font-medium transition-colors
             {isMethodActive(method) ? 'bg-foreground text-background' : 'text-muted-foreground hover:text-foreground'}"
      onclick={() => toggleMethod(method)}
    >
      {method}
    </button>
  {/each}

  <div class="h-3 w-px bg-border"></div>

  <!-- Error chip -->
  <button
    class="rounded-sm px-1.5 py-[2px] text-[9px] font-medium transition-colors
           {store.filterErrors ? 'bg-red-600 text-white' : 'text-muted-foreground hover:text-foreground'}"
    onclick={() => setFilterErrors(!store.filterErrors)}
  >
    4xx/5xx
  </button>

  <div class="h-3 w-px bg-border"></div>

  <!-- Content type chips -->
  {#each CONTENT_TYPES as ct}
    <button
      class="rounded-sm px-1.5 py-[2px] text-[9px] transition-colors
             {isContentActive(ct) ? 'bg-foreground text-background font-medium' : 'text-muted-foreground/50 hover:text-muted-foreground'}"
      onclick={() => setFilterContentType(ct === 'All' ? null : ct.toLowerCase())}
    >
      {ct}
    </button>
  {/each}

  <!-- Search -->
  <input
    class="ml-auto h-6 w-72 rounded-sm border border-border/50 bg-transparent px-2 font-mono text-[10px] text-foreground placeholder:text-muted-foreground/40 focus:outline-none focus:border-foreground/30"
    placeholder="Filter path, hostname..."
    oninput={(e) => setFilterSearch(e.currentTarget.value)}
    value={store.filterSearch}
  />

  <!-- Clear -->
  <button
    class="shrink-0 text-[9px] text-muted-foreground/40 hover:text-red-500 transition-colors"
    onclick={clearHistory}
  >
    clear
  </button>
</div>
