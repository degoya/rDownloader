import { useToast } from '@nuxt/ui/composables'
import { getCurrentScope, onScopeDispose, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { BUILD_VERSION, checkServiceVersion, differentServiceVersion } from '@/composables/serviceVersion'
import { onEventStreamOpened } from '@/composables/useEventStream'

const TOAST_ID = 'interface-outdated'

/**
 * Compares the service's version with this build's — now, and every time the event stream opens,
 * which is how a restarted service shows itself — and while they differ shows one toast with a
 * button to reload (RD-1120-16). The page never reloads by itself: a form half filled in would
 * be lost.
 */
export function useVersionNotice(): void {
  const toast = useToast()
  const { t } = useI18n()

  void checkServiceVersion()
  const stop = onEventStreamOpened(() => void checkServiceVersion())
  if (getCurrentScope()) onScopeDispose(stop)

  let shown = false
  // The version, not a flag: a second update while the toast stands rewrites its text.
  watch(differentServiceVersion, (version) => {
    if (!version) {
      if (shown) toast.remove(TOAST_ID)
      shown = false
      return
    }
    shown = true
    toast.add({
      id: TOAST_ID,
      title: t('nav.version.outdated'),
      description: t('nav.version.outdated_hint', { version, build: BUILD_VERSION }),
      color: 'info',
      icon: 'i-lucide-sparkles',
      duration: 0,
      actions: [{
        label: t('nav.version.reload'),
        icon: 'i-lucide-refresh-cw',
        color: 'neutral',
        variant: 'outline',
        onClick: () => window.location.reload()
      }]
    })
  }, { immediate: true })
}
