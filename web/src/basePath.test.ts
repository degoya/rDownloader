import { afterEach, describe, expect, it, vi } from 'vitest'

/** The module reads the mount point once, as it loads; each case loads it afresh. */
async function loadWith(base?: string) {
  if (base === undefined) delete window.__RD_BASE__
  else window.__RD_BASE__ = base
  vi.resetModules()
  return import('./basePath')
}

afterEach(() => {
  delete window.__RD_BASE__
  vi.resetModules()
})

describe('service addresses', () => {
  it('is the origin alone at the root', async () => {
    const { serviceUrl } = await loadWith()
    expect(serviceUrl()).toBe(window.location.origin)
    expect(serviceUrl('/api/v1/metrics')).toBe(`${window.location.origin}/api/v1/metrics`)
  })

  // Audit K8: behind a reverse proxy under a path, the metrics address lost the mount point.
  it('keeps the mount point a reverse proxy puts the service under', async () => {
    const { serviceUrl, withBase } = await loadWith('/downloads/')
    expect(withBase('/mcp')).toBe('/downloads/mcp')
    expect(serviceUrl()).toBe(`${window.location.origin}/downloads`)
    expect(serviceUrl('/api/v1/metrics')).toBe(`${window.location.origin}/downloads/api/v1/metrics`)
  })
})
