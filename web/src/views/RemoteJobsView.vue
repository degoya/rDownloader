<script setup lang="ts">
/**
 * Remote jobs as a place of their own (RD-110-29).
 *
 * The card was built under Accounts (RD-108-04), on the grounds that a job is charged to an
 * account. But a job that runs at a provider is something to watch and to answer — a magnet
 * fetched elsewhere, a question about which files to take — and not a setting. It sits beside
 * the subscriptions in the navigation for the same reason their hits do, and the card itself
 * is unchanged: it is handed the accounts it needs, the way the accounts page handed them.
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Account } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsRemoteJobsCard from '@/components/settings/SettingsRemoteJobsCard.vue'

const { t } = useI18n()
const accounts = ref<Account[]>([])
/** Until the accounts are read the card cannot tell "none fits" from "not known yet" (RD-120-51). */
const accountsLoading = ref(true)
const message = ref<string | null>(null)
const error = ref<string | null>(null)

onMounted(async () => {
  const response = await api.GET('/api/v1/accounts')
  accountsLoading.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  accounts.value = response.data
})
</script>

<template>
  <UDashboardPanel id="remote-jobs">
    <template #header>
      <UDashboardNavbar :title="t('nav.remote_jobs')">
        <template #leading><UDashboardSidebarCollapse /></template>
      </UDashboardNavbar>
    </template>
    <template #body>
      <div class="w-full space-y-6 pt-4">
        <header>
          <SectionHeader
            :eyebrow="t('remote_jobs.page.eyebrow')"
            :title="t('remote_jobs.page.title')"
            :description="t('remote_jobs.page.description')"
            level="page"
          />
        </header>
        <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
        <UAlert v-if="message" color="success" variant="subtle" icon="i-lucide-circle-check" :description="message" />
        <SettingsRemoteJobsCard
          :accounts="accounts"
          :accounts-loading="accountsLoading"
          @message="(text: string) => { error = null; message = text }"
          @error="(text: string) => { message = null; error = text }"
        />
      </div>
    </template>
  </UDashboardPanel>
</template>
