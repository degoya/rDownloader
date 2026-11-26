// What a host grant is asked for, in the browser it is asked of.
//
// Chrome reads a port in a match pattern, so a grant there is for the one service. Firefox does
// not: a pattern with a port matches nothing (Mozilla bugs 1362809, 1468162), so its build asks
// for the host (RD-1480-01). What decides is the manifest the browser loaded, not a guess about
// the browser, as in `data-collection.js`.

/** The manifest the browser loaded, or null where none can be read. */
export function manifestOf(api) {
  try {
    return api?.runtime?.getManifest?.() ?? null
  } catch {
    return null
  }
}

/** `<scheme>://<host>/*` for exactly this address's origin, without the port in the Firefox build. */
export function grantPattern(url, manifest) {
  return `${url.protocol}//${manifest?.browser_specific_settings?.gecko ? url.hostname : url.host}/*`
}
