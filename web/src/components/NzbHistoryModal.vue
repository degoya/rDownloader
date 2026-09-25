<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NzbImport } from '@/api/types'
import { useNzbImportsStore } from '@/stores/nzbImports'
import { formatBytes, formatMoment } from '@/utils/format'

const emit = defineEmits<{ close: [] }>()

const nzb = useNzbImportsStore()
const { t } = useI18n()

const entries = computed(() => [...nzb.imports].sort((a, b) => b.created_at.localeCompare(a.created_at)))

const STATE_COLORS = { imported: 'info', enqueued: 'success', failed: 'error' } as const

async function remove(item: NzbImport): Promise<void> {
  // A 409 (import still backs a queue package) lands in nzb.error and is shown below.
  await nzb.remove(item.id)
}
</script>

<template>
  <UModal :title="t('linkgrabber.nzb.history.title')" :description="t('linkgrabber.nzb.history.description')" :close="{ onClick: () => emit('close') }">
    <template #body>
      <div class="space-y-3">
        <UAlert v-if="nzb.error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="nzb.error" />
        <p v-if="!entries.length" class="py-6 text-center text-sm text-muted">{{ t('linkgrabber.nzb.history.empty') }}</p>
        <ul v-else class="divide-y divide-muted">
          <li v-for="item in entries" :key="item.id" class="flex items-center gap-3 py-2">
            <UIcon name="i-lucide-file-archive" class="size-4 shrink-0 text-muted" />
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-highlighted" :title="item.name">{{ item.name }}</p>
              <p class="text-xs text-muted">
                {{ formatMoment(item.created_at) }} · {{ t('common.units.file', { count: item.file_count }, item.file_count) }} · {{ formatBytes(item.total_bytes) }}
              </p>
            </div>
            <UBadge :color="STATE_COLORS[item.state]" variant="subtle" size="sm">{{ t(`linkgrabber.nzb.state.${item.state}`) }}</UBadge>
            <UButton
              icon="i-lucide-trash-2"
              color="error"
              variant="ghost"
              size="xs"
              :aria-label="t('linkgrabber.actions.delete_nzb')"
              :title="item.state === 'enqueued' ? t('linkgrabber.nzb.history.enqueued_hint') : t('linkgrabber.actions.delete_nzb')"
              :loading="nzb.deletingIds.has(item.id)"
              @click="remove(item)"
            />
          </li>
        </ul>
        <p class="text-xs text-muted">{{ t('linkgrabber.nzb.history.hint') }}</p>
      </div>
    </template>
  </UModal>
</template>
