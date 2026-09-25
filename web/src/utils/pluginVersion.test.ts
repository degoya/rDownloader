import { describe, expect, it } from 'vitest'

import { withPluginVersion } from './pluginVersion'

describe('plugin version labels', () => {
  it('puts the version behind the name', () => {
    expect(withPluginVersion('DDownload', '0.10.1')).toBe('DDownload v0.10.1')
  })

  // Not every provider comes from a plugin in every response, and a label reading
  // "DDownload vundefined" would be worse than one without a version.
  it('leaves the name alone when there is no version', () => {
    expect(withPluginVersion('DDownload', null)).toBe('DDownload')
    expect(withPluginVersion('DDownload', undefined)).toBe('DDownload')
    expect(withPluginVersion('DDownload', '')).toBe('DDownload')
  })
})
