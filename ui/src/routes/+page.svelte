<script lang="ts">
  import { onMount } from 'svelte';
  import ServiceCards from '$lib/components/ServiceCards.svelte';
  import FilterBar from '$lib/components/FilterBar.svelte';
  import RequestFeed from '$lib/components/RequestFeed.svelte';
  import RequestDetail from '$lib/components/RequestDetail.svelte';
  import StatusBar from '$lib/components/StatusBar.svelte';
  import { loadHistory, prependRequest } from '$lib/stores/requests.svelte.js';
  import { loadRoutes, trackRequest } from '$lib/stores/routes.svelte.js';
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
  <StatusBar />
  <RequestDetail />
</div>
