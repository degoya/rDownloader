import { useToast } from '@nuxt/ui/composables'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LinkFilterApplyResponse } from '@/api/types'
import { useConfirm } from '@/composables/useConfirm'
import { useCollectorStore } from '@/stores/collector'

/**
 * "Apply LinkFilter" for both buttons, the LinkGrabber's and the settings' (RD-1240-09).
 *
 * A dry run counts first; when a delete rule would delete links, the person is asked with that
 * number before anything changes (RD-1310-01). At intake and after the online check a delete
 * rule deletes without asking — the rule is the consent; only this bulk pass asks.
 */
export function useApplyLinkFilters() {
  const { t } = useI18n()
  const toast = useToast()
  const confirm = useConfirm()
  const collector = useCollectorStore()
  const applying = ref(false)

  /** Answers what changed, or `null` when it failed (the store's `error` says why) or was declined. */
  async function apply(): Promise<LinkFilterApplyResponse | null> {
    applying.value = true
    try {
      const preview = await collector.applyLinkFilters(true)
      if (!preview) return null
      if (preview.deleted > 0 && !await confirm({
        title: t('linkgrabber.link_filter.delete_title'),
        description: t('linkgrabber.link_filter.delete_description', { count: preview.deleted }, preview.deleted),
        confirmLabel: t('linkgrabber.link_filter.delete_confirm'),
        confirmIcon: 'i-lucide-trash-2',
        destructive: true
      })) return null
      const outcome = await collector.applyLinkFilters()
      if (outcome) {
        toast.add({
          title: t('linkgrabber.link_filter.applied', { hidden: outcome.hidden, shown: outcome.shown, routed: outcome.routed, deleted: outcome.deleted }),
          color: 'success',
          icon: 'i-lucide-filter'
        })
      }
      return outcome
    } finally {
      applying.value = false
    }
  }

  return { applying, apply }
}
