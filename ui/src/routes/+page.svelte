<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from '$lib/components/Sidebar.svelte';
  import RequestFeed from '$lib/components/RequestFeed.svelte';
  import RequestDetail from '$lib/components/RequestDetail.svelte';
  import { loadHistory, prependRequest } from '$lib/stores/requests.svelte.js';
  import type { RequestMeta } from '$lib/api.js';

  onMount(() => {
    // Load history on mount
    loadHistory();

    // Connect SSE for live updates
    const es = new EventSource('/api/stream');
    es.addEventListener('request', (e: MessageEvent) => {
      const meta: RequestMeta = JSON.parse(e.data);
      prependRequest(meta);
    });
    es.onerror = () => {
      // SSE will auto-reconnect; no action needed
    };

    return () => {
      es.close();
    };
  });
</script>

<svelte:head>
  <title>Portal Inspector</title>
</svelte:head>

<div class="flex h-full">
  <Sidebar />
  <RequestFeed />
  <RequestDetail />
</div>
