<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { CaptureToken, Settings, UsenetServer } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsDataResetButton from '@/components/settings/SettingsDataResetButton.vue'
import SettingsReadinessCard from '@/components/settings/SettingsReadinessCard.vue'
import { useAppTour } from '@/composables/useAppTour'
import { useSessionStore } from '@/stores/session'

const settings = defineModel<Settings>({ required: true })
defineProps<{ resetting?: boolean }>()
const emit = defineEmits<{ reset: [] }>()
const { t } = useI18n()
const session = useSessionStore()
const { startTour } = useAppTour()
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
    <div class="grid gap-3 lg:grid-cols-3">
      <article v-for="system in systems" :key="system.icon" class="relative overflow-hidden border border-muted bg-default p-5">
        <div class="mb-8 flex items-start justify-between">
          <div class="grid size-10 place-items-center bg-elevated text-primary"><UIcon :name="system.icon" class="size-5" /></div>
          <UBadge :color="system.color" variant="subtle">{{ system.status }}</UBadge>
        </div>
        <SectionHeader :eyebrow="system.eyebrow" :title="system.title" :description="system.description" />
        <div class="transfer-stripe absolute inset-x-0 bottom-0 h-1 opacity-50" />
      </article>
    </div>

    <SettingsReadinessCard class="mt-6" />

    <section class="mt-6 grid gap-px border border-muted bg-muted md:grid-cols-3 lg:grid-cols-5" data-testid="system-facts">
      <div class="bg-default p-5"><p class="eyebrow">{{ t('system.facts.about') }}</p><p class="mt-2 text-lg text-highlighted">rDownloader <span class="numeric text-sm text-muted">{{ serviceVersion || '…' }}</span></p><p class="mt-1 text-xs text-muted">Alexander Herling · GPL-3.0-or-later</p></div>
      <div class="bg-default p-5"><p class="eyebrow">{{ t('system.facts.web_ui') }}</p><p class="numeric mt-2 text-lg text-highlighted">{{ uiAddress }}</p><p class="mt-1 text-xs text-muted">{{ settings.ui_port ? t('system.facts.web_ui_configured') : t('system.facts.web_ui_default') }}</p></div>
      <div class="bg-default p-5"><p class="eyebrow">{{ t('system.facts.cnl2') }}</p><p class="numeric mt-2 text-lg text-highlighted">127.0.0.1:9666</p><p class="mt-1 text-xs text-muted">{{ t('system.facts.loopback_only') }}</p></div>
      <div class="bg-default p-5"><p class="eyebrow">{{ t('system.facts.hotfolder') }}</p><p class="numeric mt-2 text-lg text-highlighted">{{ settings.hotfolder_poll_seconds }} s</p><p class="mt-1 text-xs text-muted">{{ t('system.facts.hotfolder_note') }}</p></div>
      <div class="bg-default p-5"><p class="eyebrow">{{ t('system.facts.nzb') }}</p><p class="numeric mt-2 text-lg text-highlighted">64 MiB</p><p class="mt-1 text-xs text-muted">{{ t('system.facts.nzb_note') }}</p></div>
    </section>

    <section class="mt-6 border border-muted bg-default p-5" data-testid="log-retention">
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
      <div class="mt-4 grid gap-4 md:grid-cols-2">
        <UFormField :label="t('settings.logs.records_label')" :description="t('settings.logs.records_description')">
          <UInput v-model.number="settings.log_retention_records" type="number" min="1000" max="500000" step="1000" icon="i-lucide-database" class="mt-2 w-full" />
        </UFormField>
        <UFormField :label="t('settings.logs.days_label')" :description="t('settings.logs.days_description')">
          <UInput v-model.number="settings.log_retention_days" type="number" min="1" max="365" icon="i-lucide-calendar-days" class="mt-2 w-full" />
        </UFormField>
      </div>
    </section>

    <section class="mt-6 border border-muted bg-default p-5" data-testid="audit-retention">
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
      <div class="mt-4 grid gap-4 md:grid-cols-2">
        <UFormField :label="t('settings.audit.records_label')" :description="t('settings.audit.records_description')">
          <UInput v-model.number="settings.audit_retention_records" type="number" min="10000" max="2000000" step="10000" icon="i-lucide-database" class="mt-2 w-full" />
        </UFormField>
        <UFormField :label="t('settings.audit.days_label')" :description="t('settings.audit.days_description')">
          <UInput v-model.number="settings.audit_retention_days" type="number" min="30" max="3650" icon="i-lucide-calendar-days" class="mt-2 w-full" />
        </UFormField>
      </div>
      <div class="mt-4 border-t border-muted pt-4">
        <UFormField :label="t('settings.audit.otlp_enabled_label')" :description="t('settings.audit.otlp_enabled_description')">
          <USwitch v-model="settings.otlp_enabled" class="mt-2" data-testid="otlp-enabled" />
        </UFormField>
        <div class="mt-4 grid gap-4 md:grid-cols-2">
          <UFormField :label="t('settings.audit.otlp_endpoint_label')" :description="t('settings.audit.otlp_endpoint_description')">
            <UInput v-model="settings.otlp_endpoint" placeholder="http://127.0.0.1:4318/v1/traces" icon="i-lucide-waypoints" class="mt-2 w-full" data-testid="otlp-endpoint" />
          </UFormField>
          <UFormField :label="t('settings.audit.otlp_timeout_label')" :description="t('settings.audit.otlp_timeout_description')">
            <UInput v-model.number="settings.otlp_timeout_seconds" type="number" min="1" max="60" icon="i-lucide-timer" class="mt-2 w-full">
              <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
            </UInput>
          </UFormField>
        </div>
      </div>
      <SettingsDataResetButton class="mt-4" target="audit" :count="dataCounts.audit" @cleared="loadDataCounts()" />
    </section>

    <section class="mt-6 border border-muted bg-default p-5" data-testid="stats-retention">
      <SectionHeader :eyebrow="t('stats.retention.eyebrow')" :title="t('stats.retention.title')" :description="t('stats.retention.description')" />
      <div class="mt-4 grid gap-4 md:grid-cols-2">
        <UFormField :label="t('stats.retention.hourly_label')" :description="t('stats.retention.hourly_description')">
          <UInput v-model.number="settings.stats_hourly_days" type="number" min="1" max="3650" icon="i-lucide-timer" class="mt-2 w-full">
            <template #trailing><span class="font-mono text-xs text-muted">d</span></template>
          </UInput>
        </UFormField>
        <UFormField :label="t('stats.retention.retention_label')" :description="t('stats.retention.retention_description')">
          <UInput v-model.number="settings.stats_retention_days" type="number" min="7" max="3650" icon="i-lucide-archive" class="mt-2 w-full">
            <template #trailing><span class="font-mono text-xs text-muted">d</span></template>
          </UInput>
        </UFormField>
      </div>
      <SettingsDataResetButton class="mt-4" target="stats" :count="dataCounts.stats" @cleared="loadDataCounts()" />
    </section>
  </div>
</template>
