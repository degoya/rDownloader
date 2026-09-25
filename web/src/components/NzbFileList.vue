<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { NzbFileStatus } from '@/api/types'

const { t } = useI18n()
const props = defineProps<{ importId: string }>()
const files = ref<NzbFileStatus[]>([])
const loading = ref(true)

onMounted(async () => {
  const response = await api.GET('/api/v1/nzb/imports/{id}/files', { params: { path: { id: props.importId } } })
  files.value = response.data ?? []
  loading.value = false
})

function completed(file: NzbFileStatus): number {
  return file.segments.filter(segment => segment.state === 'completed').length
}
function failed(file: NzbFileStatus): number {
  return file.segments.filter(segment => segment.state === 'failed').length
}
</script>

<!--
  Segment state, and only that (RD-109-30).

  This panel sits directly above the package's own download rows, which carry the same file
  names — so it may only show what those rows cannot. Two things qualify: the raw NZB subject,
  with the poster's `[n/m]` ordering and the yEnc part spec, and the per-file segment tally
  including the ones that never arrived.

  The size column is gone, and not because it looked repetitive. `NzbFileStatus.total_bytes` is
  the sum of the `<segment bytes>` attributes, which is the *posted* article size including the
  yEnc overhead; the download row underneath shows the decoded file. The same file therefore
  stood as "722 MiB" here and "699 MiB" one line down, with nothing on screen explaining the
  gap — a figure that reads as a contradiction is worse than one that is merely duplicated.
-->
<template>
  <div class="divide-y divide-muted text-xs">
    <p v-if="loading" class="py-2 text-muted">{{ t('downloads.nzb.loading') }}</p>
    <div v-for="file in files" :key="file.id" class="flex items-center gap-3 py-1.5">
      <p class="min-w-0 flex-1 truncate text-highlighted" :title="file.subject">{{ file.subject }}</p>
      <span class="numeric shrink-0 text-muted" :title="t('downloads.nzb.segments_title')">{{ completed(file) }}/{{ file.segments.length }}<span v-if="failed(file)" class="text-error"> · {{ t('downloads.nzb.missing', { count: failed(file) }) }}</span></span>
    </div>
  </div>
</template>
