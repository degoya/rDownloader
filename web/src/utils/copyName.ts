/**
 * Builds the first free, length-safe copy name in the current locale.
 *
 * Written for category rules and generalised when subscriptions gained the same action: the only
 * thing that differed was the length the server accepts, so that is a parameter rather than a
 * second copy of the logic. The suffix is measured in code points, not UTF-16 units, so a name
 * ending in an emoji is trimmed to something the server will still accept.
 */
export function duplicateName(
  original: string,
  existingNames: Iterable<string>,
  copyLabel: string,
  maxLength: number
): string {
  const used = new Set(existingNames)
  for (let index = 1; index <= used.size + 1; index += 1) {
    const suffix = index === 1 ? ` (${copyLabel})` : ` (${copyLabel} ${index})`
    const available = Math.max(0, maxLength - Array.from(suffix).length)
    const base = Array.from(original.trim()).slice(0, available).join('').trimEnd()
    const candidate = `${base}${suffix}`
    if (!used.has(candidate)) return candidate
  }
  // The loop must find a free name because it checks one more candidate than there are names.
  return original
}
