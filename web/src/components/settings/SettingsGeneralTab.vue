<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { CollisionPolicy } from '@/api/storage'
import type { Settings, UsenetServer } from '@/api/types'
import { GIB, MIB, byteModel } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'
import CollisionPolicySelect from '@/components/storage/CollisionPolicySelect.vue'
import { DECIMAL, PLAIN, WHOLE, orNull } from '@/utils/numberInput'

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

/** The global level always has a policy; the select's `null` (inherit) is never offered here. */
const collisionPolicy = computed<CollisionPolicy | null>({
  get: () => settings.value.storage_collision_policy,
  set: (value) => { if (value) settings.value.storage_collision_policy = value }
})
/** The hand-set upload limit (RD-150-15), entered in MiB/s; empty is unlimited. */
const uploadLimitMiB = byteModel(
  () => settings.value.upload_limit_bytes_per_second,
  (raw) => { settings.value.upload_limit_bytes_per_second = raw },
  MIB
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
    <UCard as="section" :ui="{ body: 'grid gap-4' }">
      <UFormField :label="t('settings.active_files.label')" :description="t('settings.active_files.description')">
        <UInputNumber v-model="settings.max_active_files" required :min="1" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('settings.chunks.label')" :description="t('settings.chunks.description')">
        <UInputNumber v-model="settings.max_chunks_per_file" required :min="1" :max="16" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <UFormField data-settings-anchor="general.connections_per_host" :label="t('settings.connections_per_host.label')" :description="t('settings.connections_per_host.description')">
        <UInputNumber v-model="settings.max_connections_per_host" required :min="0" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <div>
        <UFormField :label="t('settings.nntp_connections.label')" :description="t('settings.nntp_connections.description')">
          <UInputNumber v-model="settings.nntp_connections_per_file" required :min="0" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
        </UFormField>
        <p v-if="nntpCapHint" class="mt-2 flex items-start gap-1.5 text-xs leading-5" :class="nntpCapHint.binding ? 'text-warning' : 'text-muted'" data-testid="nntp-cap-hint">
          <UIcon :name="nntpCapHint.binding ? 'i-lucide-triangle-alert' : 'i-lucide-info'" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ nntpCapHint.text }}</span>
        </p>
      </div>
      <UFormField :label="t('settings.nntp_parallel_files.label')" :description="t('settings.nntp_parallel_files.description')">
        <UInputNumber v-model="settings.nntp_parallel_files" required :min="0" :max="8" :format-options="WHOLE" increment decrement class="mt-2 w-full" data-testid="nntp-parallel-files" />
      </UFormField>
      <UFormField hint="MiB/s" data-settings-anchor="general.speed_limit" :label="t('settings.speed_limit.label')" :description="t('settings.speed_limit.description')">
        <UInputNumber :model-value="speedMiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" @update:model-value="speedMiB = orNull($event)" />
      </UFormField>
      <UFormField hint="MiB/s" :label="t('settings.upload_limit.label')" :description="t('settings.upload_limit.description')">
        <UInputNumber v-model="uploadLimitMiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" data-testid="upload-limit" />
      </UFormField>
      <UFormField data-settings-anchor="general.retries" :label="t('settings.retries.label')" :description="t('settings.retries.description')">
        <UInputNumber v-model="settings.max_retries" required :min="0" :max="100" :format-options="WHOLE" class="mt-2 w-full" />
      </UFormField>
      <USeparator />
      <div>
        <UFormField data-settings-anchor="general.auto_retry" :label="t('settings.auto_retry.label')" :description="t('settings.auto_retry.description')" orientation="horizontal">
          <USwitch v-model="settings.auto_retry_failed" data-testid="auto-retry-switch" />
        </UFormField>
        <div v-if="settings.auto_retry_failed" class="mt-4 grid gap-4" data-testid="auto-retry-options">
          <UFormField hint="h" :label="t('settings.auto_retry.interval_label')" :description="t('settings.auto_retry.interval_description')">
            <UInputNumber v-model="settings.auto_retry_interval_hours" required :min="1" :max="24" :format-options="WHOLE" increment decrement class="mt-2 w-full" data-testid="auto-retry-interval" />
          </UFormField>
          <UFormField :label="t('settings.auto_retry.rounds_label')" :description="t('settings.auto_retry.rounds_description')">
            <UInputNumber v-model="settings.auto_retry_max_rounds" required :min="0" :max="100" :format-options="WHOLE" class="mt-2 w-full" data-testid="auto-retry-rounds" />
          </UFormField>
        </div>
      </div>
      <div>
        <UFormField data-settings-anchor="general.ui_port" :label="t('settings.ui_port.label')" :description="t('settings.ui_port.description')">
          <UInputNumber v-model="uiPort" :min="1024" :max="65535" :format-options="PLAIN" :placeholder="t('settings.ui_port.placeholder')" class="mt-2 w-full" />
        </UFormField>
        <p class="mt-2 flex items-start gap-1.5 text-xs leading-5 text-warning">
          <UIcon name="i-lucide-rotate-cw" class="mt-0.5 size-3.5 shrink-0" />
          <span>{{ t('settings.ui_port.restart_hint') }}</span>
        </p>
      </div>
      <USeparator />
      <UFormField data-settings-anchor="general.mirrors" :label="t('settings.mirrors.label')" :description="t('settings.mirrors.description')" orientation="horizontal">
        <USwitch v-model="settings.mirror_detection" />
      </UFormField>
      <USeparator />
      <div>
        <UFormField data-settings-anchor="general.auto_remove" :label="t('settings.auto_remove.label')" :description="t('settings.auto_remove.description')" orientation="horizontal">
          <USwitch v-model="settings.auto_remove_finished" />
        </UFormField>
        <div v-if="settings.auto_remove_finished" class="mt-4 grid gap-4">
          <UFormField hint="h" :label="t('settings.auto_remove.delay_label')" :description="t('settings.auto_remove.delay_description')">
            <UInputNumber v-model="settings.auto_remove_delay_hours" required :min="1" :max="720" :format-options="WHOLE" class="mt-2 w-full" />
          </UFormField>
          <UFormField :label="t('settings.auto_remove.keep_failed_label')" :description="t('settings.auto_remove.keep_failed_description')" orientation="horizontal">
            <USwitch v-model="settings.auto_remove_keep_failed" />
          </UFormField>
        </div>
      </div>
      <USeparator />
      <UFormField :label="t('settings.import_history.label')" :description="t('settings.import_history.description')" orientation="horizontal">
        <USwitch v-model="settings.keep_import_history" />
      </UFormField>
      <USeparator />
      <UFormField data-settings-anchor="general.sha256" :label="t('settings.sha256.label')" :description="t('settings.sha256.description')" orientation="horizontal">
        <USwitch v-model="settings.generate_sha256" />
      </UFormField>
      <USeparator />
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.storage.title') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.storage.description') }}</p>
      </div>
      <UFormField hint="GiB" data-settings-anchor="general.minimum_free" :label="t('settings.storage.minimum_free.label')" :description="t('settings.storage.minimum_free.description')">
        <UInputNumber v-model="minimumFreeGiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('settings.storage.headroom.label')" :description="t('settings.storage.headroom.description')">
        <UInputNumber v-model="settings.storage_unknown_size_headroom" required :min="1" :max="64" :format-options="WHOLE" class="mt-2 w-full" />
      </UFormField>
      <UFormField data-settings-anchor="general.collision" :label="t('settings.storage.collision.label')" :description="t('settings.storage.collision.description')">
        <CollisionPolicySelect v-model="collisionPolicy" class="mt-2" />
      </UFormField>
      <UFormField :label="t('settings.storage.auto_resume.label')" :description="t('settings.storage.auto_resume.description')" orientation="horizontal">
        <USwitch v-model="settings.storage_auto_resume" />
      </UFormField>
      <USeparator />
      <UFormField data-settings-anchor="general.admin_login" :label="t('settings.admin_login.label')" orientation="horizontal">
        <template #description>
          {{ t('settings.admin_login.description') }}
          <span v-if="settings.admin_login_disabled" class="mt-1 block text-warning">{{ t('settings.admin_login.warning') }}</span>
        </template>
        <USwitch v-model="settings.admin_login_disabled" />
      </UFormField>
    </UCard>
  </div>
</template>
