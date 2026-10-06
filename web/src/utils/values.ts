/**
 * Small value helpers several modules used to carry a copy of (RD-1120-15). No imports, so the
 * i18n resolver and the API client can use them without a cycle.
 */

/** A form field's text without its surrounding blanks, or `null` when nothing is left. */
export function trimmed(value: string): string | null {
  const text = value.trim()
  return text.length > 0 ? text : null
}

/** Anything whose properties can be read by name — an array included. */
export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

/** An object that is not an array: a JSON object rather than a list. */
export function isPlainRecord(value: unknown): value is Record<string, unknown> {
  return isRecord(value) && !Array.isArray(value)
}

/** The last segment of a path, `/` or `\` separated. */
export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path
}
