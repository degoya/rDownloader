import { computed, watch } from 'vue'

import { useOpenSections } from '@/composables/useOpenSections'
import { packagesClosedByDefault, type PackageGroupPlace } from '@/utils/packageGroups'

/** Per browser, like the column widths; the queue keeps the key it has had since RD-106-12. */
const STORAGE_KEYS: Record<PackageGroupPlace, string> = {
  downloads: 'rdownloader-open-packages',
  linkgrabber: 'rdownloader-open-packages-linkgrabber'
}

/**
 * Which packages of the queue or the LinkGrabber are open, remembered across reloads, with
 * "open all" and "close all" over what the list shows (RD-1170-01).
 *
 * `known` is every package the list holds, filters aside: what is not among them any more is
 * forgotten. An empty list is never taken for "all gone" — it is also what a list looks like
 * before its first fetch. `shown` is what the filters leave, which is what "all" means.
 */
export function usePackageOpenState(place: PackageGroupPlace, packages: {
  known: () => string[]
  shown: () => string[]
}) {
  const sections = useOpenSections({
    storageKey: STORAGE_KEYS[place],
    defaultOpen: () => !packagesClosedByDefault[place].value
  })
  watch(packages.known, ids => { if (ids.length) sections.prune(ids) }, { immediate: true })

  /** Every shown package is open: the header button then closes them. */
  const allOpen = computed(() => {
    const ids = packages.shown()
    return ids.length > 0 && ids.every(sections.isOpen)
  })

  const openAll = (): void => sections.setAll(packages.shown(), true)
  const closeAll = (): void => sections.setAll(packages.shown(), false)
  const toggleAll = (): void => (allOpen.value ? closeAll : openAll)()

  return { ...sections, allOpen, openAll, closeAll, toggleAll }
}
