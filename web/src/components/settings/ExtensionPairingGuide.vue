<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import { serviceUrl } from '@/basePath'
import { useExtensionConnection } from '@/composables/useExtensionConnection'

/**
 * Why and how the browser extension is paired, and whether one has reported in (RD-150-17).
 *
 * A hoster account that takes over the browser's sign-in (a provider with a `cookie_scope_host`,
 * DDownload for one) can only receive that session from the extension, so the place where the
 * extension is paired says so, lists the steps, and shows the live answer of the service instead
 * of leaving the reader to find out at the account. The pairing itself is the
 * `CapturePairingCard` beside it.
 */
const { t } = useI18n()
const { connected } = useExtensionConnection()
const origin = serviceUrl()
</script>

<template>
  <div class="space-y-3" data-testid="extension-pairing-guide">
    <UAlert
      :color="connected ? 'success' : 'neutral'"
      :icon="connected ? 'i-lucide-plug-zap' : 'i-lucide-puzzle'"
      :title="connected ? t('system.extension.connected_title') : t('system.extension.missing_title')"
      :description="connected ? t('system.extension.connected_description') : t('system.extension.missing_description')"
      data-testid="extension-status"
      :data-connected="connected"
    />
    <p class="max-w-3xl text-sm leading-6 text-muted">{{ t('system.extension.why') }}</p>
    <ol class="max-w-3xl list-decimal space-y-1 ps-5 text-sm leading-6 text-muted">
      <li>{{ t('system.extension.step_install') }}</li>
      <li>{{ t('system.extension.step_pair') }}</li>
      <li>{{ t('system.extension.step_options', { origin }) }}</li>
    </ol>
  </div>
</template>
