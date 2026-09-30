/**
 * The JSON files the settings hand out and take back: the settings backup, site rules, routing
 * and the per-area bundles. Four components carried the same blob download and the same hidden
 * file input by hand; the formats differ, the mechanics do not.
 */

/** Offers `data` as a readable JSON download named `rdownloader-<name>-<YYYY-MM-DD>.json`. */
export function downloadJson(data: unknown, name: string): void {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = `rdownloader-${name}-${new Date().toISOString().slice(0, 10)}.json`
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
}

/**
 * Opens a hidden `<input type="file">`. Cleared first, so choosing the same file a second time
 * still fires `change`.
 */
export function openFilePicker(input: HTMLInputElement | null): void {
  if (!input) return
  input.value = ''
  input.click()
}

/** The file a file input's `change` event carries, or null when none was chosen. */
export function chosenFile(event: Event): File | null {
  const target = event.target
  return target instanceof HTMLInputElement ? target.files?.item(0) ?? null : null
}
