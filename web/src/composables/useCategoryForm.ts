import { computed, reactive, type WritableComputedRef } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, CreateCategory, PackageNameRegex, PackageNameRulesOverride, PostprocessLevel, SortTemplates } from '@/api/types'
import { usePostprocessStore } from '@/stores/postprocess'
import { INHERIT_LEVEL, postprocessLevelItems } from '@/utils/format'

type Switchable = 'recursive_unpack' | 'unpack_to_subfolder' | 'direct_unpack' | 'malware_scan' | 'sfv_verify'
  | 'safe_postproc' | 'delete_par2' | 'upload_enabled' | 'unwrap_package_folder'

/** The three sort templates as the form holds them (RD-1100-08); an empty one sorts nothing. */
export interface SortingForm { series: string, dated: string, movie: string }

/** The form's sort templates from a stored category's. */
export function sortingForm(sorting: SortTemplates | null | undefined): SortingForm {
  return { series: sorting?.series ?? '', dated: sorting?.dated ?? '', movie: sorting?.movie ?? '' }
}

/** What the post-processing endpoint takes for the form's templates: `null` when all are empty. */
export function sortingBody(form: SortingForm): SortTemplates | null {
  const series = form.series.trim()
  const dated = form.dated.trim()
  const movie = form.movie.trim()
  return series || dated || movie ? { series: series || null, dated: dated || null, movie: movie || null } : null
}

/**
 * The category's post-processing fields in the body of `PATCH /api/v1/categories/{id}/postprocess`.
 * The endpoint replaces every field it carries, so the ones not being changed are passed back as
 * they are; the plugin steps, the sort templates and the package-name rules are the caller's — the
 * category the create and update routes answer with does not carry them.
 */
export function categoryPostprocessBody(
  category: Category,
  pluginSteps: string[] | null,
  sorting: SortTemplates | null,
  naming: { rules: PackageNameRulesOverride | null, regex: PackageNameRegex[] | null } = {
    rules: category.package_name_rules ?? null,
    regex: category.package_name_regex ?? null
  }
) {
  return {
    postprocess_level: category.postprocess_level ?? null,
    script: category.script ?? null,
    cleanup_extensions: category.cleanup_extensions ?? null,
    recursive_unpack: category.recursive_unpack ?? null,
    unpack_to_subfolder: category.unpack_to_subfolder ?? null,
    direct_unpack: category.direct_unpack ?? null,
    malware_scan: category.malware_scan ?? null,
    sfv_verify: category.sfv_verify ?? null,
    safe_postproc: category.safe_postproc ?? null,
    delete_par2: category.delete_par2 ?? null,
    plugin_steps: pluginSteps,
    upload_enabled: category.upload_enabled ?? null,
    upload_remote: category.upload_remote ?? null,
    sorting,
    unwrap_package_folder: category.unwrap_package_folder ?? null,
    package_name_rules: naming.rules,
    package_name_regex: naming.regex
  }
}

/**
 * The category editor's form (`RoutingCategories.vue`): its fields, the selects over them, and
 * how it is emptied or filled from a stored category.
 *
 * Every nullable switch is a three-way select — inherit, on, off — where "inherit" sends `null`.
 */
