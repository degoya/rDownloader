import { describe, expect, it } from 'vitest'

import type { InstalledPlugin, PluginLifecycle } from '@/api/types'

import { domainAllowed, stagedResolvers } from './useStagedResolvers'

function plugin(version: string, type = 'resolver'): InstalledPlugin {
  return {
    id: 'p1', name: 'Hoster', version, plugin_type: type, domains: ['*.hoster.test', 'hoster.test']
  } as unknown as InstalledPlugin
}

function lifecycle(staged: string | null, restartRequired = false): PluginLifecycle {
  return {
    plugin_id: 'p1', active_version: '1.0.0', running_version: '1.0.0', staged_version: staged,
    previous_version: null, update_policy: 'manual', restart_required: restartRequired
  } as PluginLifecycle
}

describe('stagedResolvers', () => {
  it('names a loaded staged resolver version', () => {
    expect(stagedResolvers([plugin('1.0.0'), plugin('2.0.0')], [lifecycle('2.0.0')])).toEqual([
      { pluginId: 'p1', name: 'Hoster', version: '2.0.0', domains: ['*.hoster.test', 'hoster.test'] }
    ])
  })

  /** The trial route refuses a version that is only stored; offering it would promise nothing. */
  it('leaves out a staged version that waits for a restart, and every non-resolver', () => {
    expect(stagedResolvers([plugin('2.0.0')], [lifecycle('2.0.0', true)])).toEqual([])
    expect(stagedResolvers([plugin('2.0.0', 'transfer')], [lifecycle('2.0.0')])).toEqual([])
    expect(stagedResolvers([plugin('2.0.0')], [lifecycle(null)])).toEqual([])
  })
})

describe('domainAllowed', () => {
  it('follows the resolver host rule', () => {
    expect(domainAllowed('https://hoster.test/file', ['hoster.test'])).toBe(true)
    expect(domainAllowed('https://cdn.hoster.test/file', ['*.hoster.test'])).toBe(true)
    expect(domainAllowed('https://hoster.test/file', ['*.hoster.test'])).toBe(false)
    expect(domainAllowed('https://evilhoster.test/file', ['*.hoster.test'])).toBe(false)
    expect(domainAllowed('magnet:?xt=urn:btih:abc', ['*'])).toBe(false)
    expect(domainAllowed('not a url', ['*'])).toBe(false)
  })
})
