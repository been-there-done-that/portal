<script lang="ts">
  import { onMount } from 'svelte';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import ServiceCards from '$lib/components/ServiceCards.svelte';
  import FilterBar from '$lib/components/FilterBar.svelte';
  import RequestFeed from '$lib/components/RequestFeed.svelte';
  import RequestDetail from '$lib/components/RequestDetail.svelte';
  import { loadHistory, prependRequest } from '$lib/stores/requests.svelte.js';
  import { loadRoutes, trackRequest, routeStore } from '$lib/stores/routes.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  onMount(() => {
    loadHistory();
    loadRoutes();

    const routeInterval = setInterval(loadRoutes, 5000);

    const es = new EventSource('/api/stream');
    es.addEventListener('request', (e: MessageEvent) => {
      const meta: RequestMeta = JSON.parse(e.data);
      prependRequest(meta);
      trackRequest(meta.hostname, meta.duration_ms, meta.status);
    });
    es.onerror = () => {};

    return () => {
      clearInterval(routeInterval);
      es.close();
    };
  });
</script>

<svelte:head>
  <title>Portal</title>
</svelte:head>

<div class="flex h-full flex-col bg-background">
  <ServiceCards />
  <FilterBar />
  <RequestFeed />
  <RequestDetail />

  <!-- Floating version pill -->
  {#if routeStore.daemon}
    <div class="fixed bottom-3 right-3 z-40">
      <Badge variant="secondary" class="font-mono text-[10px] shadow-md backdrop-blur-sm bg-card/80 border border-border px-2.5 py-1">
        portal v{routeStore.daemon.version} · pid {routeStore.daemon.pid}
      </Badge>
    </div>
  {/if}
</div>