export function useCategoryForm() {
  const { t } = useI18n()
  const postprocess = usePostprocessStore()
  const form = reactive<CreateCategory>({
    name: '',
    color: '#38BDF8',
    storage_root_id: '',
    relative_path: '',
    is_default: false,
    postprocess_level: null,
    script: null,
    cleanup_extensions: null,
    recursive_unpack: null,
    unpack_to_subfolder: null,
    direct_unpack: null,
    malware_scan: null,
    sfv_verify: null,
    safe_postproc: null,
    delete_par2: null,
    upload_enabled: null,
    upload_remote: null,
    unwrap_package_folder: null
  })

  /** One inherit/on/off select over `field`; `labels` gives the three item labels in that order. */
  function inheritable(field: Switchable, labels: () => [string, string, string]) {
    const items = computed(() => {
      const [inherit, on, off] = labels()
      return [
        { label: inherit, value: INHERIT_LEVEL },
        { label: on, value: 'on' },
        { label: off, value: 'off' }
      ]
    })
    const value: WritableComputedRef<string> = computed({
      get: () => form[field] == null ? INHERIT_LEVEL : (form[field] ? 'on' : 'off'),
      set: (next: string) => { form[field] = next === INHERIT_LEVEL ? null : next === 'on' }
    })
    return { items, value }
  }

  const levelItems = computed(() => postprocessLevelItems())
  const scriptItems = computed(() => [
    { label: t('routing.category.script_inherit'), value: INHERIT_LEVEL },
    ...postprocess.scripts.map(script => ({ label: script, value: script }))
  ])
  const level = computed({
    get: () => form.postprocess_level ?? INHERIT_LEVEL,
    set: (value: string) => { form.postprocess_level = value === INHERIT_LEVEL ? null : value as PostprocessLevel }
  })
  const script = computed({
    get: () => form.script ?? INHERIT_LEVEL,
    set: (value: string) => { form.script = value === INHERIT_LEVEL ? null : value }
  })
  const upload = inheritable('upload_enabled', () =>
    [t('routing.category.upload_inherit'), t('routing.category.upload_on'), t('routing.category.upload_off')])
  const uploadRemote = computed({
    get: () => form.upload_remote ?? '',
    set: (value: string) => { form.upload_remote = value.trim() ? value : null }
  })
  const recursive = inheritable('recursive_unpack', () =>
    [t('routing.category.recursive_inherit'), t('routing.category.recursive_on'), t('routing.category.recursive_off')])
  const subfolder = inheritable('unpack_to_subfolder', () =>
    [t('routing.category.subfolder_inherit'), t('routing.category.subfolder_on'), t('routing.category.subfolder_off')])
  const directUnpack = inheritable('direct_unpack', () =>
    [t('routing.category.direct_unpack_inherit'), t('routing.category.direct_unpack_on'), t('routing.category.direct_unpack_off')])
  const malwareScan = inheritable('malware_scan', () =>
    [t('routing.category.malware_scan_inherit'), t('routing.category.malware_scan_on'), t('routing.category.malware_scan_off')])
  const sfv = inheritable('sfv_verify', () =>
    [t('routing.category.sfv_inherit'), t('routing.category.sfv_on'), t('routing.category.sfv_off')])
  const safePostproc = inheritable('safe_postproc', () =>
    [t('routing.category.safe_postproc_inherit'), t('routing.category.safe_postproc_on'), t('routing.category.safe_postproc_off')])
  const deletePar2 = inheritable('delete_par2', () =>
    [t('routing.category.delete_par2_inherit'), t('routing.category.delete_par2_on'), t('routing.category.delete_par2_off')])
  const unwrap = inheritable('unwrap_package_folder', () =>
    [t('routing.category.unwrap_inherit'), t('routing.category.unwrap_on'), t('routing.category.unwrap_off')])

  /** Empties the form for a new category on `rootId`. */
  function clear(rootId: string): void {
    form.name = ''
    form.color = '#38BDF8'
    form.storage_root_id = rootId
    form.relative_path = ''
    form.is_default = false
    form.postprocess_level = null
    form.script = null
    form.recursive_unpack = null
    form.unpack_to_subfolder = null
    form.direct_unpack = null
    form.malware_scan = null
    form.sfv_verify = null
    form.safe_postproc = null
    form.delete_par2 = null
    form.upload_enabled = null
    form.upload_remote = null
    form.unwrap_package_folder = null
  }

  /** Fills the form from a stored category; the cleanup list and plugin steps are the caller's. */
  function fill(category: Category): void {
    form.name = category.name
    form.color = category.color
    form.storage_root_id = category.storage_root_id
    form.relative_path = category.relative_path
    form.is_default = category.is_default
    form.postprocess_level = category.postprocess_level ?? null
    form.script = category.script ?? null
    form.recursive_unpack = category.recursive_unpack ?? null
    form.unpack_to_subfolder = category.unpack_to_subfolder ?? null
    form.direct_unpack = category.direct_unpack ?? null
    form.malware_scan = category.malware_scan ?? null
    form.sfv_verify = category.sfv_verify ?? null
    form.safe_postproc = category.safe_postproc ?? null
    form.delete_par2 = category.delete_par2 ?? null
    form.upload_enabled = category.upload_enabled ?? null
    form.upload_remote = category.upload_remote ?? null
    form.unwrap_package_folder = category.unwrap_package_folder ?? null
  }

  return {
    form,
    levelItems,
    scriptItems,
    level,
    script,
    uploadItems: upload.items,
    upload: upload.value,
    uploadRemote,
    recursiveItems: recursive.items,
    recursiveUnpack: recursive.value,
    subfolderItems: subfolder.items,
    unpackToSubfolder: subfolder.value,
    directUnpackItems: directUnpack.items,
    directUnpack: directUnpack.value,
    malwareScanItems: malwareScan.items,
    malwareScan: malwareScan.value,
    sfvItems: sfv.items,
    sfvVerify: sfv.value,
    safePostprocItems: safePostproc.items,
    safePostproc: safePostproc.value,
    deletePar2Items: deletePar2.items,
    deletePar2: deletePar2.value,
    unwrapItems: unwrap.items,
    unwrapPackageFolder: unwrap.value,
    clear,
    fill
  }
}
