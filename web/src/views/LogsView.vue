<script setup lang="ts">
/**
 * The structured log viewer and the diagnostic bundle (RD-110-02), as two tabs (RD-1120-01).
 *
 * Everything shown here was redacted before it was stored; the view only draws. The filter
 * bar sits above the list and the list says when it is a full page, per `design.md`: a page
 * that quietly ends is how "nothing matched" gets mistaken for "nothing was asked for".
 *
 * The bundle has a tab of its own: under a long list nobody found it but by searching the page.
 * The tab is held in the address as `?tab=` the way the statistics hold theirs
 * (`StatsHistoryView.vue`): the log is the first tab and carries no query, the bundle is
 * `?tab=bundle`, a value the page does not have shows the log, and a change pushes, so back and
 * forward walk the tabs. Only the shown tab is mounted; the refresh button belongs to the log
 * and is not shown beside the bundle, whose preview button is its own reload.
 */
import type { TabsItem } from '@nuxt/ui'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import type { LogLevel, LogRecord } from '@/api/types'
import DataState from '@/components/DataState.vue'
import DiagnosticBundlePanel from '@/components/logs/DiagnosticBundlePanel.vue'
import { useLogsStore } from '@/stores/logs'
import { formatMoment } from '@/utils/format'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()
const store = useLogsStore()
const expanded = ref<Set<number>>(new Set())

type LogsTab = 'log' | 'bundle'

const tabs = computed<TabsItem[]>(() => [
  { value: 'log', label: t('logs.tabs.log'), icon: 'i-lucide-scroll-text' },
  { value: 'bundle', label: t('logs.tabs.bundle'), icon: 'i-lucide-package' }
])

// `UTabs` hands back a string; anything but the bundle's name is the log.
const active = computed<string>({
  get: (): LogsTab => (route.query.tab === 'bundle' ? 'bundle' : 'log'),
  set: (value) => {
    if (value === active.value || (value !== 'log' && value !== 'bundle')) return
    const query = { ...route.query }
    delete query.tab
    if (value === 'bundle') query.tab = 'bundle'
    void router.push({ path: route.path, query, hash: route.hash })
  }
})

const LEVELS: LogLevel[] = ['trace', 'debug', 'info', 'warn', 'error']

const levelItems = [
  { value: 'all', label: t('logs.levels.all') },
  ...LEVELS.map(level => ({ value: level, label: t(`logs.levels.${level}`) }))
]

function levelColor(level: LogLevel): 'error' | 'warning' | 'primary' | 'neutral' {
  if (level === 'error') return 'error'
  if (level === 'warn') return 'warning'
  if (level === 'info') return 'primary'
  return 'neutral'
}

function hasFields(record: LogRecord): boolean {
  return Object.keys(record.fields).length > 0
}

function toggle(id: number): void {
  const next = new Set(expanded.value)
  if (next.has(id)) next.delete(id)
  else next.add(id)
  expanded.value = next
}

// The log is fetched once per visit, when its tab is first shown; back from the bundle, the
// pages already loaded (older ones included) stay, and the refresh button fetches anew.
let fetched = false
watch(active, (tab) => {
  if (tab !== 'log' || fetched) return
  fetched = true
  void store.refresh()
}, { immediate: true })
</script>

