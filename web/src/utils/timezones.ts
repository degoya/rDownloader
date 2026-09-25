/**
 * IANA time zone names for the schedule pickers.
 *
 * `Intl.supportedValuesOf` is the browser's own list and needs no bundled data, but it is not
 * everywhere yet — the fallback covers the zones a European installation realistically uses plus
 * the browser's own, so the field is never empty and never loses the value it already had.
 */
const FALLBACK = [
  'UTC',
  'Europe/Berlin', 'Europe/Vienna', 'Europe/Zurich', 'Europe/London', 'Europe/Paris',
  'Europe/Madrid', 'Europe/Rome', 'Europe/Amsterdam', 'Europe/Brussels', 'Europe/Prague',
  'Europe/Warsaw', 'Europe/Stockholm', 'Europe/Helsinki', 'Europe/Athens', 'Europe/Istanbul',
  'Europe/Moscow', 'Europe/Lisbon', 'Europe/Dublin', 'Europe/Copenhagen', 'Europe/Oslo',
  'America/New_York', 'America/Chicago', 'America/Denver', 'America/Los_Angeles',
  'America/Sao_Paulo', 'Asia/Dubai', 'Asia/Kolkata', 'Asia/Shanghai', 'Asia/Tokyo',
  'Asia/Seoul', 'Australia/Sydney', 'Pacific/Auckland'
]

/** The zone the browser is set to, or UTC where that cannot be determined. */
export function browserTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'
  } catch {
    return 'UTC'
  }
}

/**
 * Every selectable zone, sorted, with `current` guaranteed to be present — a schedule saved with
 * a zone this browser does not list must still show what it is set to rather than appearing
 * blank or silently changing.
 */
export function timezoneOptions(current?: string | null): string[] {
  let zones: string[]
  try {
    const supported = (Intl as { supportedValuesOf?: (key: string) => string[] }).supportedValuesOf
    zones = supported ? [...supported('timeZone')] : [...FALLBACK]
  } catch {
    zones = [...FALLBACK]
  }
  if (!zones.includes('UTC')) zones.push('UTC')
  if (current && !zones.includes(current)) zones.push(current)
  return zones.sort((a, b) => a.localeCompare(b))
}
