import { readonly, ref, type Ref } from 'vue'

/**
 * Handoff state for app-wide NZB/torrent import requests: a window-level drop
 * stashes files here and navigates to `/linkgrabber`; the LinkGrabber view consumes them once
 * it (re)mounts. The keyboard shortcut reuses `requestFileImport()` with an empty file
 * list to open the modal without a drop. Kept as plain module state (not a Pinia store) since
 * it only ever has one reader.
 */

const pendingDropFiles = ref<File[]>([])
const requested = ref(false)
let modalOpen = false

/** Reactive signal for views that need to react to a request arriving while they're already mounted. */
export const fileImportRequested: Readonly<Ref<boolean>> = readonly(requested)

/** Stashes files (empty for the future `n` shortcut) and flags a pending import request. */
export function requestFileImport(files: File[] = []): void {
  pendingDropFiles.value = files
  requested.value = true
}

/** Consume-once: returns the pending request and clears it, or `null` if none is pending. */
export function consumeFileImportRequest(): { files: File[] } | null {
  if (!requested.value) return null
  const files = pendingDropFiles.value
  pendingDropFiles.value = []
  requested.value = false
  return { files }
}

/** Set around the import modal's lifetime so the window drop zone no-ops. */
export function setFileImportModalOpen(open: boolean): void {
  modalOpen = open
}

export function isFileImportModalOpen(): boolean {
  return modalOpen
}

let dropClaim: ((files: File[]) => void) | null = null

/**
 * A page that takes dropped files itself, instead of the LinkGrabber (RD-120-51).
 *
 * The remote-job form hands `.torrent`/`.nzb` files to a provider account; a drop on that page
 * that navigated to the LinkGrabber and imported them there would do the very thing the reader
 * went to this page not to do. While a claim is held the window drop zone shows no overlay and
 * hands every drop to `take`. Returns the release; a newer claim is never released by an older
 * holder.
 */
export function claimFileDrops(take: (files: File[]) => void): () => void {
  dropClaim = take
  return () => {
    if (dropClaim === take) dropClaim = null
  }
}

/** Who takes a drop instead of the LinkGrabber, or `null`. */
export function fileDropClaim(): ((files: File[]) => void) | null {
  return dropClaim
}

/**
 * The metadata formats the LinkGrabber accepts, in one place: the drop zone, the import modal's
 * file list and its package-name suggestion all read from here, so a new format cannot be added
 * to one of them and silently forgotten in the others.
 */
const IMPORT_SUFFIX = /\.(?:nzb|torrent|dlc|ccf|rsdf|txt|text)$/i

/** Keeps supported LinkGrabber metadata files. Filenames are only readable at drop time. */
export function filterImportFiles(files: FileList | File[]): File[] {
  return Array.from(files).filter(file => IMPORT_SUFFIX.test(file.name))
}

/** Drops the metadata suffix and the `{{password}}` marker so package and folder read cleanly. */
export function importPackageNameOf(fileName: string): string {
  const cleaned = fileName.replace(IMPORT_SUFFIX, '').replace(/\{\{.*\}\}/, '').trim()
  return cleaned || fileName
}
