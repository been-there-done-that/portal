<script lang="ts">
  import * as Tabs from '$lib/components/ui/tabs/index.js';
  import { Badge } from '$lib/components/ui/badge/index.js';
  import { ScrollArea } from '$lib/components/ui/scroll-area/index.js';
  import { selectedRecord, selectedId } from '$lib/stores/requests.svelte.js';

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
</script>

<div class="flex flex-1 flex-col overflow-hidden bg-background">
  {#if !selectedId}
    <div class="flex flex-1 items-center justify-center font-mono text-xs text-muted-foreground">
      Select a request to inspect
    </div>
  {:else if !selectedRecord}
    <div class="flex flex-1 items-center justify-center font-mono text-xs text-muted-foreground">
      Loading…
    </div>
  {:else}
    <!-- Header -->
    <div class="border-b border-border px-4 py-3">
      <div class="flex items-center gap-2">
        <Badge variant="outline" class="font-mono text-[11px]">{selectedRecord.method}</Badge>
        <span class="flex-1 truncate font-mono text-sm text-foreground">{selectedRecord.path}</span>
        <Badge variant={statusVariant(selectedRecord.status)} class="font-mono text-[11px]">
          {selectedRecord.status}
        </Badge>
      </div>
      <p class="mt-1 font-mono text-[10px] text-muted-foreground">
        {selectedRecord.hostname} · {selectedRecord.duration_ms}ms
      </p>
    </div>

    <!-- Tabs -->
    <Tabs.Root value="request" class="flex flex-1 flex-col overflow-hidden">
      <Tabs.List class="mx-4 mt-2 w-fit rounded-none border-b border-border bg-transparent p-0">
        {#each ['request', 'response', 'headers', 'timing'] as tab}
          <Tabs.Trigger
            value={tab}
            class="rounded-none border-b-2 border-transparent px-3 py-1.5 font-mono text-[11px] capitalize
                   data-[state=active]:border-primary data-[state=active]:text-foreground"
          >
            {tab}
          </Tabs.Trigger>
        {/each}
      </Tabs.List>

      <!-- Request body -->
      <Tabs.Content value="request" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          {#if selectedRecord.req_truncated}
            <p class="mb-2 font-mono text-[10px] text-orange-500">
              Body truncated — showing first 1 MB of {selectedRecord.req_total_bytes.toLocaleString()} bytes
            </p>
          {/if}
          <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(selectedRecord.req_headers) ? tryFormatJson(selectedRecord.req_body) : selectedRecord.req_body || '(empty)'}</pre>
        </ScrollArea>
      </Tabs.Content>

      <!-- Response body -->
      <Tabs.Content value="response" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          {#if selectedRecord.res_truncated}
            <p class="mb-2 font-mono text-[10px] text-orange-500">
              Body truncated — showing first 1 MB of {selectedRecord.res_total_bytes.toLocaleString()} bytes
            </p>
          {/if}
          <pre class="whitespace-pre-wrap font-mono text-[11px] text-muted-foreground">{isJson(selectedRecord.res_headers) ? tryFormatJson(selectedRecord.res_body) : selectedRecord.res_body || '(empty)'}</pre>
        </ScrollArea>
      </Tabs.Content>

      <!-- Headers -->
      <Tabs.Content value="headers" class="flex-1 overflow-hidden p-0">
        <ScrollArea class="h-full px-4 py-3">
          <p class="mb-2 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Request</p>
          {#each selectedRecord.req_headers as [key, value]}
            <div class="mb-1 flex gap-2 font-mono text-[11px]">
              <span class="w-48 flex-shrink-0 text-blue-400">{key}</span>
              <span class="break-all text-muted-foreground">{value}</span>
            </div>
          {/each}
          <p class="mb-2 mt-4 font-mono text-[10px] uppercase tracking-widest text-muted-foreground">Response</p>
          {#each selectedRecord.res_headers as [key, value]}
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
              <span class="text-foreground">{selectedRecord.duration_ms}ms</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Timestamp</span>
              <span class="text-foreground">{new Date(selectedRecord.timestamp).toISOString()}</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Request size</span>
              <span class="text-foreground">{selectedRecord.req_total_bytes} bytes</span>
            </div>
            <div class="flex justify-between">
              <span class="text-muted-foreground">Response size</span>
              <span class="text-foreground">{selectedRecord.res_total_bytes} bytes</span>
            </div>
          </div>
        </ScrollArea>
      </Tabs.Content>
    </Tabs.Root>
  {/if}
</div>
