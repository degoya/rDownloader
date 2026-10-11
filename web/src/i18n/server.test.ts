import { afterEach, beforeAll, describe, expect, it } from 'vitest'

import { i18n } from '@/i18n'
import { translateServerMessage } from '@/i18n/server'
import { loadEveryLocale } from '@/test/locales'

beforeAll(loadEveryLocale)

describe('a coded message whose parameter did not arrive (RD-1240-37)', () => {
  afterEach(() => {
    i18n.global.locale.value = 'en'
  })

  it('shows the server text, which carries the cause, rather than a sentence with a hole', () => {
    i18n.global.locale.value = 'de'
    // What a LinkGrabber row holds: code and English text, the parameters only it brings.
    const row = {
      code: 'media.ytdlp_failed',
      message: 'yt-dlp failed: ERROR: [youtube] gjTzGhcQ21E: Video unavailable',
      params: { host: 'youtube.com' }
    }
    expect(translateServerMessage(row)).toBe(row.message)
  })

  it('still translates the line when the parameter is there', () => {
    i18n.global.locale.value = 'de'
    expect(translateServerMessage({
      code: 'media.ytdlp_failed',
      message: 'yt-dlp failed: exit code 1, no error output',
      params: { detail: 'exit code 1, no error output' }
    })).toBe('yt-dlp fehlgeschlagen: exit code 1, no error output')
  })

  it('leaves a line without parameters alone', () => {
    i18n.global.locale.value = 'de'
    expect(translateServerMessage({ code: 'media.playlist_empty', message: 'Playlist contains no downloadable entries' }))
      .toBe('Die Playlist enthält keine ladbaren Einträge')
  })
})
