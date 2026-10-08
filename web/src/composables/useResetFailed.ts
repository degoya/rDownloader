import { computed, ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Download, DownloadState } from '@/api/types'
import { useResetConfirm } from '@/composables/useResetConfirm'
import { useTransfersStore } from '@/stores/transfers'
import { bulkRefusals } from '@/stores/transfersShared'

/** What "reset failed" takes: the failed files, the blocked ones, or both (RD-1190-15). */
export type StuckKind = 'failed' | 'blocked' | 'both'
export const STUCK_KINDS: readonly StuckKind[] = ['failed', 'blocked', 'both']

/**
 * The states behind each kind. A cancelled file is not among them: it was stopped on purpose,
 * and a one-click reset should not start what somebody cancelled.
 */
export const STUCK_STATES: Readonly<Record<StuckKind, readonly DownloadState[]>> = {
  failed: ['failed'],
  blocked: ['blocked'],
  both: ['failed', 'blocked']
}

/**
 * Resetting every failed or blocked file at once — of the list as it is filtered, or of one
 * package — without selecting them one by one (RD-1190-15).
 *
 * The server is asked by state (`filter` on the bulk endpoint) wherever that names exactly the
 * files on screen: always for a package, and for the list unless a name search narrows it. With
 * a search the list shows only some of the files in those states, so those go by id. Either way
 * the confirmation names the files and their number first.
 */
export function useResetFailed(view: {
  /** The files the list shows, after the state filter and the name search. */
  visible: Ref<Download[]>
  /** The name search in effect; empty when none narrows the list. */
  needle: Ref<string>
}) {
  const { t } = useI18n()
  const transfers = useTransfersStore()
  const confirmReset = useResetConfirm()
  const busy = ref(false)

  const counts = computed<Record<StuckKind, number>>(() => {
    const failed = view.visible.value.filter(item => item.state === 'failed').length
    const blocked = view.visible.value.filter(item => item.state === 'blocked').length
    return { failed, blocked, both: failed + blocked }
  })

  /** Every file of the shown list in the kind's states. */
  async function resetShown(kind: StuckKind): Promise<void> {
    const states = STUCK_STATES[kind]
    const targets = view.visible.value.filter(item => states.includes(item.state))
    const filter = view.needle.value
      ? null
      : { states: [...new Set(targets.map(item => item.state))] }
    await run(targets, filter)
  }

  /** Every failed and blocked file of one package, whatever the list filter shows of it. */
  async function resetPackage(packageId: string): Promise<void> {
    const states = STUCK_STATES.both
    const targets = transfers.downloads.filter(item => item.package_id === packageId && states.includes(item.state))
    await run(targets, { states: [...states], package_id: packageId })
  }

  async function run(targets: Download[], filter: { states: DownloadState[], package_id?: string } | null): Promise<void> {
    if (!targets.length) {
      transfers.notice = t('downloads.notices.nothing_to_reset')
      return
    }
    const answer = await confirmReset(targets.map(item => item.file_name), false)
    if (!answer.confirmed) return
    busy.value = true
    try {
      if (!filter) {
        await transfers.reset(targets.map(item => item.id), false)
        return
      }
      const response = await api.POST('/api/v1/downloads/bulk', { body: { ids: [], action: 'reset', filter } })
      await transfers.refresh()
      if (!response.data) {
        transfers.error = responseError(response)
        return
      }
      transfers.error = bulkRefusals(response.data)
      transfers.notice = t('downloads.reset_failed.done', { count: response.data.affected }, response.data.affected)
    } finally {
      busy.value = false
    }
  }

  return { busy, counts, resetShown, resetPackage }
}
