<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Category, CategoryRule, Settings, StorageRoot } from '@/api/types'
import RoutingBackupButtons from '@/components/routing/RoutingBackupButtons.vue'
import RoutingCategories from '@/components/routing/RoutingCategories.vue'
import RoutingCategoryRules from '@/components/routing/RoutingCategoryRules.vue'
import RoutingStorageRoots from '@/components/routing/RoutingStorageRoots.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsCollectorTab from '@/components/settings/SettingsCollectorTab.vue'
import { useFetchState } from '@/composables/useFetchState'

const settings = defineModel<Settings>({ required: true })
/** Owned by the parent: only the collector pane needs the settings save bar. */
const activeTab = defineModel<string>('subTab', { required: true })
const { t } = useI18n()
const roots = ref<StorageRoot[]>([])
const categories = ref<Category[]>([])
const rules = ref<CategoryRule[]>([])
/** One fetch feeds all three list sub-tabs, so one state describes all three (RD-104-07). */
const { loading, loadError, load } = useFetchState()

/** Counts live in the tab badges so nothing is hidden behind a tab the user has not opened. */
const tabItems = computed(() => [
  { value: 'roots', slot: 'roots', label: t('routing.tabs.roots'), icon: 'i-lucide-hard-drive', badge: roots.value.length },
  { value: 'categories', slot: 'categories', label: t('routing.tabs.categories'), icon: 'i-lucide-folder-tree', badge: categories.value.length },
  { value: 'rules', slot: 'rules', label: t('routing.tabs.rules'), icon: 'i-lucide-git-branch', badge: rules.value.length },
  { value: 'collector', slot: 'collector', label: t('routing.tabs.collector'), icon: 'i-lucide-shield-ban' }
])

onMounted(() => void load(refresh))

async function refresh(): Promise<string | null> {
  const [rootResponse, categoryResponse, ruleResponse] = await Promise.all([
    api.GET('/api/v1/storage-roots'),
    api.GET('/api/v1/categories'),
    api.GET('/api/v1/category-rules')
  ])
  if (rootResponse.data) roots.value = rootResponse.data
  if (categoryResponse.data) categories.value = categoryResponse.data
  if (ruleResponse.data) rules.value = ruleResponse.data
  // Any of the three failing means at least one sub-tab would otherwise print "none
  // configured" over a list it never received.
  const failed = [rootResponse, categoryResponse, ruleResponse].find(response => !response.data)
  return failed ? responseError(failed) : null
}

/**
 * Deleting a category also removes its rules server-side (and detaches hotfolders, which the
 * hotfolders page reloads for itself when it is opened).
 */
async function reloadDependents(): Promise<void> {
  const ruleResponse = await api.GET('/api/v1/category-rules')
  if (ruleResponse.data) rules.value = ruleResponse.data
}
</script>

<template>
  <div class="w-full space-y-6">
    <header class="flex flex-wrap items-end justify-between gap-4">
      <div>
        <SectionHeader
          :eyebrow="t('routing.header.eyebrow')"
          :title="t('routing.header.title')"
          :description="t('routing.header.description')"
          level="page"
        />
      </div>
      <RoutingBackupButtons @imported="refresh" />
    </header>

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
      :ui="{ content: 'pt-4' }"
    >
      <template #roots>
        <RoutingStorageRoots v-model="roots" :loading="loading" :load-error="loadError" />
      </template>
      <template #categories>
        <RoutingCategories v-model="categories" :roots="roots" :loading="loading" :load-error="loadError" @removed="reloadDependents" />
      </template>
      <template #rules>
        <RoutingCategoryRules v-model="rules" :categories="categories" :loading="loading" :load-error="loadError" />
      </template>
      <template #collector>
        <SettingsCollectorTab v-model="settings" />
      </template>
    </UTabs>
  </div>
</template>
