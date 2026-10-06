<script setup lang="ts">
/**
 * How Usenet files use the servers (RD-1120-21, from *General*): the connections one file may
 * take of each server and how many files run at once, beside the servers whose own connection
 * count is the ceiling of both.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings, UsenetServer } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
/**
 * Only to tell the person whether the per-file cap binds. `null` while the list is pending or
 * failed, which must not read as "no servers".
 */
const props = defineProps<{ servers: UsenetServer[] | null }>()
const { t } = useI18n()

/**
 * The cap applies per server, so it is measured against the largest enabled one, not the
 * sum. `null` while unknown or when no enabled server exists.
 */
const largestServer = computed(() => {
  const enabled = (props.servers ?? []).filter(server => server.enabled)
  if (!enabled.length) return null
  return Math.max(...enabled.map(server => server.max_connections))
})

/** The two connection numbers used to contradict each other silently (RD-108-25); this says which one applies. */
const nntpCapHint = computed(() => {
  const largest = largestServer.value
  if (largest === null) return null
  const cap = settings.value.nntp_connections_per_file
  if (cap === 0) return { text: t('settings.nntp_connections.follows_servers', { largest }), binding: false }
  if (cap < largest) return { text: t('settings.nntp_connections.capped', { cap, largest }), binding: true }
  return { text: t('settings.nntp_connections.not_binding', { largest }), binding: false }
})
</script>

<template>
  <UCard as="section" :ui="{ body: 'grid gap-4' }">
    <SectionHeader :eyebrow="t('settings.nntp_limits.eyebrow')" :title="t('settings.nntp_limits.title')" :description="t('settings.nntp_limits.description')" />
    <div>
      <UFormField data-settings-anchor="usenet.nntp_connections" :label="t('settings.nntp_connections.label')" :description="t('settings.nntp_connections.description')">
        <UInputNumber v-model="settings.nntp_connections_per_file" required :min="0" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <p v-if="nntpCapHint" class="mt-2 flex items-start gap-1.5 text-xs leading-5" :class="nntpCapHint.binding ? 'text-warning' : 'text-muted'" data-testid="nntp-cap-hint">
        <UIcon :name="nntpCapHint.binding ? 'i-lucide-triangle-alert' : 'i-lucide-info'" class="mt-0.5 size-3.5 shrink-0" />
        <span>{{ nntpCapHint.text }}</span>
      </p>
    </div>
    <UFormField data-settings-anchor="usenet.nntp_parallel_files" :label="t('settings.nntp_parallel_files.label')" :description="t('settings.nntp_parallel_files.description')">
      <UInputNumber v-model="settings.nntp_parallel_files" required :min="0" :max="8" :format-options="WHOLE" increment decrement class="mt-2 w-full" data-testid="nntp-parallel-files" />
    </UFormField>
  </UCard>
</template>
