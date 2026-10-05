import { afterEach, describe, expect, it, vi } from 'vitest'

import { REQUIRED_LOCALES, SUPPORTED_LOCALES, currentLocale, detectLocale, i18n, languageItems, setLocale } from '@/i18n'

/**
 * A fifth language added the way the guide in `CONTRIBUTING.md` says: one entry in the list,
 * marked `in-progress`, and no code anywhere else (RD-1100-09). `it` has no catalogue in this
 * tree, so every key it is asked for is one it lacks. `vi.mock` is hoisted above the import.
 */
vi.mock('@/locales/languages.json', () => ({
  default: {
    en: { name: 'English', status: 'required' },
    de: { name: 'Deutsch', status: 'required' },
    fr: { name: 'Français', status: 'required' },
    es: { name: 'Español', status: 'required' },
    it: { name: 'Italiano', status: 'in-progress' }
  }
}))

describe('an in-progress language from the list', () => {
  afterEach(async () => {
    await setLocale('en')
    localStorage.clear()
  })

  it('is offered by the picker, marked as unfinished, with no code naming it', () => {
    expect(SUPPORTED_LOCALES).toEqual(['en', 'de', 'fr', 'es', 'it'])
    expect(REQUIRED_LOCALES).toEqual(['en', 'de', 'fr', 'es'])
    expect(languageItems(i18n.global.t)).toEqual([
      { label: 'English', value: 'en' },
      { label: 'Deutsch', value: 'de' },
      { label: 'Français', value: 'fr' },
      { label: 'Español', value: 'es' },
      { label: 'Italiano (in progress)', value: 'it' }
    ])
  })

  it('switches to it and shows English for every key it lacks, never the raw key', async () => {
    await setLocale('it')

    expect(currentLocale()).toBe('it')
    expect(document.documentElement.lang).toBe('it')
    expect(i18n.global.t('common.actions.cancel')).toBe('Cancel')
    expect(i18n.global.t('common.preferences.language')).not.toBe('common.preferences.language')
  })

  it('keeps a stored choice of it, but never picks it from the browser alone', () => {
    const languages = vi.spyOn(navigator, 'languages', 'get').mockReturnValue(['it-IT', 'de-DE'])
    try {
      expect(detectLocale()).toBe('de')
      localStorage.setItem('rd.locale', 'it')
      expect(detectLocale()).toBe('it')
    } finally {
      languages.mockRestore()
    }
  })

  it('refuses a code the list does not name', async () => {
    await setLocale('xx')
    expect(currentLocale()).toBe('en')
  })
})
