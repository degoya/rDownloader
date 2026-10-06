<script setup lang="ts">
/**
 * *Accounts* (RD-1120-23): the provider accounts and, on a tab of their own, the sign-ins for
 * protected sites, which were *Network › Authentication* — both are how rDownloader gets into a
 * site. The setup wizard embeds the accounts alone, under its own step heading.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import SectionHeader from '@/components/SectionHeader.vue'
import SettingsAccountsCard from '@/components/settings/SettingsAccountsCard.vue'
import SettingsAuthProfilesCard from '@/components/settings/SettingsAuthProfilesCard.vue'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

defineProps<{ hideHeader?: boolean }>()
/** Owned by the settings view, which keeps it in the address; the wizard shows no tabs. */
const activeTab = defineModel<string>('subTab', { default: 'accounts' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('accounts', t))
</script>

<template>
  <SettingsAccountsCard v-if="hideHeader" embedded />
  <div v-else class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.accounts.eyebrow')"
        :title="t('settings.headers.accounts.title')"
        :description="t('settings.headers.accounts.description')"
        level="page"
      />
      <SettingsCrossLink class="mt-2" anchor="interface.nzb_hand_over" />
    </header>

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #accounts>
        <SettingsAccountsCard />
      </template>
      <template #logins>
        <SettingsAuthProfilesCard />
      </template>
    </UTabs>
  </div>
</template>
