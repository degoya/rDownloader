import { safeHttpUrl } from '@/utils/safeUrl'

/**
 * The page a link came from, when the data knows one and it is a web page: a captured browser
 * download's referrer, or the page a media link was extracted from. Anything that is not
 * `http(s)` is withheld rather than opened — a referrer is what a browser said, not what this
 * service checked.
 */
export function sourcePageUrl(...candidates: readonly (string | null | undefined)[]): string | null {
  for (const candidate of candidates) {
    const url = safeHttpUrl(candidate)
    if (url) return new URL(url).href
  }
  return null
}
