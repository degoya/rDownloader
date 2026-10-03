/**
 * The page a link came from, when the data knows one and it is a web page: a captured browser
 * download's referrer, or the page a media link was extracted from. Anything that is not
 * `http(s)` is withheld rather than opened — a referrer is what a browser said, not what this
 * service checked.
 */
export function sourcePageUrl(...candidates: readonly (string | null | undefined)[]): string | null {
  for (const candidate of candidates) {
    if (!candidate) continue
    try {
      const url = new URL(candidate)
      if (url.protocol === 'http:' || url.protocol === 'https:') return url.href
    } catch {
      // Not an address at all; the next candidate may be one.
    }
  }
  return null
}
