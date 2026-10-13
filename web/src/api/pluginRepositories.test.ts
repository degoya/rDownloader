import { describe, expect, it } from 'vitest'

import { releaseNotesByPlugin, type PluginOffer, type PluginOffers } from './pluginRepositories'

function offer(pluginId: string, version: string, notes: string | null, repository = 'rDownloader'): PluginOffer {
  return {
    repository_id: repository,
    repository_name: repository,
    official: repository === 'rDownloader',
    compatibility: 'compatible',
    installed_version: null,
    package: { plugin_id: pluginId, version, release_notes: notes } as PluginOffer['package']
  }
}

describe('releaseNotesByPlugin', () => {
  it('collects the notes of every offered version, newest first, one per version', () => {
    const offers: PluginOffers = {
      updates: [{ offer: offer('a', '1.10.0', 'Ten'), installed_version: '1.2.0', policy: 'manual', adds_permissions: false, added_permissions: { granted: [], http_domains: [], stream_hosts: [] } }],
      // The versions of an installed plugin come in `installed`, the others in `available`.
      installed: [offer('a', '1.9.0', 'Nine')],
      available: [
        offer('a', '1.10.0', 'Ten again', 'Community'),
        offer('a', '1.8.0', '   '),
        offer('b', '0.1.0', null)
      ]
    }
    const notes = releaseNotesByPlugin(offers)
    expect(notes.get('a')).toEqual([
      { version: '1.10.0', notes: 'Ten', repository: 'rDownloader' },
      { version: '1.9.0', notes: 'Nine', repository: 'rDownloader' }
    ])
    expect(notes.has('b')).toBe(false)
  })

  it('drops maintenance-only notes and keeps repeated notes with the newest version (RD-1280-17)', () => {
    const maintenance = 'Maintenance release: internal changes only, no change in behaviour.'
    const offers: PluginOffers = {
      updates: [],
      installed: [offer('a', '0.1.15', 'In 0.1.13: Captcha pages are solved again.')],
      available: [
        offer('a', '0.1.14', 'In 0.1.13: Captcha pages are solved again.'),
        offer('a', '0.1.13', 'Captcha pages are solved again.'),
        offer('a', '0.1.12', ` ${maintenance}\n`),
        offer('b', '0.2.0', maintenance)
      ]
    }
    const notes = releaseNotesByPlugin(offers)
    expect(notes.get('a')).toEqual([
      { version: '0.1.15', notes: 'In 0.1.13: Captcha pages are solved again.', repository: 'rDownloader' },
      { version: '0.1.13', notes: 'Captcha pages are solved again.', repository: 'rDownloader' }
    ])
    expect(notes.has('b')).toBe(false)
  })
})
