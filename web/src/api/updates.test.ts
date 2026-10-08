/** The points of an update's notes (RD-1150-02), shared by the dialog and the Updates page. */
import { describe, expect, it } from 'vitest'

import { offerLinks, releaseNotePoints } from './updates'

describe('releaseNotePoints', () => {
  it('takes one point per line, without its marker and without empty lines', () => {
    expect(releaseNotePoints('- One.\n\n-  Two.\n* Three.\n')).toEqual(['One.', 'Two.', 'Three.'])
  })

  it('keeps a single sentence as one point and has none for empty notes', () => {
    expect(releaseNotePoints('Maintenance release: internal changes only, no change in behaviour.'))
      .toEqual(['Maintenance release: internal changes only, no change in behaviour.'])
    expect(releaseNotePoints('')).toEqual([])
    expect(releaseNotePoints(' \n ')).toEqual([])
  })

  it('leaves a hyphen inside a line alone', () => {
    expect(releaseNotePoints('- Up-to-date mirrors - all of them.')).toEqual(['Up-to-date mirrors - all of them.'])
  })
})

describe('offerLinks', () => {
  const offer = {
    changelog_url: 'https://example.com/CHANGELOG.md#190',
    release_url: 'https://example.com/releases/v1.9.0',
    download_url: 'https://example.com/releases/v1.9.0/rdownloader.tar.gz'
  }

  it('links the offer\'s http(s) addresses, the download before the release page', () => {
    expect(offerLinks(offer)).toEqual({
      changelog: offer.changelog_url,
      release: offer.release_url,
      download: offer.download_url,
      get: offer.download_url
    })
    expect(offerLinks({ ...offer, download_url: null }).get).toBe(offer.release_url)
  })

  it('links no address that is not http(s), like every other link of the interface (WEB-1)', () => {
    const links = offerLinks({ changelog_url: 'javascript:alert(1)', release_url: 'data:text/html,x', download_url: 'file:///etc/passwd' })
    expect(links).toEqual({ changelog: undefined, release: undefined, download: undefined, get: undefined })
    expect(offerLinks({ ...offer, download_url: 'javascript:alert(1)' }).get).toBe(offer.release_url)
  })
})
