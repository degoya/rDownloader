<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { PostprocessQueueEntry } from '@/api/types'
import { postprocessStageLabel } from '@/utils/format'

const props = defineProps<{ entries: PostprocessQueueEntry[] }>()
const { t } = useI18n()

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}
</script>

<template>
  <section class="border border-primary/40 bg-elevated">
    <header class="flex items-center gap-2 border-b border-muted px-3 py-2">
      <UIcon name="i-lucide-workflow" class="size-4 text-primary" />
      <p class="text-sm font-semibold text-highlighted">{{ t('downloads.postprocess.queue.title') }}</p>
      <UBadge color="neutral" variant="outline" size="sm" class="numeric">{{ props.entries.length }}</UBadge>
    </header>
    <ul class="divide-y divide-muted">
      <li v-for="entry in props.entries" :key="entry.package_id" class="grid gap-x-3 gap-y-1 px-3 py-2 sm:grid-cols-[minmax(0,2fr)_auto_minmax(8rem,1fr)_auto] sm:items-center">
        <div class="min-w-0">
          <p class="truncate text-sm font-medium text-highlighted" :title="entry.name">{{ entry.name }}</p>
          <p v-if="entry.current" class="truncate font-mono text-[11px] text-muted" :title="entry.current">{{ fileName(entry.current) }}</p>
        </div>
        <div class="flex items-center gap-1">
          <UBadge v-if="entry.pending" color="neutral" variant="subtle" size="sm">{{ t('downloads.postprocess.queue.pending') }}</UBadge>
          <UBadge v-else-if="entry.stage" color="primary" variant="subtle" size="sm">{{ postprocessStageLabel(entry.stage) }}</UBadge>
          <UBadge v-if="entry.state === 'failed'" color="error" variant="subtle" size="sm">{{ t('downloads.postprocess.states.failed') }}</UBadge>
        </div>
        <UProgress :model-value="entry.pending ? null : entry.percent ?? null" size="xs" :color="entry.state === 'failed' ? 'error' : 'primary'" class="w-full" />
        <span class="numeric w-10 text-right text-[11px] text-toned">{{ entry.percent !== null && entry.percent !== undefined && !entry.pending ? `${entry.percent} %` : '—' }}</span>
      </li>
    </ul>
  </section>
</template>
