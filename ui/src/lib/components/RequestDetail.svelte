<script lang="ts">
  import * as Sheet from '$lib/components/ui/sheet/index.js';
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { store, clearSelected } from '$lib/stores/requests.svelte.js';

  function tryFormatJson(text: string): string {
    try {
      return JSON.stringify(JSON.parse(text), null, 2);
    } catch {
      return text;
    }
  }

  function isJson(headers: [string, string][]): boolean {
    return headers.some(
      ([k, v]) => k.toLowerCase() === 'content-type' && v.includes('json')
    );
  }

  function statusVariant(status: number): 'default' | 'secondary' | 'destructive' | 'outline' {
    if (status >= 500) return 'destructive';
    if (status >= 400) return 'outline';
    return 'secondary';
  }

  let sheetOpen = $derived(store.selectedId !== null);

  function handleOpenChange(open: boolean) {
    if (!open) clearSelected();
  }
</script>

<Sheet.Root open={sheetOpen} onOpenChange={handleOpenChange}>
  <Sheet.Content side="right" class="w-[420px] sm:max-w-[420px] flex flex-col p-0 gap-0" showCloseButton={false}>
    {#if !store.selectedRecord}
      <div class="flex flex-1 items-center justify-center font-mono text-xs text-muted-foreground">
        Loading...
      </div>
    {:else}
      <!-- Header -->
      <Sheet.Header class="border-b border-border px-4 py-3 gap-1">
        <div class="flex items-center gap-2">
          <Badge variant="outline" class="font-mono text-[11px]">{store.selectedRecord.method}</Badge>
          <Sheet.Title class="flex-1 truncate font-mono text-sm">
            {store.selectedRecord.path}
          </Sheet.Title>
          <Badge variant={statusVariant(store.selectedRecord.status)} class="font-mono text-[11px]">
            {store.selectedRecord.status}
          </Badge>
        </div>
        <Sheet.Description class="font-mono text-[10px]">
          {store.selectedRecord.hostname} · {store.selectedRecord.duration_ms}ms
        </Sheet.Description>
      </Sheet.Header>

      <!-- Tabs -->
      <Tabs.Root value="request" class="flex flex-1 flex-col overflow-hidden">
        <Tabs.List variant="line" class="mx-4 mt-2 w-fit">
          {#each ['request', 'response', 'headers', 'timing'] as tab}
            <Tabs.Trigger value={tab} class="font-mono text-[11px] capitalize">
              {tab}
            </Tabs.Trigger>
          {/each}
        </Tabs.List>

        <!-- Request body -->
        <Tabs.Content value="request" class="flex-1 overflow-hidden p-0">
          <ScrollArea class="h-full px-4 py-3">
            {#if store.selectedRecord.req_truncated}
              <p class="mb-2 font-mono text-[10px] text-orange-500">
                Body truncated — showing first 1 MB of {store.selectedRecord.req_total_bytes.toLocaleString()} bytes
              </p>
            {/if}
            <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(store.selectedRecord.req_headers) ? tryFormatJson(store.selectedRecord.req_body) : store.selectedRecord.req_body || '(empty)'}</pre>
          </ScrollArea>
        </Tabs.Content>

        <!-- Response body -->
        <Tabs.Content value="response" class="flex-1 overflow-hidden p-0">
          <ScrollArea class="h-full px-4 py-3">
            {#if store.selectedRecord.res_truncated}
              <p class="mb-2 font-mono text-[10px] text-orange-500">
                Body truncated — showing first 1 MB of {store.selectedRecord.res_total_bytes.toLocaleString()} bytes
              </p>
            {/if}
            <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(store.selectedRecord.res_headers) ? tryFormatJson(store.selectedRecord.res_body) : store.selectedRecord.res_body || '(empty)'}</pre>
          </ScrollArea>
        </Tabs.Content>

        <!-- Headers -->
        <Tabs.Content value="headers" class="flex-1 overflow-hidden p-0">
          <ScrollArea class="h-full px-4 py-3">
            <p class="mb-2 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Request</p>
            {#each store.selectedRecord.req_headers as [key, value]}
              <div class="mb-1 flex gap-2 font-mono text-[11px]">
                <span class="w-48 flex-shrink-0 text-blue-400">{key}</span>
                <span class="break-all text-muted-foreground">{value}</span>
              </div>
            {/each}
            <p class="mb-2 mt-4 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Response</p>
            {#each store.selectedRecord.res_headers as [key, value]}
              <div class="mb-1 flex gap-2 font-mono text-[11px]">
                <span class="w-48 flex-shrink-0 text-green-400">{key}</span>
                <span class="break-all text-muted-foreground">{value}</span>
              </div>
            {/each}
          </ScrollArea>
        </Tabs.Content>

        <!-- Timing -->
        <Tabs.Content value="timing" class="flex-1 overflow-hidden p-0">
          <ScrollArea class="h-full px-4 py-3">
            <div class="space-y-2 font-mono text-[11px]">
              <div class="flex justify-between">
                <span class="text-muted-foreground">Total duration</span>
                <span class="text-foreground">{store.selectedRecord.duration_ms}ms</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">Timestamp</span>
                <span class="text-foreground">{new Date(store.selectedRecord.timestamp).toISOString()}</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">Request size</span>
                <span class="text-foreground">{store.selectedRecord.req_total_bytes} bytes</span>
              </div>
              <div class="flex justify-between">
                <span class="text-muted-foreground">Response size</span>
                <span class="text-foreground">{store.selectedRecord.res_total_bytes} bytes</span>
              </div>
            </div>
          </ScrollArea>
        </Tabs.Content>
      </Tabs.Root>
    {/if}
  </Sheet.Content>
</Sheet.Root>