<template>
  <UDashboardPanel id="logs">
    <template #header>
      <UDashboardNavbar :title="t('logs.title')">
        <template #right>
          <UButton
            v-if="active === 'log'"
            icon="i-lucide-refresh-cw"
            color="neutral"
            variant="subtle"
            :label="t('common.actions.refresh')"
            :loading="store.fetching"
            @click="store.refresh()"
          />
        </template>
      </UDashboardNavbar>
    </template>

    <template #body>
      <UTabs v-model="active" :items="tabs" :content="false" variant="pill" class="mb-4 w-full" />
      <template v-if="active === 'log'">
        <p class="mb-4 text-sm leading-6 text-muted">{{ t('logs.intro') }}</p>

        <UCard class="mb-4">
          <form class="grid gap-3 md:grid-cols-6" @submit.prevent="store.refresh()">
            <UFormField :label="t('logs.filters.level')">
              <USelect v-model="store.filters.level" :items="levelItems" value-key="value" class="w-full" data-testid="log-level" />
            </UFormField>
            <UFormField :label="t('logs.filters.component')">
              <UInput v-model="store.filters.component" :placeholder="t('logs.filters.component_placeholder')" class="w-full" data-testid="log-component" />
            </UFormField>
            <UFormField :label="t('logs.filters.code')">
              <UInput v-model="store.filters.code" class="w-full" data-testid="log-code" />
            </UFormField>
            <UFormField :label="t('logs.filters.correlation')">
              <UInput v-model="store.filters.correlationId" class="w-full" data-testid="log-correlation" />
            </UFormField>
            <UFormField :label="t('logs.filters.search')" class="md:col-span-2">
              <UInput v-model="store.filters.search" icon="i-lucide-search" class="w-full" data-testid="log-search" />
            </UFormField>
            <div class="flex flex-wrap gap-2 md:col-span-6">
              <UButton type="submit" icon="i-lucide-filter" :label="t('common.actions.apply')" :loading="store.fetching" />
              <UButton
                type="button"
                color="neutral"
                variant="ghost"
                icon="i-lucide-x"
                :label="t('logs.filters.clear')"
                @click="store.clearFilters(); store.refresh()"
              />
            </div>
          </form>
        </UCard>

        <UAlert
          v-if="store.dropped > 0"
          class="mb-4"
          color="warning"
          :description="t('logs.list.dropped', { count: store.dropped })"
        />

        <div class="mb-2 flex flex-wrap items-baseline justify-between gap-2">
          <h2 class="text-sm font-semibold text-highlighted">
            {{ t('logs.list.title') }}
            <span v-if="store.settled" class="numeric ml-2 text-xs font-normal text-muted">
              {{ t('logs.list.shown', { shown: store.records.length, total: store.total }) }}
            </span>
          </h2>
          <p v-if="store.retention" class="text-xs text-muted">
            {{ t('logs.list.retention', { days: store.retention.days, records: store.retention.records }) }}
          </p>
        </div>

        <!-- The entries in a card of their own, under their heading, as on the other list pages (RD-1110-17). -->
        <UCard as="section" :ui="{ body: 'p-0 sm:p-0' }">
          <DataState class="p-4 sm:p-6" variant="inline" :loading="store.loading" :error="store.error" :empty="store.settled && store.records.length === 0" :rows="6">
            <UEmpty :description="t('logs.list.empty')" />
          </DataState>
          <ul v-if="store.records.length" class="divide-y divide-muted" data-testid="log-list">
            <li v-for="record in store.records" :key="record.id" class="px-3 py-2">
              <!-- Below md the message takes a line of its own under the meta line: inline it was
                   left ~60 px on a phone and wrapped letter by letter. The expand button stays at
                   the end of the meta line there. -->
              <div class="flex flex-wrap items-start gap-x-3 gap-y-1">
                <span class="numeric shrink-0 text-xs text-muted">{{ formatMoment(record.recorded_at) }}</span>
                <UBadge :color="levelColor(record.level)" variant="subtle" size="sm">{{ t(`logs.levels.${record.level}`) }}</UBadge>
                <span class="numeric shrink-0 text-xs text-muted">{{ record.component }}</span>
                <UBadge v-if="record.code" color="neutral" variant="outline" size="sm" class="numeric">{{ record.code }}</UBadge>
                <UBadge v-if="record.correlation_id" color="neutral" variant="soft" size="sm" class="numeric" :title="t('logs.filters.correlation')">{{ record.correlation_id }}</UBadge>
                <span class="order-last min-w-0 basis-full break-words text-sm text-highlighted md:order-none md:min-w-64 md:flex-1 md:basis-0" data-testid="log-message">{{ record.message }}</span>
                <!-- The fields are the row's last line, after the message even where it moves down (`design.md`, *Opening and closing*). -->
                <UCollapsible
                  v-if="hasFields(record)"
                  class="contents"
                  :open="expanded.has(record.id)"
                  :ui="{ content: 'order-last basis-full' }"
                  @update:open="toggle(record.id)"
                >
                  <UButton
                    class="ml-auto md:ml-0"
                    size="xs"
                    variant="ghost"
                    color="neutral"
                    :icon="expanded.has(record.id) ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
                    :aria-expanded="expanded.has(record.id)"
                    :aria-label="expanded.has(record.id) ? t('logs.list.collapse') : t('logs.list.expand')"
                    :title="expanded.has(record.id) ? t('logs.list.collapse') : t('logs.list.expand')"
                  />
                  <template #content>
                    <dl class="mt-1 grid gap-x-4 gap-y-1 text-xs sm:grid-cols-[auto_1fr]" data-testid="log-fields">
                      <template v-for="(value, name) in record.fields" :key="name">
                        <dt class="numeric text-muted">{{ name }}</dt>
                        <dd class="numeric break-all text-highlighted">{{ value }}</dd>
                      </template>
                    </dl>
                  </template>
                </UCollapsible>
              </div>
            </li>
          </ul>
        </UCard>

        <div v-if="store.fullPage" class="mt-3 flex flex-wrap items-center gap-3">
          <p class="text-xs text-muted">{{ t('logs.list.full_page') }}</p>
          <UButton
            size="xs"
            color="neutral"
            variant="outline"
            icon="i-lucide-history"
            :label="t('logs.list.older')"
            :loading="store.fetching"
            @click="store.loadOlder()"
          />
        </div>
      </template>
      <DiagnosticBundlePanel v-else />
    </template>
  </UDashboardPanel>
</template>
