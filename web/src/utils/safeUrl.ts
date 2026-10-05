/**
 * A remote address the interface may put into an `href`, or `undefined` (audit K9).
 *
 * Addresses from plugins, extractors, feeds and manifests reach the page as data, and Vue
 * binds `javascript:` into an `href` as readily as `https:`. Only an absolute http(s) address
 * becomes a link; anything else is shown as text by the caller or not linked at all. The
 * address is returned as given: the browser parses an `href` exactly as `URL` does here.
 */
export function safeHttpUrl(value: string | null | undefined): string | undefined {
  if (!value) return undefined
  try {
    const url = new URL(value)
    return url.protocol === 'http:' || url.protocol === 'https:' ? value : undefined
  } catch {
    return undefined
  }
}
