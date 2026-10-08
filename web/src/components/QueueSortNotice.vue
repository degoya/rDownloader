<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { QueueSort, QueueSortColumn } from '@/composables/useQueueSort'

/**
 * The notice above a download list sorted for the eye (RD-1190-16): what it is sorted by, that
 * the queue itself runs in its own order, that dragging is off meanwhile, and the way back.
 */
const props = defineProps<{
  sort: QueueSort
}>()

const emit = defineEmits<{
  reset: []
}>()

const { t } = useI18n()

const COLUMN_KEYS: Record<QueueSortColumn, string> = {
  name: 'common.queue_columns.name',
  state: 'common.queue_columns.state',
  progress: 'common.queue_columns.progress',
  size: 'common.queue_columns.size',
  meta: 'downloads.view_sort.category'
}

const description = computed(() => t('downloads.view_sort.description', {
  column: t(COLUMN_KEYS[props.sort.column]),
  direction: t(`downloads.view_sort.${props.sort.direction}`)
}))
</script>

<template>
  <UAlert
    color="info"
    variant="subtle"
    icon="i-lucide-arrow-down-up"
    :title="t('downloads.view_sort.title')"
    :description="description"
    data-testid="queue-sort-notice"
  >
    <template #actions>
      <UButton :label="t('downloads.view_sort.reset')" icon="i-lucide-list-restart" color="neutral" variant="outline" size="xs" @click="emit('reset')" />
    </template>
  </UAlert>
</template>
