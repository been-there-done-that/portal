<script lang="ts">
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { Button } from '$lib/components/ui/button/index.js';
  import { stopRoute } from '$lib/api.js';
  import { loadRoutes } from '$lib/stores/routes.svelte.js';
  import type { RouteInfo } from '$lib/api.js';

  let {
    route,
    requestCount,
    latencySamples,
    errorCount,
    active,
    onclick,
  }: {
    route: RouteInfo;
    requestCount: number;
    latencySamples: number[];
    errorCount: number;
    active: boolean;
    onclick: () => void;
  } = $props();

  const serviceName = $derived(route.hostname.replace(/\.localhost$/, ''));
  const isAlias = $derived(route.pid === 0);
  const badgeType = $derived(isAlias ? 'ALIAS' : route.protocol === 'tcp' ? 'TCP' : 'HTTP');

  const badgeClass = $derived(
    badgeType === 'HTTP'
      ? 'bg-green-950 text-green-500 border-green-900'
      : badgeType === 'ALIAS'
        ? 'bg-indigo-950 text-indigo-400 border-indigo-900'
        : 'bg-amber-950 text-amber-500 border-amber-900'
  );

  const sparklineColor = $derived(
    badgeType === 'HTTP'
      ? 'bg-green-500'
      : badgeType === 'ALIAS'
        ? 'bg-indigo-400'
        : 'bg-amber-500'
  );

  const avgLatency = $derived(
    latencySamples.length > 0
      ? Math.round(latencySamples.reduce((a, b) => a + b, 0) / latencySamples.length)
      : 0
  );

  const maxSample = $derived(latencySamples.length > 0 ? Math.max(...latencySamples) : 1);

  const url = $derived(`https://${route.hostname}`);

  async function handleStop(e: MouseEvent) {
    e.stopPropagation();
    await stopRoute(route.hostname);
    await loadRoutes();
  }

  function handleCopy(e: MouseEvent) {
    e.stopPropagation();
    navigator.clipboard.writeText(url);
  }

  function handleOpen(e: MouseEvent) {
    e.stopPropagation();
    window.open(url);
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  role="button"
  tabindex="0"
  class="group relative flex min-w-[200px] flex-1 cursor-pointer flex-col gap-1.5 rounded-md border p-3 text-left transition-colors
    {active
      ? 'border-blue-500 bg-blue-950/20'
      : 'border-border bg-card hover:border-border/80 hover:bg-accent/30'}"
  onclick={onclick}
  onkeydown={(e) => e.key === 'Enter' && onclick()}
>
  <!-- Header row -->
  <div class="flex items-start justify-between gap-2">
    <span class="truncate font-mono text-xs font-medium text-foreground">{serviceName}</span>
    <Badge class="shrink-0 border px-1.5 py-0 font-mono text-[10px] {badgeClass}">
      {badgeType}
    </Badge>
  </div>

  <!-- Meta row -->
  <div class="font-mono text-[10px] text-muted-foreground">
    {#if isAlias}
      <span class="truncate">{route.hostname}</span>
    {:else}
      <span>.localhost · :{route.port} · pid {route.pid}</span>
    {/if}
  </div>

  <!-- Stats or alias hint -->
  {#if isAlias}
    <div class="mt-0.5 font-mono text-[10px] text-muted-foreground/60 italic">
      alias → :{route.port}
    </div>
  {:else}
    <div class="mt-0.5 flex items-end gap-3">
      <div class="flex flex-col">
        <span class="font-mono text-[10px] text-muted-foreground/60">req</span>
        <span class="font-mono text-sm font-semibold leading-tight text-foreground">
          {requestCount}
        </span>
      </div>
      <div class="flex flex-col">
        <span class="font-mono text-[10px] text-muted-foreground/60">avg</span>
        <span class="font-mono text-sm font-semibold leading-tight text-foreground">
          {avgLatency}<span class="text-[9px] text-muted-foreground">ms</span>
        </span>
      </div>
      <div class="flex flex-col">
        <span class="font-mono text-[10px] text-muted-foreground/60">err</span>
        <span class="font-mono text-sm font-semibold leading-tight {errorCount > 0 ? 'text-destructive' : 'text-foreground'}">
          {errorCount}
        </span>
      </div>
    </div>

    <!-- Sparkline -->
    {#if latencySamples.length > 0}
      <div class="mt-1 flex h-5 items-end gap-px">
        {#each latencySamples as sample}
          <div
            class="w-[3px] rounded-sm {sparklineColor} opacity-70"
            style="height: {Math.max(2, Math.round((sample / maxSample) * 20))}px"
          ></div>
        {/each}
      </div>
    {:else}
      <div class="mt-1 h-5"></div>
    {/if}
  {/if}

  <!-- Actions (visible on hover) -->
  <div class="absolute right-2 top-2 flex gap-1 opacity-0 transition-opacity group-hover:opacity-100">
    <button
      class="rounded px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
      title="Open in browser"
      onclick={handleOpen}
    >
      ↗
    </button>
    <button
      class="rounded px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground hover:bg-accent hover:text-foreground transition-colors"
      title="Copy URL"
      onclick={handleCopy}
    >
      ⎘
    </button>
    <button
      class="rounded px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground hover:bg-destructive/20 hover:text-destructive transition-colors"
      title="Stop route"
      onclick={handleStop}
    >
      ✕
    </button>
  </div>
</div>
