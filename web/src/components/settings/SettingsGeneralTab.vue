<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Settings, UsenetServer } from '@/api/types'
import { GIB, byteModel } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const speedMiB = defineModel<number | null>('speedMib', { required: true })
const { t } = useI18n()

// Only to tell the person whether the per-file cap binds; the servers are edited elsewhere.
// `null` until the list has arrived - a failed or pending fetch must not read as "no servers".
const usenetServers = ref<UsenetServer[] | null>(null)
onMounted(async () => {
  try {
    const response = await api.GET('/api/v1/usenet/servers')
    if (response.data) usenetServers.value = response.data
  } catch {
    // The hint is a courtesy; without the list the field keeps its plain description.
  }
})

/**
 * The cap applies per server, so it is measured against the largest enabled one, not the
 * sum. `null` while unknown or when no enabled server exists.
 */
const largestServer = computed(() => {
  const enabled = (usenetServers.value ?? []).filter(server => server.enabled)
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

/** The free-space threshold is entered in GiB; the API stores raw bytes. */
const minimumFreeGiB = byteModel(
  () => settings.value.storage_minimum_free_bytes,
  (raw) => { settings.value.storage_minimum_free_bytes = raw ?? '0' },
  GIB,
  '0'
)

/** Empty input clears the override; the backend then keeps the built-in default port. */
const uiPort = computed<number | null>({
  get: () => settings.value.ui_port ?? null,
  set: (value) => {
    const port = Number(value)
    settings.value.ui_port = Number.isFinite(port) && port > 0 ? Math.round(port) : null
  }
})
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.general.eyebrow')"
        :title="t('settings.headers.general.title')"
        :description="t('settings.headers.general.description')"
        level="page"
      />
    </header>
    <section class="grid gap-4 border border-muted bg-default p-5 md:grid-cols-2">
      <UFormField :label="t('settings.active_files.label')" :description="t('settings.active_files.description')">
        <UInput v-model.number="settings.max_active_files" type="number" min="1" max="32" icon="i-lucide-files" class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('settings.chunks.label')" :description="t('settings.chunks.description')">
        <UInput v-model.number="settings.max_chunks_per_file" type="number" min="1" max="16" icon="i-lucide-split" class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('settings.connections_per_host.label')" :description="t('settings.connections_per_host.description')">
        <UInput v-model.number="settings.max_connections_per_host" type="number" min="0" max="32" icon="i-lucide-share-2" class="mt-2 w-full" />
      </UFormField>
      <div>
        <UFormField :label="t('settings.nntp_connections.label')" :description="t('settings.nntp_connections.description')">
          <UInput v-model.number="settings.nntp_connections_per_file" type="number" min="0" max="32" icon="i-lucide-network" class="mt-2 w-full" />
        </UFormField>
        <p v-if="nntpCapHint" class="mt-2 flex items-start gap-1.5 text-xs leading-5" :class="nntpCapHint.binding ? 'text-warning' : 'text-muted'" data-testid="nntp-cap-hint">
          <UIcon :name="nntpCapHint.binding ? 'i-lucide-triangle-alert' : 'i-lucide-info'" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ nntpCapHint.text }}</span>
        </p>
      </div>
      <UFormField :label="t('settings.speed_limit.label')" :description="t('settings.speed_limit.description')">
        <UInput v-model.number="speedMiB" type="number" min="0" step="0.5" icon="i-lucide-gauge" class="mt-2 w-full">
          <template #trailing><span class="font-mono text-xs text-muted">MiB/s</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('settings.retries.label')" :description="t('settings.retries.description')">
        <UInput v-model.number="settings.max_retries" type="number" min="0" max="100" icon="i-lucide-repeat" class="mt-2 w-full" />
      </UFormField>
      <div>
        <UFormField :label="t('settings.ui_port.label')" :description="t('settings.ui_port.description')">
          <UInput v-model.number="uiPort" type="number" min="1024" max="65535" icon="i-lucide-plug" :placeholder="t('settings.ui_port.placeholder')" class="mt-2 w-full" />
        </UFormField>
        <p class="mt-2 flex items-start gap-1.5 text-xs leading-5 text-warning">
          <UIcon name="i-lucide-rotate-cw" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ t('settings.ui_port.restart_hint') }}</span>
        </p>
      </div>
      <div class="flex items-center justify-between gap-5 border-t border-muted pt-4 md:col-span-2">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.mirrors.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.mirrors.description') }}</p>
        </div>
        <USwitch v-model="settings.mirror_detection" :aria-label="t('settings.mirrors.label')" />
      </div>
      <div class="border-t border-muted pt-4 md:col-span-2">
        <div class="flex items-center justify-between gap-5">
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('settings.auto_remove.label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.auto_remove.description') }}</p>
          </div>
          <USwitch v-model="settings.auto_remove_finished" :aria-label="t('settings.auto_remove.label')" />
        </div>
        <div v-if="settings.auto_remove_finished" class="mt-4 grid gap-4 md:grid-cols-2">
          <UFormField :label="t('settings.auto_remove.delay_label')" :description="t('settings.auto_remove.delay_description')">
            <UInput v-model.number="settings.auto_remove_delay_hours" type="number" min="1" max="720" icon="i-lucide-timer" class="mt-2 w-full">
              <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
            </UInput>
          </UFormField>
          <div class="flex items-center justify-between gap-5 self-end pb-1">
            <div>
              <p class="text-sm font-medium text-highlighted">{{ t('settings.auto_remove.keep_failed_label') }}</p>
              <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.auto_remove.keep_failed_description') }}</p>
            </div>
            <USwitch v-model="settings.auto_remove_keep_failed" :aria-label="t('settings.auto_remove.keep_failed_label')" />
          </div>
        </div>
      </div>
      <div class="flex items-center justify-between gap-5 border-t border-muted pt-4 md:col-span-2">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.import_history.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.import_history.description') }}</p>
        </div>
        <USwitch v-model="settings.keep_import_history" :aria-label="t('settings.import_history.label')" />
      </div>
      <div class="flex items-center justify-between gap-5 border-t border-muted pt-4 md:col-span-2">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.sha256.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.sha256.description') }}</p>
        </div>
        <USwitch v-model="settings.generate_sha256" :aria-label="t('settings.sha256.label')" />
      </div>
      <div class="border-t border-muted pt-4 md:col-span-2">
        <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.title') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.description') }}</p>
      </div>
      <UFormField :label="t('settings.storage.minimum_free.label')" :description="t('settings.storage.minimum_free.description')">
        <UInput v-model.number="minimumFreeGiB" type="number" min="0" step="1" icon="i-lucide-shield-check" class="mt-2 w-full">
          <template #trailing><span class="font-mono text-xs text-muted">GiB</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('settings.storage.headroom.label')" :description="t('settings.storage.headroom.description')">
        <UInput v-model.number="settings.storage_unknown_size_headroom" type="number" min="1" max="64" icon="i-lucide-scaling" class="mt-2 w-full" />
      </UFormField>
      <div class="flex items-center justify-between gap-5 md:col-span-2">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.auto_resume.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.auto_resume.description') }}</p>
        </div>
        <USwitch v-model="settings.storage_auto_resume" :aria-label="t('settings.storage.auto_resume.label')" />
      </div>
      <div class="flex items-center justify-between gap-5 border-t border-muted pt-4 md:col-span-2">
        <div>
          <p class="text-sm font-medium text-highlighted">{{ t('settings.admin_login.label') }}</p>
          <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.admin_login.description') }}</p>
          <p v-if="settings.admin_login_disabled" class="mt-1 text-xs leading-5 text-warning">{{ t('settings.admin_login.warning') }}</p>
        </div>
        <USwitch v-model="settings.admin_login_disabled" :aria-label="t('settings.admin_login.label')" />
      </div>
    </section>
  </div>
</template>
