import { useToast } from '@nuxt/ui/composables'
import { watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import {
  failedViewRoute,
  serviceConnection,
  takeFailedViewRoute
} from '@/composables/serviceConnection'

const LOST_TOAST = 'service-connection-lost'
const VIEW_TOAST = 'service-view-failed'

/**
 * The toasts that go with the sidebar's connection dot: one when the service is lost, removed
 * once it is back, and one for a view whose code could not be fetched — that view is then loaded
 * by a full reload as soon as the service answers again, since a browser may keep the failed
 * module import cached for the life of the page.
 */
export function useConnectionNotices(): void {
  const toast = useToast()
  const router = useRouter()
  const { t } = useI18n()

  function reload(route: string): void {
    window.location.assign(router.resolve(route).href)
  }

  watch(serviceConnection, (state) => {
    if (state === 'disconnected') {
      toast.add({
        id: LOST_TOAST,
        title: t('nav.connection.lost'),
        description: t('nav.connection.lost_hint'),
        color: 'warning',
        icon: 'i-lucide-unplug',
        duration: 0
      })
      return
    }
    toast.remove(LOST_TOAST)
    const route = takeFailedViewRoute()
    if (route) reload(route)
  })

  watch(failedViewRoute, (route) => {
    if (!route) {
      toast.remove(VIEW_TOAST)
      return
    }
    toast.add({
      id: VIEW_TOAST,
      title: t('nav.connection.view_failed'),
      description: t('nav.connection.view_failed_hint'),
      color: 'warning',
      icon: 'i-lucide-file-x',
      duration: 0,
      actions: [{
        label: t('nav.connection.reload'),
        icon: 'i-lucide-refresh-cw',
        color: 'neutral',
        variant: 'outline',
        onClick: () => {
          const target = takeFailedViewRoute()
          if (target) reload(target)
        }
      }]
    })
  })
}
