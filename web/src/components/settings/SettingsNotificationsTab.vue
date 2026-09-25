<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Category, NotificationRule, NotificationTarget } from '@/api/types'
import NotificationHistory from '@/components/notifications/NotificationHistory.vue'
import NotificationRules from '@/components/notifications/NotificationRules.vue'
import NotificationTargets from '@/components/notifications/NotificationTargets.vue'
import { useFetchState } from '@/composables/useFetchState'
import SectionHeader from '@/components/SectionHeader.vue'

const { t } = useI18n()
const targets = ref<NotificationTarget[]>([])
const rules = ref<NotificationRule[]>([])
const categories = ref<Category[]>([])
const history = ref<InstanceType<typeof NotificationHistory> | null>(null)
/** One fetch feeds targets and rules, so one state describes both (RD-104-07). */
const { loading, loadError, load: trackLoad } = useFetchState()

async function load(): Promise<void> {
  await trackLoad(async () => {
    const [targetResponse, ruleResponse, categoryResponse] = await Promise.all([
      api.GET('/api/v1/notifications/targets'),
      api.GET('/api/v1/notifications/rules'),
      api.GET('/api/v1/categories')
    ])
    if (targetResponse.data) targets.value = targetResponse.data
    if (ruleResponse.data) rules.value = ruleResponse.data
    if (categoryResponse.data) categories.value = categoryResponse.data
    const failed = [targetResponse, ruleResponse].find(response => !response.data)
    return failed ? responseError(failed) : null
  })
}

async function refresh(): Promise<void> {
  await load()
  await history.value?.reload()
}

onMounted(load)
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.notifications.eyebrow')"
        :title="t('settings.headers.notifications.title')"
        :description="t('settings.headers.notifications.description')"
        level="page"
      />
    </header>
    <NotificationTargets v-model="targets" :loading="loading" :load-error="loadError" @changed="refresh" />
    <NotificationRules v-model="rules" :targets="targets" :categories="categories" :loading="loading" :load-error="loadError" />
    <NotificationHistory ref="history" />
  </div>
</template>
