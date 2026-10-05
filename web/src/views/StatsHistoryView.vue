<script setup lang="ts">
/**
 * Statistics and history on one page, as two tabs (RD-1101-05; the owner: fewer entries in the
 * navigation, one place and the digit `7` for both).
 *
 * The tab is held in the address as `?tab=`, the way the settings sub-tabs hold theirs
 * (`useSettingsSubTab`): the statistics are the first tab and carry no query, so `/stats` stays
 * the plain address, the history is `?tab=history`, and a value the page does not have shows
 * the statistics. A change pushes, so back and forward walk the tabs; `/history` redirects here.
 *
 * Each tab's content is a chunk of its own, fetched when the tab is first shown, as each was
 * while it was a view of its own; only the shown tab is mounted, so the statistics stop polling
 * while the history is open. A chunk the stopped service could not deliver is reported like a
 * view's (`router.onError`), and the page is loaded again once the service is back.
 */
import type { TabsItem } from '@nuxt/ui'
import { computed, defineAsyncComponent, type Component } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import { isChunkLoadError, reportViewLoadFailure } from '@/composables/serviceConnection'

const { t } = useI18n()
const route = useRoute()
const router = useRouter()

type StatsHistoryTab = 'stats' | 'history'

function lazyTab(load: () => Promise<{ default: Component }>): Component {
  return defineAsyncComponent(() => load().then(module => module.default, (error: unknown) => {
    if (isChunkLoadError(error)) reportViewLoadFailure(router.currentRoute.value.fullPath)
    throw error
  }))
}

const TAB_CONTENT: Record<StatsHistoryTab, Component> = {
  stats: lazyTab(() => import('@/components/stats/StatsTab.vue')),
  history: lazyTab(() => import('@/components/history/HistoryTab.vue'))
}

const tabs = computed<TabsItem[]>(() => [
  { value: 'stats', label: t('nav.stats'), icon: 'i-lucide-chart-column' },
  { value: 'history', label: t('nav.history'), icon: 'i-lucide-history' }
])

// `UTabs` hands back a string; anything but the history's name is the statistics.
const active = computed<string>({
  get: (): StatsHistoryTab => (route.query.tab === 'history' ? 'history' : 'stats'),
  set: (value) => {
    if (value === active.value || (value !== 'stats' && value !== 'history')) return
    const query = { ...route.query }
    delete query.tab
    if (value === 'history') query.tab = 'history'
    void router.push({ path: route.path, query, hash: route.hash })
  }
})
const content = computed(() => TAB_CONTENT[active.value === 'history' ? 'history' : 'stats'])
</script>

<template>
  <UDashboardPanel id="stats">
    <template #header>
      <UDashboardNavbar :title="t('nav.stats_history')">
        <template #leading><UDashboardSidebarCollapse /></template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <div class="w-full space-y-4">
        <UTabs v-model="active" :items="tabs" :content="false" variant="pill" class="w-full" />
        <component :is="content" :key="active" />
      </div>
    </template>
  </UDashboardPanel>
</template>
