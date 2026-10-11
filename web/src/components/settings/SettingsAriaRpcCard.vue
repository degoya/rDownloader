<script setup lang="ts">
/**
 * The aria2 JSON-RPC switch (RD-1240-11): AriaNg, Motrix-style front ends, browser extensions and
 * Android remotes add links and watch the queue through `/jsonrpc`. Off by default; the secret
 * they ask for is an API token with the three areas the SABnzbd and qBittorrent adapters need,
 * issued on the card above.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import { serviceUrl } from '@/basePath'
import CopyField from '@/components/CopyField.vue'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const endpoint = serviceUrl('/jsonrpc')
</script>

<template>
  <UCard as="section" data-settings-anchor="clients.aria2" :ui="{ body: 'space-y-4' }">
    <SectionHeader
      :eyebrow="t('settings.aria2.eyebrow')"
      :title="t('settings.aria2.title')"
      :description="t('settings.aria2.description')"
      level="sub"
    />
    <UFormField :label="t('settings.aria2.enabled.label')" :description="t('settings.aria2.enabled.description')" orientation="horizontal" class="border-t border-muted pt-4">
      <USwitch v-model="settings.aria2_rpc_enabled" data-testid="aria2-switch" />
    </UFormField>
    <div v-if="settings.aria2_rpc_enabled" class="space-y-2" data-testid="aria2-connection">
      <p class="text-sm text-muted">{{ t('settings.aria2.endpoint') }}</p>
      <CopyField :value="endpoint" :label="t('settings.aria2.copy_endpoint')" />
      <p class="text-xs text-muted">{{ t('settings.aria2.secret') }}</p>
    </div>
  </UCard>
</template>
