/**
 * The JSON files the settings hand out: the settings backup, site rules, routing and the per-area
 * bundles. Four components carried the same blob download by hand; the formats differ, the
 * mechanics do not. Taking one back in is `useJsonImport`.
 */

/** Offers `data` as a readable JSON download named `rdownloader-<name>-<YYYY-MM-DD>.json`. */
export function downloadJson(data: unknown, name: string): void {
  const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
  downloadBlob(blob, `rdownloader-${name}-${new Date().toISOString().slice(0, 10)}.json`)
}

/** Offers a file the service wrote — an exported link file (RD-1210-01) — under `fileName`. */
export function downloadBlob(blob: Blob, fileName: string): void {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = fileName
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  URL.revokeObjectURL(url)
}
