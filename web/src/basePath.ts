/**
 * Where this application is mounted, when a reverse proxy puts it under a path.
 *
 * The value cannot be a build-time constant: the bundle is embedded in the binary and the
 * same binary is deployed at the root by most people and under `/downloads` by some. The
 * server injects it into `index.html` as it is served, and everything that builds a URL —
 * the router, the API client, the event stream — reads it from here.
 *
 * Empty at the root, which is the overwhelmingly common case and costs nothing.
 */
declare global {
  interface Window {
    __RD_BASE__?: string
  }
}

/** Normalised mount point: a leading slash and no trailing one, or empty for the root. */
export const BASE_PATH: string = normalise(
  typeof window === 'undefined' ? '' : (window.__RD_BASE__ ?? '')
)

function normalise(value: string): string {
  const trimmed = value.trim().replace(/^\/+|\/+$/g, '')
  return trimmed ? `/${trimmed}` : ''
}

/** Prefixes an absolute application path with the mount point. */
export function withBase(path: string): string {
  return `${BASE_PATH}${path}`
}
