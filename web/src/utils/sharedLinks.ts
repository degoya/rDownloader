/**
 * Reading a shared link out of the query string (RD-090-08).
 *
 * The Web Share Target is declared as a GET target, so a share from a mobile browser lands
 * on `/linkgrabber?shared=…&shared_url=…&title=…`. Parsing it here rather than in the view
 * keeps the rule testable: what counts as a link, and what is dropped.
 */

/** Query parameters the manifest's share target fills in. */
const SHARE_PARAMS = ['shared', 'shared_url'] as const

/**
 * Extracts the shared text, or null when nothing usable was shared.
 *
 * Android fills `text` with either the shared text or, for some apps, the URL; iOS tends to
 * use `url`. Both are read and joined, because an app that puts the link in `text` and a
 * title in `title` is at least as common as one that fills `url`.
 */
export function sharedText(query: Record<string, unknown>): string | null {
  const parts = SHARE_PARAMS.map(key => query[key])
    .flatMap(value => (Array.isArray(value) ? value : [value]))
    .filter((value): value is string => typeof value === 'string')
    .map(value => value.trim())
    .filter(value => value.length > 0)
  if (!parts.length) return null
  // Deduplicated: an app that fills both `text` and `url` with the same link would
  // otherwise hand the LinkGrabber the same address twice.
  const unique = [...new Set(parts)]
  const joined = unique.join('\n')
  // A share with no link at all — a plain sentence, a title only — is not intake. Handing it
  // over would produce an empty batch and an error the user cannot act on.
  return /\b[a-z][a-z0-9+.-]*:\/\//i.test(joined) || /magnet:\?/i.test(joined) ? joined : null
}
