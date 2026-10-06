<script setup lang="ts">
/**
 * The notice at the top of a service's settings page while that service is switched off
 * (RD-1120-23). The page stays editable — its settings apply once the service is back on — but
 * nobody should tune BitTorrent for half an hour before finding out it does not run. The action
 * leads to the switch on the services page through its search anchor.
 */
import { useI18n } from 'vue-i18n'

import { useSettingsLink } from '@/composables/useSettingsLink'

defineProps<{
  /** The key under `settings.services` that names it, as the switch does. */
  service: 'torrent' | 'usenet' | 'media' | 'gallery' | 'recording' | 'remote'
  /** The service's `*_service_enabled` setting; nothing shows while it is on. */
  enabled: boolean
}>()

const { t } = useI18n()
const { to, reveal } = useSettingsLink('services.switches')
</script>

<template>
  <UAlert
    v-if="!enabled"
    color="warning"
    variant="subtle"
    icon="i-lucide-power-off"
    :title="t('settings.services.off.title', { name: t(`settings.services.${service}.label`) })"
    :description="t('settings.services.off.description')"
    :data-service-off="service"
  >
    <template #actions>
      <UButton
        size="xs"
        color="warning"
        variant="outline"
        icon="i-lucide-toggle-left"
        :to="to"
        :label="t('settings.services.off.open')"
        @click="reveal"
      />
    </template>
  </UAlert>
</template>
