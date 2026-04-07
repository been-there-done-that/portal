<script lang="ts">
  import { Button } from '$lib/components/ui/button/index.js';
  import { Separator } from '$lib/components/ui/separator/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import {
    store,
    loadHistory,
    setFilterHostname,
    setFilterMethods,
    setFilterErrors,
  } from '$lib/stores/requests.svelte.js';
  import { deleteAllRequests } from '$lib/api.js';

  const METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE'];

  function toggleMethod(m: string) {
    const next = new Set(store.filterMethods);
    next.has(m) ? next.delete(m) : next.add(m);
    setFilterMethods(next);
  }

  async function clearHistory() {
    await deleteAllRequests(store.filterHostname ?? undefined);
    await loadHistory();
  }
</script>

<aside class="flex h-full w-[200px] flex-shrink-0 flex-col border-r border-border bg-card font-mono text-xs">
  <!-- Routes -->
  <div class="px-3 pt-4 pb-2">
    <p class="mb-2 text-[10px] uppercase tracking-widest text-muted-foreground">Routes</p>

    <button
      class="w-full rounded px-2 py-1.5 text-left transition-colors {store.filterHostname === null
        ? 'bg-accent text-accent-foreground'
        : 'hover:bg-accent/50 text-muted-foreground'}"
      onclick={() => { setFilterHostname(null); }}
    >
      All routes
    </button>

    {#each store.hostnames as hostname}
      <button
        class="w-full rounded px-2 py-1.5 text-left transition-colors {store.filterHostname === hostname
          ? 'bg-accent text-accent-foreground'
          : 'hover:bg-accent/50 text-muted-foreground'}"
        onclick={() => { setFilterHostname(hostname); }}
      >
        {hostname}
      </button>
    {/each}
  </div>

  <Separator />

  <!-- Filters -->
  <div class="px-3 py-3">
    <p class="mb-2 text-[10px] uppercase tracking-widest text-muted-foreground">Filter</p>
    <div class="flex flex-wrap gap-1.5">
      {#each METHODS as method}
        <button onclick={() => toggleMethod(method)}>
          <Badge
            variant={store.filterMethods.has(method) ? 'default' : 'outline'}
            class="cursor-pointer px-2 py-0.5 text-[10px]"
          >
            {method}
          </Badge>
        </button>
      {/each}
      <button onclick={() => { setFilterErrors(!store.filterErrors); }}>
        <Badge
          variant={store.filterErrors ? 'destructive' : 'outline'}
          class="cursor-pointer px-2 py-0.5 text-[10px]"
        >
          errors
        </Badge>
      </button>
    </div>
  </div>

  <!-- Clear -->
  <div class="mt-auto border-t border-border px-3 py-3">
    <Button variant="ghost" size="sm" class="w-full text-destructive hover:text-destructive text-[11px]" onclick={clearHistory}>
      Clear history
    </Button>
  </div>
</aside>
