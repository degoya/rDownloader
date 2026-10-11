/**
 * A position in a video as people type it — `90`, `1:30`, `1:02:03` — to and from the seconds
 * the media section stores (RD-1240-15).
 */

const TIMECODE = /^(?:(\d+):)?(?:(\d+):)?(\d+)$/

/**
 * The seconds a typed position stands for: `null` for an empty field, `undefined` for one that
 * does not read as a time. Minutes and seconds behind a colon stay below 60.
 */
export function parseTimecode(text: string): number | null | undefined {
  const value = text.trim()
  if (!value) return null
  const match = TIMECODE.exec(value)
  if (!match) return undefined
  const parts = match.slice(1).filter((part): part is string => part !== undefined).map(Number)
  const [last, ...leading] = [...parts].reverse() as [number, ...number[]]
  if (leading.length && last > 59) return undefined
  if (leading.length === 2 && (leading[0] ?? 0) > 59) return undefined
  return parts.reduce((total, part) => total * 60 + part, 0)
}

/** Seconds as `m:ss` or `h:mm:ss`; the empty string for none. */
export function formatTimecode(seconds: number | null | undefined): string {
  if (seconds === null || seconds === undefined) return ''
  const hours = Math.floor(seconds / 3600)
  const minutes = Math.floor((seconds % 3600) / 60)
  const rest = String(seconds % 60).padStart(2, '0')
  return hours ? `${hours}:${String(minutes).padStart(2, '0')}:${rest}` : `${minutes}:${rest}`
}
