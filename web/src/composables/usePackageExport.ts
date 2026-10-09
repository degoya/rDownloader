import { useOverlay, useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import PackageExportModal from '@/components/PackageExportModal.vue'
import { useErrorToast } from '@/composables/useErrorToast'
import { downloadBlob } from '@/utils/jsonFile'

export type PackageExportFormat = 'rdlinks' | 'crawljob'

/** What the export dialog answers: the file to write, and a passphrase or an empty string. */
export interface PackageExportChoice {
  format: PackageExportFormat
  passphrase: string
}

/** What to export; any mix, or `all` for every package of the download list. */
export interface PackageExportSelection {
  packageIds?: string[]
  downloadIds?: string[]
  collectorPackageIds?: string[]
  all?: boolean
}

/** The name the service gave the file in its `Content-Disposition`, or a dated one of our own. */
export function exportFileName(disposition: string | null, format: PackageExportFormat): string {
  const named = disposition?.match(/filename="([^"]+)"/)?.[1]
  return named ?? `rdownloader-${new Date().toISOString().slice(0, 10)}.${format}`
}

/**
 * Exporting packages as a link file (RD-1210-01): the dialog asks for the format and an optional
 * passphrase, the service writes the file, and the browser saves it. One instance per view — the
 * package rows emit and the view calls this, so a list of a thousand packages holds one dialog.
 */
export function usePackageExport() {
  const { t } = useI18n()
  const toast = useToast()
  const showError = useErrorToast()
  const modal = useOverlay().create(PackageExportModal)

  async function exportPackages(selection: PackageExportSelection): Promise<void> {
    const choice: unknown = await modal.open().result
    if (!choice || typeof choice !== 'object' || !('format' in choice)) return
    const { format, passphrase } = choice as PackageExportChoice
    const { data, error, response } = await api.POST('/api/v1/packages/export', {
      body: {
        package_ids: selection.packageIds ?? [],
        download_ids: selection.downloadIds ?? [],
        collector_package_ids: selection.collectorPackageIds ?? [],
        all: selection.all ?? false,
        format,
        ...(passphrase ? { passphrase } : {})
      },
      parseAs: 'blob'
    })
    if (!data) {
      showError(t('common.export.failed'), errorMessage(error))
      return
    }
    downloadBlob(data as Blob, exportFileName(response.headers.get('content-disposition'), format))
    const links = Number(response.headers.get('x-rd-export-links') ?? 0)
    const skipped = Number(response.headers.get('x-rd-export-skipped') ?? 0)
    toast.add({
      title: t('common.export.done', { count: links }, links),
      ...(skipped ? { description: t('common.export.skipped', { count: skipped }, skipped) } : {}),
      color: skipped ? 'warning' : 'success',
      icon: 'i-lucide-file-down'
    })
  }

  return { exportPackages }
}
