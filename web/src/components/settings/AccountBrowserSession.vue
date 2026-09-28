<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BrowserSession } from '@/api/types'
import ExtensionPairingModal from '@/components/settings/ExtensionPairingModal.vue'
import { useExtensionConnection } from '@/composables/useExtensionConnection'

/**
 * Where a request for the browser's session at an account's provider stands (RD-120-45).
 *
 * The service cannot read a browser's cookies. This line says who can — the extension, in the
 * browser that is signed in — and what the person does there; the extension asks once more for
 * that one site before anything is read. Without an extension, the pairing opens in place — in
 * the setup wizard a link to the settings would leave the wizard (RD-150-17) — and the service
 * is asked again while the request waits, so the warning goes once the extension reports in.
 */
const props = defineProps<{ session: BrowserSession }>()
const emit = defineEmits<{ cancel: [], dismiss: [] }>()

const { t } = useI18n()
const pairingOpen = ref(false)
const waiting = computed(() => props.session.state === 'waiting')
/** Whether a browser extension polled the service recently; only the service can know. */
const { connected: extensionConnected } = useExtensionConnection(connected => waiting.value && !connected)
const host = computed(() => props.session.host)
</script>

<template>
  <div class="mt-2 border border-muted bg-elevated p-3" data-testid="browser-session">
    <template v-if="session.state === 'waiting'">
      <p class="text-xs leading-5 text-muted">{{ t('network.account.browser_session.waiting', { host }) }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('network.account.browser_session.consent', { host }) }}</p>
      <div v-if="!extensionConnected" class="mt-2 space-y-2" data-testid="browser-session-extension-missing">
        <p class="text-xs leading-5 text-warning">{{ t('network.account.browser_session.extension_missing') }}</p>
        <UButton
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-puzzle"
          :label="t('captcha.widget.extension_setup')"
          @click="pairingOpen = true"
        />
        <ExtensionPairingModal v-model:open="pairingOpen" />
      </div>
      <UButton class="mt-2" size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('network.account.browser_session.cancel')" @click="emit('cancel')" />
    </template>
    <div v-else class="flex items-start gap-2">
      <p
        class="min-w-0 flex-1 text-xs leading-5"
        :class="session.state === 'delivered' ? 'text-success' : 'text-warning'"
      >
        {{ t(`network.account.browser_session.${session.state}`, { host }) }}
      </p>
      <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('network.account.browser_session.dismiss')" @click="emit('dismiss')" />
    </div>
  </div>
</template>
