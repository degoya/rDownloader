<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import SettingsAccountsTab from '@/components/settings/SettingsAccountsTab.vue'
import SettingsUsenetTab from '@/components/settings/SettingsUsenetTab.vue'

const { t } = useI18n()
const activeTab = ref('accounts')
const tabItems = ref([
  { value: 'accounts', label: t('wizard.services.accounts_tab'), icon: 'i-lucide-key-square' },
  { value: 'usenet', label: t('wizard.services.usenet_tab'), icon: 'i-lucide-network' }
])
</script>

<template>
  <div class="space-y-5">
    <p class="max-w-3xl text-sm leading-6 text-muted">{{ t('wizard.services.intro') }}</p>
    <UTabs v-model="activeTab" :items="tabItems" variant="link" :unmount-on-hide="false">
      <template #content="{ item }">
        <div class="pt-4">
          <SettingsAccountsTab v-if="item.value === 'accounts'" hide-header />
          <SettingsUsenetTab v-else hide-header />
        </div>
      </template>
    </UTabs>
  </div>
</template>
