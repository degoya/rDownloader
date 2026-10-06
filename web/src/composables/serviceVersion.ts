/**
 * Which version the service runs, against the version this page was built as (RD-1120-16).
 *
 * A single-page app never fetches `index.html` again on its own, so a tab left open across an
 * update keeps the old interface while the status bar already names the new version.
 * `useVersionNotice` turns a difference into the toast that offers the reload.
 */

import { computed, readonly, ref } from 'vue'

import { api } from '@/api/client'

/**
 * The version this bundle was built as: `web/package.json`'s, which `scripts/set-version.sh`
 * writes with every other copy, handed in by Vite's `define`.
 */
export const BUILD_VERSION: string = __RD_BUILD_VERSION__

const reported = ref<string | null>(null)

/** The version the service answers as, or `null` until it has; the status bar shows it. */
export const serviceVersion = readonly(reported)

/** The service's version while it is not this page's, otherwise `null`. */
export const differentServiceVersion = computed(() =>
  reported.value !== null && reported.value !== BUILD_VERSION ? reported.value : null)

/** Asks the service for its version. A failed request changes nothing. */
export async function checkServiceVersion(): Promise<void> {
  const response = await api.GET('/api/v1/health')
  const version = (response.data as { version?: string } | undefined)?.version
  if (version) reported.value = version
}

/** Test seam: forgets the service's answer. */
export function resetServiceVersionForTests(): void {
  reported.value = null
}
