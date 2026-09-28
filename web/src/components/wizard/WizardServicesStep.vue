<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Provider } from '@/api/types'
import ExtensionPairingModal from '@/components/settings/ExtensionPairingModal.vue'
import SettingsAccountsTab from '@/components/settings/SettingsAccountsTab.vue'
import SettingsUsenetTab from '@/components/settings/SettingsUsenetTab.vue'
import { useExtensionConnection } from '@/composables/useExtensionConnection'
import { providerText } from '@/i18n/plugins'

const { t } = useI18n()
const activeTab = ref('accounts')
const tabItems = ref([
  { value: 'accounts', label: t('wizard.services.accounts_tab'), icon: 'i-lucide-key-square' },
  { value: 'usenet', label: t('wizard.services.usenet_tab'), icon: 'i-lucide-network' }
])

const providers = ref<Provider[]>([])
const pairingOpen = ref(false)
/**
 * The providers whose account can take over the browser's sign-in (a `cookie_scope_host`), which
 * only a paired extension delivers (RD-150-17). Named in the hint, so the reader knows whether it
 * concerns them before an account waits for a session that cannot arrive.
 */
const browserSessionProviders = computed(() => providers.value
  .filter(provider => provider.credentials !== 'none' && provider.cookie_scope_host)
  .map(provider => providerText(provider.slug, 'name') ?? provider.display_name))
const { connected } = useExtensionConnection(isConnected => browserSessionProviders.value.length > 0 && !isConnected)
const pairingActions = computed(() => [{
  label: t('captcha.widget.extension_setup'),
  icon: 'i-lucide-puzzle',
  color: 'neutral' as const,
  variant: 'outline' as const,
  onClick: () => { pairingOpen.value = true }
}])

onMounted(async () => {
  const response = await api.GET('/api/v1/providers')
  if (response.data) providers.value = response.data
})
</script>

<template>
  <div class="space-y-5">
    <p class="max-w-3xl text-sm leading-6 text-muted">{{ t('wizard.services.intro') }}</p>
    <UAlert
      v-if="browserSessionProviders.length && !connected"
      color="warning"
      variant="subtle"
      icon="i-lucide-puzzle"
      :title="t('wizard.services.extension_title')"
      :description="t('wizard.services.extension_hint', { providers: browserSessionProviders.join(', ') })"
      :actions="pairingActions"
      data-testid="services-extension-hint"
    />
    <ExtensionPairingModal v-model:open="pairingOpen" />
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
