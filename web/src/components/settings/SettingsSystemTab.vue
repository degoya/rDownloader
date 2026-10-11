<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { CaptureToken, Settings, UsenetServer } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import StatTiles from '@/components/StatTiles.vue'
import SettingsCleanupCard from '@/components/settings/SettingsCleanupCard.vue'
import SettingsDataResetButton from '@/components/settings/SettingsDataResetButton.vue'
import SettingsReadinessCard from '@/components/settings/SettingsReadinessCard.vue'
import SettingsUpdateCard from '@/components/settings/SettingsUpdateCard.vue'
import { useAppTour } from '@/composables/useAppTour'
import { subTabItems } from '@/composables/useSettingsSubTab'
import { useSessionStore } from '@/stores/session'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
/**
 * Owned by the settings view, which keeps it in the address (RD-180-15). Seven blocks became
 * three tabs: what is running, the updates, and how long the service keeps its records. The
 * header with the reset, wizard and tour buttons stays above them.
 */
const activeTab = defineModel<string>('subTab', { default: 'status' })
defineProps<{ resetting?: boolean }>()
const emit = defineEmits<{ reset: [] }>()
const { t } = useI18n()
const session = useSessionStore()
const { startTour } = useAppTour()
const tabItems = computed(() => subTabItems('system', t))
// Only for the status card below; pairing itself lives in the desktop client section.
const agents = ref<CaptureToken[]>([])
const usenetServers = ref<UsenetServer[]>([])
const serviceVersion = ref<string>('')
/**
 * How many records each clearable store holds, so the confirmation can name the number
 * before it asks rather than after (RD-120-34). `null` until the count has arrived; the
 * buttons stay quiet about a figure they do not have yet.
 */
const dataCounts = ref<{ logs: number | null, audit: number | null, stats: number | null }>({
  logs: null,
  audit: null,
  stats: null
})

onMounted(() => {
  void loadAgents()
  void loadUsenetServers()
  void loadVersion()
  void loadDataCounts()
})

async function loadDataCounts(): Promise<void> {
  const response = await api.GET('/api/v1/system/data-reset')
  if (response.data) dataCounts.value = response.data
}

async function loadAgents(): Promise<void> {
  const response = await api.GET('/api/v1/capture/agents')
  if (response.data) agents.value = response.data
}

async function loadVersion(): Promise<void> {
  const response = await api.GET('/api/v1/health')
  const data = response.data as { version?: string } | undefined
  if (data?.version) serviceVersion.value = data.version
}

async function loadUsenetServers(): Promise<void> {
  const response = await api.GET('/api/v1/usenet/servers')
  if (response.data) usenetServers.value = response.data
}

defineExpose({ refresh: loadUsenetServers })

const usenetStatus = computed(() => {
  const active = usenetServers.value.filter(server => server.enabled)
  if (!active.length) return { label: t('system.cards.usenet.none'), color: 'warning' as const }
  const connections = active.reduce((sum, server) => sum + server.max_connections, 0)
  return {
    label: t('system.cards.usenet.summary', {
      servers: t('usenet.summary.servers', { count: active.length }, active.length),
      connections: t('usenet.summary.connections', { count: connections }, connections)
    }),
    color: 'success' as const
  }
})

/** Configured port wins; without an override the address this page was loaded from is the truth. */
const uiAddress = computed(() => settings.value.ui_port ? `127.0.0.1:${settings.value.ui_port}` : window.location.host)

/** The fixed facts under the status cards; the product's own tile shows its version in its slot. */
const facts = computed(() => [
  { key: 'about', label: t('system.facts.about'), value: 'rDownloader', hint: 'Alexander Herling · GPL-3.0-or-later', numeric: false },
  { key: 'web_ui', label: t('system.facts.web_ui'), value: uiAddress.value, hint: settings.value.ui_port ? t('system.facts.web_ui_configured') : t('system.facts.web_ui_default') },
  { key: 'cnl2', label: t('system.facts.cnl2'), value: '127.0.0.1:9666', hint: t('system.facts.loopback_only') },
  { key: 'hotfolder', label: t('system.facts.hotfolder'), value: `${settings.value.hotfolder_poll_seconds} s`, hint: t('system.facts.hotfolder_note') },
  { key: 'nzb', label: t('system.facts.nzb'), value: '64 MiB', hint: t('system.facts.nzb_note') }
])

