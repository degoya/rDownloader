import { useToast } from '@nuxt/ui/composables'
import { watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { useSitePicksStore } from '@/stores/sitePicks'

const TOAST_ID = 'site-pick-listed'

/**
 * The toast for a page whose releases wait for a choice, listed while the LinkGrabber is not on
 * screen — a copied link, the browser extension, Click'n'Load (RD-1190-17). "Choose" goes to the
 * LinkGrabber, whose panel opens its drawer on the page; on the LinkGrabber itself the drawer
 * opens at once and no toast is needed.
 */
export function useSitePickNotice(): void {
  const picks = useSitePicksStore()
  const toast = useToast()
  const router = useRouter()
  const { t } = useI18n()

  watch(() => picks.notice, (notice) => {
    if (!notice) return
    toast.add({
      id: TOAST_ID,
      title: t('linkgrabber.picks.announced', { rule: notice.rule }),
      description: t('linkgrabber.picks.summary', { count: notice.entries }, notice.entries),
      color: 'info',
      icon: 'i-lucide-list-checks',
      actions: [{
        label: t('linkgrabber.picks.open'),
        icon: 'i-lucide-panel-bottom-open',
        color: 'neutral',
        variant: 'outline',
        onClick: () => { void router.push({ name: 'linkgrabber' }) }
      }]
    })
  })
}
