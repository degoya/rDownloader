// What a match pattern means in each browser, for the fake browser's filters and grants.
//
// The one difference that cost three features (RD-1480-01): Chrome reads a port in the host part,
// Firefox does not — it compares the whole part with the address's host name, so a pattern with a
// port matches nothing there (Mozilla bugs 1362809 and 1468162).

function parse(pattern) {
  if (pattern === '<all_urls>') return { scheme: '*', host: '*', port: null, path: '/*' }
  const match = /^(\*|https?):\/\/([^/]+)(\/.*)$/.exec(pattern)
  if (!match) throw new Error(`Invalid match pattern: ${pattern}`)
  const [, scheme, authority, path] = match
  const [, host, port] = /^(\[[^\]]*\]|[^:]*)(?::(\d+|\*))?$/.exec(authority) ?? []
  if (host === undefined) throw new Error(`Invalid match pattern: ${pattern}`)
  return { scheme, host, port: port === undefined || port === '*' ? null : port, path }
}

function hostCovers(held, asked) {
  if (held === '*' || held === asked) return true
  if (!held.startsWith('*.')) return false
  const domain = held.slice(2)
  const below = asked.startsWith('*.') ? asked.slice(2) : asked
  return below === domain || below.endsWith(`.${domain}`)
}

function globMatches(glob, text) {
  const escaped = glob.split('*').map((part) => part.replace(/[.+?^${}()|[\]\\]/g, '\\$&')).join('.*')
  return new RegExp(`^${escaped}$`).test(text)
}

/** Whether a request to `url` is covered by `pattern` in this browser. */
export function matchesPattern(target, pattern, url) {
  const { scheme, host, port, path } = parse(pattern)
  const address = new URL(url)
  if (!/^https?:$/.test(address.protocol)) return false
  if (scheme !== '*' && `${scheme}:` !== address.protocol) return false
  if (port !== null) {
    if (target === 'firefox') return false
    if (port !== (address.port || (address.protocol === 'https:' ? '443' : '80'))) return false
  }
  return hostCovers(host, address.hostname) && globMatches(path, address.pathname + address.search)
}

/** Whether holding `held` includes `asked`, as `permissions.contains` answers it. Paths are ignored. */
export function subsumes(target, held, asked) {
  const a = parse(held)
  const b = parse(asked)
  if (a.scheme !== '*' && a.scheme !== b.scheme) return false
  // Firefox has no port: the two parts are compared as they were written.
  if (target === 'firefox' && a.host !== '*' && a.port !== b.port) return false
  if (target === 'chrome' && a.port !== null && a.port !== b.port) return false
  return hostCovers(a.host, b.host)
}