const systems = computed(() => [
  {
    title: t('system.cards.http.title'),
    eyebrow: t('system.cards.http.eyebrow'),
    icon: 'i-lucide-cloud-download',
    status: t('system.cards.http.status'),
    color: 'success' as const,
    description: t('system.cards.http.description')
  },
  {
    title: t('system.cards.capture.title'),
    eyebrow: t('system.cards.capture.eyebrow'),
    icon: 'i-lucide-scan-line',
    status: agents.value.length ? t('system.cards.capture.paired') : t('system.cards.capture.ready'),
    color: agents.value.length ? 'success' as const : 'warning' as const,
    description: t('system.cards.capture.description')
  },
  {
    title: t('system.cards.usenet.title'),
    eyebrow: t('system.cards.usenet.eyebrow'),
    icon: 'i-lucide-network',
    status: usenetStatus.value.label,
    color: usenetStatus.value.color,
    description: t('system.cards.usenet.description')
  }
])
</script>

<template>
  <div class="w-full">
    <header class="mb-6 flex flex-wrap items-start justify-between gap-4">
      <div>
        <SectionHeader
          :eyebrow="t('settings.headers.system.eyebrow')"
          :title="t('settings.headers.system.title')"
          :description="t('settings.headers.system.description')"
          level="page"
        />
      </div>
      <div class="flex shrink-0 flex-wrap gap-2">
        <UButton
          icon="i-lucide-rotate-ccw"
          color="error"
          variant="soft"
          :label="t('settings.reset.button')"
          :loading="resetting"
          @click="emit('reset')"
        />
        <UButton
          icon="i-lucide-wand-2"
          color="neutral"
          variant="subtle"
          :label="t('wizard.rerun')"
          @click="session.openWizard()"
        />
        <UButton
          icon="i-lucide-footprints"
          color="neutral"
          variant="subtle"
          :label="t('tour.rerun')"
          @click="startTour()"
        />
      </div>
    </header>
    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #status>
        <div>
          <div class="grid gap-3 lg:grid-cols-3">
            <UCard v-for="system in systems" :key="system.icon" as="article" class="relative">
              <div class="mb-8 flex items-start justify-between">
                <UAvatar :icon="system.icon" color="primary" size="xl" />
                <UBadge :color="system.color" variant="subtle">{{ system.status }}</UBadge>
              </div>
              <SectionHeader :eyebrow="system.eyebrow" :title="system.title" :description="system.description" />
              <div class="transfer-stripe absolute inset-x-0 bottom-0 h-1 opacity-50" />
            </UCard>
          </div>

          <SettingsReadinessCard class="mt-6" />

          <StatTiles as="section" surface="default" :tiles="facts" class="mt-6 md:grid-cols-3 lg:grid-cols-5" data-testid="system-facts">
            <template #value="{ tile }">
              <template v-if="tile.key === 'about'">rDownloader <span class="numeric text-sm text-muted">{{ serviceVersion || '…' }}</span></template>
              <template v-else>{{ tile.value }}</template>
            </template>
          </StatTiles>
        </div>
      </template>
      <template #updates>
        <SettingsUpdateCard v-model="settings" />
      </template>
      <template #retention>
        <div>
          <UCard as="section" data-settings-anchor="system.logs" data-testid="log-retention">
            <div class="flex flex-wrap items-start justify-between gap-4">
              <SectionHeader
                :eyebrow="t('settings.logs.eyebrow')"
                :title="t('settings.logs.title')"
                :description="t('settings.logs.description')"
              />
              <UButton
                icon="i-lucide-scroll-text"
                color="neutral"
                variant="subtle"
                :label="t('settings.logs.open')"
                to="/logs"
              />
            </div>
            <SettingsDataResetButton class="mt-4" target="logs" :count="dataCounts.logs" @cleared="loadDataCounts()" />
            <div class="mt-4 grid gap-4">
              <UFormField :label="t('settings.logs.records_label')" :description="t('settings.logs.records_description')">
                <UInputNumber v-model="settings.log_retention_records" required :min="1000" :max="500000" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
              <UFormField :label="t('settings.logs.days_label')" :description="t('settings.logs.days_description')">
                <UInputNumber v-model="settings.log_retention_days" required :min="1" :max="365" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
            </div>
          </UCard>

          <UCard as="section" data-settings-anchor="system.audit" class="mt-6" data-testid="audit-retention">
            <div class="flex flex-wrap items-start justify-between gap-4">
              <SectionHeader
                :eyebrow="t('settings.audit.eyebrow')"
                :title="t('settings.audit.title')"
                :description="t('settings.audit.description')"
              />
              <UButton
                icon="i-lucide-shield-check"
                color="neutral"
                variant="subtle"
                :label="t('settings.audit.open')"
                to="/audit"
              />
            </div>
            <div class="mt-4 grid gap-4">
              <UFormField :label="t('settings.audit.records_label')" :description="t('settings.audit.records_description')">
                <UInputNumber v-model="settings.audit_retention_records" required :min="10000" :max="2000000" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
              <UFormField :label="t('settings.audit.days_label')" :description="t('settings.audit.days_description')">
                <UInputNumber v-model="settings.audit_retention_days" required :min="30" :max="3650" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
            </div>
            <USeparator class="my-4" />
            <div>
              <UFormField :label="t('settings.audit.otlp_enabled_label')" :description="t('settings.audit.otlp_enabled_description')" orientation="horizontal">
                <USwitch v-model="settings.otlp_enabled" data-testid="otlp-enabled" />
              </UFormField>
              <div class="mt-4 grid gap-4">
                <UFormField :label="t('settings.audit.otlp_endpoint_label')" :description="t('settings.audit.otlp_endpoint_description')">
                  <UInput v-model="settings.otlp_endpoint" placeholder="http://127.0.0.1:4318/v1/traces" icon="i-lucide-waypoints" class="mt-2 w-full" data-testid="otlp-endpoint" />
                </UFormField>
                <UFormField :label="t('settings.audit.otlp_timeout_label')" :description="t('settings.audit.otlp_timeout_description')">
                  <NumberWithUnit v-model="settings.otlp_timeout_seconds" unit="s" required :min="1" :max="60" :format-options="WHOLE" class="mt-2 w-full" />
                </UFormField>
              </div>
            </div>
            <SettingsDataResetButton class="mt-4" target="audit" :count="dataCounts.audit" @cleared="loadDataCounts()" />
          </UCard>

          <UCard as="section" data-settings-anchor="system.history" class="mt-6" data-testid="history-retention">
            <div class="flex flex-wrap items-start justify-between gap-4">
              <SectionHeader
                :eyebrow="t('settings.history.eyebrow')"
                :title="t('settings.history.title')"
                :description="t('settings.history.description')"
              />
              <UButton
                icon="i-lucide-history"
                color="neutral"
                variant="subtle"
                :label="t('settings.history.open')"
                to="/stats?tab=history"
              />
            </div>
            <div class="mt-4 grid gap-4">
              <UFormField :label="t('settings.history.entries_label')" :description="t('settings.history.entries_description')">
                <UInputNumber v-model="settings.history_retention_entries" required :min="100" :max="100000" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
              <UFormField :label="t('settings.history.days_label')" :description="t('settings.history.days_description')">
                <UInputNumber v-model="settings.history_retention_days" required :min="1" :max="3650" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
            </div>
          </UCard>

          <UCard as="section" data-settings-anchor="system.stats_retention" class="mt-6" data-testid="stats-retention">
            <SectionHeader :eyebrow="t('stats.retention.eyebrow')" :title="t('stats.retention.title')" :description="t('stats.retention.description')" />
            <div class="mt-4 grid gap-4">
              <UFormField :label="t('stats.retention.hourly_label')" :description="t('stats.retention.hourly_description')">
                <NumberWithUnit v-model="settings.stats_hourly_days" unit="d" required :min="1" :max="3650" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
              <UFormField :label="t('stats.retention.retention_label')" :description="t('stats.retention.retention_description')">
                <NumberWithUnit v-model="settings.stats_retention_days" unit="d" required :min="7" :max="3650" :format-options="WHOLE" class="mt-2 w-full" />
              </UFormField>
            </div>
            <SettingsDataResetButton class="mt-4" target="stats" :count="dataCounts.stats" @cleared="loadDataCounts()" />
          </UCard>

          <!-- The copies kept for taking updates back and the plugin cache (RD-1240-34). -->
          <SettingsCleanupCard v-model="settings" class="mt-6" />

          <!-- With the other retention rules since RD-1120-23; it was a switch on General. -->
          <UCard as="section" class="mt-6" data-testid="import-history-retention">
            <SectionHeader :eyebrow="t('settings.import_history.eyebrow')" :title="t('settings.import_history.title')" />
            <UFormField data-settings-anchor="system.import_history" :label="t('settings.import_history.label')" :description="t('settings.import_history.description')" orientation="horizontal" class="mt-4 border-t border-muted pt-4">
              <USwitch v-model="settings.keep_import_history" />
            </UFormField>
          </UCard>
        </div>
      </template>
    </UTabs>
  </div>
</template>
