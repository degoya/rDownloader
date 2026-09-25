<script setup lang="ts">
import * as uiLocales from '@nuxt/ui/locale'
import { computed, onBeforeUnmount, onMounted, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { onSessionLost } from '@/api/client'
import AuthGate from '@/components/AuthGate.vue'
import ControlRoomLayout from '@/components/ControlRoomLayout.vue'
import SetupWizard from '@/components/wizard/SetupWizard.vue'
import { useDocumentTitle } from '@/composables/useDocumentTitle'
import { useCaptchasStore } from '@/stores/captchas'
import { useCollectorStore } from '@/stores/collector'
import { useSessionStore } from '@/stores/session'
import { useStreamsStore } from '@/stores/streams'
import { useTransfersStore } from '@/stores/transfers'
import { loadDisplaySettings } from '@/utils/loadDisplaySettings'

const session = useSessionStore()
const transfers = useTransfersStore()
// Connected app-wide (not just on the LinkGrabber route) so the nav badge stays live.
const collector = useCollectorStore()
// Loaded app-wide for the nav badge; the Streams route reuses the same store.
const streams = useStreamsStore()
// A parked download can ask for a captcha at any time, on any route.
const captchas = useCaptchasStore()
const { t, locale } = useI18n()
// The one place the browser tab is written; every view leaves it alone (RD-106-07).
useDocumentTitle()
const uiLocale = computed(() => ({ en: uiLocales.en, de: uiLocales.de, fr: uiLocales.fr, es: uiLocales.es })[locale.value as 'en' | 'de' | 'fr' | 'es'] ?? uiLocales.en)

onMounted(() => void session.initialize())
// Any request refused for want of a session sends the whole interface back to the sign-in,
// with the reason, rather than leaving each view to fail on its own (RD-130-09).
const stopListening = onSessionLost(() => session.expire())
onBeforeUnmount(() => {
  stopListening()
  transfers.disconnectEvents()
  collector.disconnectEvents()
  captchas.disconnectEvents()
})

watch(() => session.ready, (ready) => {
  if (ready) {
    void transfers.refresh()
    transfers.connectEvents()
    void collector.refresh()
    collector.connectEvents()
    void streams.refresh()
    // One request at startup: byte formatting is a server setting and applies on every view,
    // so it has to be known before the first list renders.
    void loadDisplaySettings()
    void captchas.refresh()
    captchas.connectEvents()
  }
}, { immediate: true })
</script>

<template>
  <UApp :locale="uiLocale">
    <div v-if="!session.initialized" class="signal-grid flex min-h-screen items-center justify-center">
      <div class="flex items-center gap-3 text-toned">
        <UIcon name="i-lucide-loader-circle" class="size-5 animate-spin text-primary" />
        <span class="font-mono text-sm">{{ t('common.app.loading') }}</span>
      </div>
    </div>
    <SetupWizard v-else-if="session.wizardActive" />
    <AuthGate v-else-if="!session.ready" />
    <!--
      `data-vaul-drawer-wrapper` is what a drawer opened with `should-scale-background` looks for:
      it scales this element back while it is open, which is what makes the page read as lying
      behind the drawer rather than beside it. Without the attribute the prop is silently a no-op.
    -->
    <div v-else data-vaul-drawer-wrapper class="min-h-screen bg-default">
      <ControlRoomLayout />
    </div>
  </UApp>
</template>
