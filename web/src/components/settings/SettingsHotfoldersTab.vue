<script setup lang="ts">
/**
 * The hotfolders page: the watched folders and the one settings value they own, the poll
 * interval (RD-110-31). A page of its own since RD-110-29, no longer a sub-tab of the routing
 * page; the list component is unchanged and still saves each folder and the interval itself.
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Category, HotFolder, Settings } from '@/api/types'
import RoutingHotfolders from '@/components/routing/RoutingHotfolders.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useFetchState } from '@/composables/useFetchState'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const hotfolders = ref<HotFolder[]>([])
const categories = ref<Category[]>([])
/** One fetch feeds the list and its category names, so one state describes both (RD-104-07). */
const { loading, loadError, load } = useFetchState()

onMounted(() => void load(refresh))

async function refresh(): Promise<string | null> {
  const [hotfolderResponse, categoryResponse] = await Promise.all([
    api.GET('/api/v1/hotfolders'),
    api.GET('/api/v1/categories')
  ])
  if (hotfolderResponse.data) hotfolders.value = hotfolderResponse.data
  if (categoryResponse.data) categories.value = categoryResponse.data
  const failed = [hotfolderResponse, categoryResponse].find(response => !response.data)
  return failed ? responseError(failed) : null
}
</script>

<template>
  <div class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.hotfolders.eyebrow')"
        :title="t('settings.headers.hotfolders.title')"
        :description="t('settings.headers.hotfolders.description')"
        level="page"
      />
    </header>
    <RoutingHotfolders v-model="hotfolders" v-model:settings="settings" :categories="categories" :loading="loading" :load-error="loadError" />
  </div>
</template>
