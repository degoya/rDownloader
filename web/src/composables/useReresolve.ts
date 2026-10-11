import { useToast } from '@nuxt/ui/composables'
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import { useErrorToast } from '@/composables/useErrorToast'
import { translateServerMessage } from '@/i18n/server'
import { bulkRefusals } from '@/stores/transfersShared'

/**
 * The downloads and packages being resolved right now, for the rows to say so (RD-1250-05).
 * Shared by every view: the request is one, the rows that show it are many.
 */
const resolvingNow = ref<ReadonlySet<string>>(new Set())

function mark(ids: readonly string[], on: boolean): void {
  const next = new Set(resolvingNow.value)
  for (const id of ids) {
    if (on) next.add(id)
    else next.delete(id)
  }
  resolvingNow.value = next
}

/** Whether a row is being resolved now: the file itself or its whole package. */
export function isResolvingNow(downloadId: string, packageId?: string): boolean {
  return resolvingNow.value.has(downloadId) || (packageId !== undefined && resolvingNow.value.has(packageId))
}

/**
 * Resolving downloads again with the plugin version installed now (RD-1210-01), at once
 * (RD-1250-05).
 *
 * A download keeps the plugin version that first resolved it; this drops that binding for the
 * selected files or packages and resolves them right away, without starting the download, so
 * size and name are known before. A running file is paused briefly by the service, a finished
 * one is left alone. Only links with an account are resolved ahead: a hoster link without one
 * would run the hoster's free flow, so the service skips it (owner, 2026-10-11). While the
 * request runs the rows say "resolving"; the toast then counts what resolved, failed and was
 * skipped, and names the reasons for the failures.
 */
export function useReresolve() {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()

  async function reresolve(target: { ids?: string[], packageIds?: string[] }): Promise<void> {
    const marked = [...target.ids ?? [], ...target.packageIds ?? []]
    mark(marked, true)
    try {
      const { data, error } = await api.POST('/api/v1/downloads/reresolve', {
        body: { ids: target.ids ?? [], package_ids: target.packageIds ?? [], check_now: true }
      })
      if (!data) {
        showError(t('downloads.reresolve.failed'), errorMessage(error))
        return
      }
      // A skipped link is counted in the title; its reason is the same for every one of them.
      const failures = data.results.flatMap(result => result.failure && !result.skipped ? [translateServerMessage(result.failure)] : [])
      const reasons = [...new Set([bulkRefusals(data), ...failures].filter(Boolean))].slice(0, 3).join(' · ')
      const checked = data.results.length > 0
      toast.add({
        title: !checked
          ? t('downloads.reresolve.done', { count: data.affected }, data.affected)
          : data.skipped > 0
            ? t('downloads.reresolve.checked_skipped', { resolved: data.resolved, failed: data.failed, skipped: data.skipped })
            : t('downloads.reresolve.checked', { resolved: data.resolved, failed: data.failed }),
        ...(reasons ? { description: reasons } : {}),
        color: reasons || data.failed > 0 ? 'warning' : 'success',
        icon: 'i-lucide-refresh-cw'
      })
    } finally {
      mark(marked, false)
    }
  }

  return { reresolve }
}
