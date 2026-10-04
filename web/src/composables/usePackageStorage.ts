import { useOverlay } from '@nuxt/ui/composables'

import PackageStorageModal from '@/components/storage/PackageStorageModal.vue'

interface PackageStorageOptions {
  packageId: string
  packageName: string
  downloads: { id: string, file_name: string, state: string }[]
}

/**
 * Opens a package's collision policy and duplicates (RD-150-01, RD-150-02); resolves with
 * whether anything was changed, so the caller knows to reload.
 */
export function usePackageStorage(): (options: PackageStorageOptions) => Promise<boolean> {
  const overlay = useOverlay()
  const modal = overlay.create(PackageStorageModal)
  return async (options): Promise<boolean> => {
    const instance = modal.open(options)
    return (await instance.result) === true
  }
}
