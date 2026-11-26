<script setup lang="ts">
/**
 * *Notifications* (RD-1160-01): the targets with the rules that send to them on *Targets & rules*,
 * what was delivered on *History* — the record no longer sits under the forms that set it up. The
 * language of the built-in texts (RD-1480-11) is the page's one card of the settings document.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { NotificationRule, NotificationTarget, Settings } from '@/api/types'
import NotificationHistory from '@/components/notifications/NotificationHistory.vue'
import NotificationRules from '@/components/notifications/NotificationRules.vue'
import NotificationTargets from '@/components/notifications/NotificationTargets.vue'
import SettingsDocumentGate from '@/components/settings/SettingsDocumentGate.vue'
import SettingsNotificationLanguageCard from '@/components/settings/SettingsNotificationLanguageCard.vue'
import { useFetchState } from '@/composables/useFetchState'
import { subTabItems } from '@/composables/useSettingsSubTab'
import { useCategories } from '@/stores/categories'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>()
/** Owned by the settings view, which keeps it in the address. */
const activeTab = defineModel<string>('subTab', { default: 'targets' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('notifications', t))
const targets = ref<NotificationTarget[]>([])
const rules = ref<NotificationRule[]>([])
const { categories, fetchCategories } = useCategories()
const history = ref<InstanceType<typeof NotificationHistory> | null>(null)
/** One fetch feeds targets and rules, so one state describes both (RD-104-07). */
const { loading, loadError, load: trackLoad } = useFetchState()

async function load(): Promise<void> {
  await trackLoad(async () => {
    const [targetResponse, ruleResponse] = await Promise.all([
      api.GET('/api/v1/notifications/targets'),
      api.GET('/api/v1/notifications/rules'),
      fetchCategories()
    ])
    if (targetResponse.data) targets.value = targetResponse.data
    if (ruleResponse.data) rules.value = ruleResponse.data
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

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #targets>
        <div class="space-y-6">
          <NotificationTargets v-model="targets" :loading="loading" :load-error="loadError" @changed="refresh" />
          <NotificationRules v-model="rules" :targets="targets" :categories="categories" :loading="loading" :load-error="loadError" />
          <SettingsDocumentGate v-if="settings">
            <SettingsNotificationLanguageCard v-model="settings" />
          </SettingsDocumentGate>
        </div>
      </template>
      <template #history>
        <NotificationHistory ref="history" :targets="targets" />
      </template>
    </UTabs>
  </div>
</template>
