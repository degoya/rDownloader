<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { BandwidthCapability, BandwidthStatus } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { formatBytes, formatLongMoment } from '@/utils/format'

const { t } = useI18n()
const status = ref<BandwidthStatus | null>(null)
const capabilities = ref<BandwidthCapability[]>([])
let timer: ReturnType<typeof setInterval> | null = null

/** Anything a limit cannot fully reach; shown so a setting is never silently ignored. */
const partial = computed(() =>
  capabilities.value.filter(entry => !entry.download_enforced || !entry.scoped_enforced)
)

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/bandwidth/status')
  if (response.data) status.value = response.data
}

function usage(used: string, limit: string | null | undefined): string {
  return limit
    ? t('bandwidth.status.usage', { used: formatBytes(used), limit: formatBytes(limit) })
    : formatBytes(used)
}

onMounted(async () => {
  await load()
  const response = await api.GET('/api/v1/bandwidth/capabilities')
  if (response.data) capabilities.value = response.data
  timer = setInterval(() => void load(), 10_000)
})
onUnmounted(() => {
  if (timer) clearInterval(timer)
})

defineExpose({ reload: load })
</script>

<template>
  <section v-if="status" class="border border-muted bg-default p-5">
    <SectionHeader
      :eyebrow="t('bandwidth.status.eyebrow')"
      :title="status.active_profile?.name ?? t('bandwidth.status.no_profile')"
    />
    <dl class="mt-4 grid gap-4 sm:grid-cols-2">
      <div>
        <dt class="text-xs text-muted">{{ t('bandwidth.status.binding') }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">
          <template v-if="status.binding_limit">
            {{ formatBytes(status.binding_limit.bytes_per_second) }}/s
            <span class="text-muted">· {{ t(`bandwidth.source.${status.binding_limit.source}`) }}</span>
          </template>
          <template v-else>{{ t('bandwidth.status.unlimited') }}</template>
        </dd>
      </div>
      <div>
        <dt class="text-xs text-muted">{{ t('bandwidth.status.next_switch') }}</dt>
        <dd class="mt-1 text-sm text-highlighted">
          {{ formatLongMoment(status.next_switch_at) || t('bandwidth.status.no_switch') }}
          <span class="text-muted">· {{ status.timezone }}</span>
        </dd>
      </div>
      <div v-if="status.daily">
        <dt class="text-xs text-muted">{{ t('bandwidth.status.daily', { period: status.daily.period_key }) }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">{{ usage(status.daily.used_bytes, status.daily.limit_bytes) }}</dd>
      </div>
      <div v-if="status.monthly">
        <dt class="text-xs text-muted">{{ t('bandwidth.status.monthly', { period: status.monthly.period_key }) }}</dt>
        <dd class="numeric mt-1 text-sm text-highlighted">{{ usage(status.monthly.used_bytes, status.monthly.limit_bytes) }}</dd>
      </div>
    </dl>
    <UAlert
      v-if="status.budget_exhausted"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-gauge"
      :description="t('bandwidth.status.budget_exhausted')"
    />
    <div v-if="partial.length" class="mt-4 border-t border-muted pt-4">
      <p class="text-xs font-medium text-highlighted">{{ t('bandwidth.capabilities.title') }}</p>
      <ul class="mt-2 space-y-1">
        <li v-for="entry in partial" :key="entry.kind" class="flex items-start gap-1.5 text-xs text-muted">
          <UIcon name="i-lucide-info" class="mt-0.5 size-3.5 shrink-0" />
          <span>
            <span class="font-medium text-highlighted">{{ t(`bandwidth.kinds.${entry.kind}`) }}</span>
            — {{ entry.note }}
          </span>
        </li>
      </ul>
    </div>
  </section>
</template>
