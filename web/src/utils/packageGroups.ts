import { ref, type Ref } from 'vue'

/** The two lists of collapsible package groups (RD-1170-01). */
export type PackageGroupPlace = 'downloads' | 'linkgrabber'

/**
 * Whether a package nobody has opened or closed yet starts closed, one switch per list.
 *
 * `ref`s like `showNzbHandOver`: a saved change has to reach the list on screen, not wait for a
 * reload. The defaults are what each list did before the switches existed — the queue closed, the
 * LinkGrabber open. What the user opened or closed by hand stays so either way.
 */
export const packagesClosedByDefault: Record<PackageGroupPlace, Ref<boolean>> = {
  downloads: ref(true),
  linkgrabber: ref(false)
}

export function setPackagesClosedByDefault(settings: {
  downloads_packages_closed_by_default?: boolean | null
  linkgrabber_packages_closed_by_default?: boolean | null
}): void {
  packagesClosedByDefault.downloads.value = settings.downloads_packages_closed_by_default !== false
  packagesClosedByDefault.linkgrabber.value = settings.linkgrabber_packages_closed_by_default === true
}
