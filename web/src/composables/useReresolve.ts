import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import { useErrorToast } from '@/composables/useErrorToast'
import { bulkRefusals } from '@/stores/transfersShared'

/**
 * Resolving downloads again with the plugin version installed now (RD-1210-01).
 *
 * A download keeps the plugin version that first resolved it; this drops that binding for the
 * selected files or packages. A running file is paused and started again by the service, a
 * finished one is left alone, so the toast counts the files that resolve anew and names every
 * refusal.
 */
export function useReresolve() {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()

  async function reresolve(target: { ids?: string[], packageIds?: string[] }): Promise<void> {
    const { data, error } = await api.POST('/api/v1/downloads/reresolve', {
      body: { ids: target.ids ?? [], package_ids: target.packageIds ?? [] }
    })
    if (!data) {
      showError(t('downloads.reresolve.failed'), errorMessage(error))
      return
    }
    const refusals = bulkRefusals(data)
    toast.add({
      title: t('downloads.reresolve.done', { count: data.affected }, data.affected),
      ...(refusals ? { description: refusals } : {}),
      color: refusals ? 'warning' : 'success',
      icon: 'i-lucide-refresh-cw'
    })
  }

  return { reresolve }
}
