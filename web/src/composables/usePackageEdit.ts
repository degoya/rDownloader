import { useOverlay } from '@nuxt/ui/composables'

import type { PostprocessLevel } from '@/api/types'
import PackageEditModal from '@/components/PackageEditModal.vue'
import { usePostprocessStore } from '@/stores/postprocess'

export interface PackageEditResult {
  name: string
  password: string | null
  clearPassword: boolean
  /** null = inherit the category/global level */
  postprocessLevel: PostprocessLevel | null
  /** null = inherit the category/global script */
  script: string | null
  /** Rename the folder on disk as well, not only the label (RD-106-13). */
  renameFolder: boolean
}

export interface PackageEditOptions {
  name: string
  hasPassword: boolean
  /** The stored archive password, shown for editing (RD-104-04). */
  password: string | null
  postprocessLevel: PostprocessLevel | null
  script: string | null
  /** Offers "rename the folder too"; only the download list can move data. */
  canRenameFolder?: boolean
}

/** Editable package shape shared by the downloader and the LinkGrabber. */
interface EditablePackage {
  name: string
  has_password?: boolean
  password?: string | null
  postprocess_level?: PostprocessLevel | null
  script?: string | null
}

/** Diff between the modal result and the current package, in store `PackageChange` shape. */
export function packageEditChange(pkg: EditablePackage, result: PackageEditResult): { name?: string, password?: string | null, postprocessLevel?: PostprocessLevel | null, script?: string | null } {
  const change: ReturnType<typeof packageEditChange> = {}
  if (result.name !== pkg.name) change.name = result.name
  if (result.clearPassword) change.password = null
  else if (result.password !== (pkg.password ?? null)) change.password = result.password
  if (result.postprocessLevel !== (pkg.postprocess_level ?? null)) change.postprocessLevel = result.postprocessLevel
  if (result.script !== (pkg.script ?? null)) change.script = result.script
  return change
}

/** Opens the package editor; resolves with the edited values or null when cancelled. */
export function usePackageEdit(): (options: PackageEditOptions) => Promise<PackageEditResult | null> {
  const overlay = useOverlay()
  const modal = overlay.create(PackageEditModal)
  const postprocess = usePostprocessStore()

  return async (options): Promise<PackageEditResult | null> => {
    const scripts = await postprocess.loadScripts()
    const instance = modal.open({ ...options, scripts })
    const result = await instance.result
    return result && typeof result === 'object' && 'name' in result ? result as PackageEditResult : null
  }
}
