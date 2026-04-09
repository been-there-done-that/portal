<script lang="ts">
  import { routeStore } from '$lib/stores/routes.svelte.js';
  import { store } from '$lib/stores/requests.svelte.js';

  function formatUptime(secs: number): string {
    const h = Math.floor(secs / 3600);
    const m = Math.floor((secs % 3600) / 60);
    if (h > 0) return `${h}h ${m}m`;
    return `${m}m`;
  }

  const filterActive = $derived(
    store.filterMethods.size > 0 || store.filterErrors || store.filterHostname !== null
  );
</script>

<div class="flex h-7 items-center justify-between border-t border-border bg-card px-3 font-mono text-[10px] text-muted-foreground">
  <!-- Left: daemon info -->
  <div class="flex items-center gap-1.5">
    {#if routeStore.daemon}
      <span class="font-semibold text-foreground">portal</span>
      <span class="text-muted-foreground/50">·</span>
      <span>v{routeStore.daemon.version}</span>
      <span class="text-muted-foreground/50">·</span>
      <span>pid {routeStore.daemon.pid}</span>
      <span class="text-muted-foreground/50">·</span>
      <span>uptime {formatUptime(routeStore.daemon.uptime_secs)}</span>
    {:else}
      <span class="font-semibold text-foreground">portal</span>
    {/if}
  </div>

  <!-- Right: request count + filter context -->
  <div class="flex items-center gap-1.5">
    {#if filterActive}
      <span class="text-muted-foreground/60">(filtered)</span>
      <span class="text-muted-foreground/50">·</span>
    {/if}
    <span>{store.filtered.length} requests</span>
  </div>
</div>
