import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import { useConfirm } from '@/composables/useConfirm'
import { useSettingsStore } from '@/stores/settings'
import { useTransfersStore } from '@/stores/transfers'

/**
 * "Reset the package completely" (RD-1390-07): a failed package's files go from disk and it
 * downloads again from the start, as if it had just come from the LinkGrabber.
 *
 * The server decides what goes — only what the package demonstrably made, never a stranger's file
 * in the same folder — and refuses with a stable code before deleting anything. Here the
 * confirmation names the package and its number of files first, and says whether the unpacked
 * results go too, which the setting `package_reset_delete_extracted` decides.
 */
export function usePackageReset() {
  const { t } = useI18n()
  const transfers = useTransfersStore()
  const confirm = useConfirm()
  const busy = ref(false)

  async function resetPackage(packageId: string): Promise<void> {
    const name = transfers.packages.find(item => item.id === packageId)?.name ?? ''
    const count = transfers.downloads.filter(item => item.package_id === packageId).length
    const settings = await useSettingsStore().fetchSettings().then(response => response.data, () => undefined)
    const keepsExtracted = settings?.package_reset_delete_extracted === false
    const confirmed = await confirm({
      title: t('downloads.package_reset.title'),
      description: t(
        keepsExtracted ? 'downloads.package_reset.description_keep' : 'downloads.package_reset.description',
        { name, count },
        count
      ),
      confirmLabel: t('downloads.package_reset.confirm'),
      confirmIcon: 'i-lucide-trash-2',
      destructive: true
    })
    if (!confirmed) return
    busy.value = true
    try {
      const response = await api.POST('/api/v1/packages/{id}/reset', { params: { path: { id: packageId } } })
      await transfers.refresh()
      if (!response.data) {
        transfers.error = responseError(response)
        return
      }
      const reset = Number(response.data.params?.count ?? count)
      transfers.notice = t('downloads.package_reset.done', { count: reset }, reset)
    } finally {
      busy.value = false
    }
  }

  return { busy, resetPackage }
}
