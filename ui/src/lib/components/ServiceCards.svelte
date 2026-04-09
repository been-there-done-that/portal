<script lang="ts">
  import ServiceCard from './ServiceCard.svelte';
  import { routeStore, setSelectedHostname } from '$lib/stores/routes.svelte.js';
  import { setFilterHostname } from '$lib/stores/requests.svelte.js';

  function toggleFilter(hostname: string) {
    if (routeStore.selectedHostname === hostname) {
      setSelectedHostname(null);
      setFilterHostname(null);
    } else {
      setSelectedHostname(hostname);
      setFilterHostname(hostname);
    }
  }
</script>

<div class="shrink-0 border-b border-border px-5 py-3">
  <div class="mb-2 flex items-center justify-between">
    <h2 class="text-xs font-medium text-muted-foreground">Services</h2>
    <span class="text-[10px] text-muted-foreground/50">click to filter · hover for actions</span>
  </div>
  <div class="flex gap-2 overflow-x-auto pb-1">
    {#each routeStore.routes as route (route.hostname)}
      <ServiceCard
        {route}
        requestCount={routeStore.requestCounts[route.hostname] ?? 0}
        latencySamples={routeStore.latencySamples[route.hostname] ?? []}
        errorCount={routeStore.errorCounts[route.hostname] ?? 0}
        active={routeStore.selectedHostname === route.hostname}
        onclick={() => toggleFilter(route.hostname)}
      />
    {/each}
    <div class="flex min-w-[120px] items-center justify-center rounded-md border border-dashed border-border">
      <div class="py-4 px-3 text-center">
        <div class="text-lg text-muted-foreground/30">+</div>
        <div class="font-mono text-[9px] text-muted-foreground/40">portal run<br>portal alias</div>
      </div>
    </div>
  </div>
</div>
