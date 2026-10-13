import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'
import { useRoute, useRouter } from 'vue-router'

import { useIntakeModal } from '@/composables/useIntakeModal'
import { useCollectorStore } from '@/stores/collector'
import { sharedText } from '@/utils/sharedLinks'

/** Links into the LinkGrabber: pasted in the intake dialog, or shared from a mobile browser. */
export function useGrabberIntake() {
  const { t } = useI18n()
  const collector = useCollectorStore()
  const openIntake = useIntakeModal()
  const toast = useToast()
  const route = useRoute()
  const router = useRouter()

  /**
   * Takes over a link shared from a mobile browser (RD-090-08).
   *
   * The share target is a GET target, so the link arrives in the query string. It is consumed
   * once and removed from the URL: a reload must not hand the same link over a second time,
   * and the address bar should not keep showing what was shared.
   */
  async function handleSharedLinks(): Promise<void> {
    const text = sharedText(route.query as Record<string, unknown>)
    await router.replace({ path: route.path })
    if (!text) return
    await collector.collect({ text })
  }

  async function addLinks(): Promise<void> {
    const result = await openIntake()
    if (!result) return
    const outcome = await collector.collect({ text: result.text, ...(result.packageName ? { packageName: result.packageName } : {}), ...(result.password ? { password: result.password } : {}) })
    if (outcome.skippedExcluded) {
      toast.add({ title: t('linkgrabber.intake.skipped_excluded', { count: outcome.skippedExcluded }, outcome.skippedExcluded), color: 'warning', icon: 'i-lucide-shield-ban' })
    }
    if (outcome.skippedDisabled) {
      toast.add({ title: t('linkgrabber.intake.skipped_disabled', { count: outcome.skippedDisabled }, outcome.skippedDisabled), color: 'warning', icon: 'i-lucide-power-off' })
    }
    // Both numbers, not just the loss: "12 of 40" says a rule reached too far, while a bare
    // "12 dropped" reads like a fault in the paste (RD-110-07).
    if (outcome.listed) {
      toast.add({ title: t('linkgrabber.intake.listed', { count: outcome.listed }, outcome.listed), color: 'info', icon: 'i-lucide-list-checks' })
    }
    if (outcome.crawledDropped) {
      toast.add({ title: t('linkgrabber.intake.crawled_dropped', { count: outcome.crawledDropped, found: outcome.crawledFound }, outcome.crawledDropped), color: 'warning', icon: 'i-lucide-file-x' })
    }
  }

  return { addLinks, handleSharedLinks }
}
