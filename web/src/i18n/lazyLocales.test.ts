import { afterEach, describe, expect, it } from 'vitest'

import { currentLocale, i18n, setLocale } from '@/i18n'

/**
 * Only English ships in the main chunk; the other catalogues arrive when first chosen
 * (RD-140-27). This file deliberately does not preload them, unlike the catalogue tests.
 */
describe('lazy locale catalogues', () => {
  afterEach(() => setLocale('en'))

  it('starts with English only and fetches a language when it is chosen', async () => {
    expect(currentLocale()).toBe('en')
    expect(Object.keys(i18n.global.getLocaleMessage('de'))).toHaveLength(0)
    expect(i18n.global.te('common.actions.cancel', 'en')).toBe(true)

    await setLocale('de')

    expect(currentLocale()).toBe('de')
    expect(document.documentElement.lang).toBe('de')
    expect(i18n.global.te('common.actions.cancel', 'de')).toBe(true)
    expect(i18n.global.t('common.actions.cancel')).not.toBe(i18n.global.t('common.actions.cancel', {}, { locale: 'en' }))
  })

  it('lets a later choice win over a chunk that arrives after it', async () => {
    const slow = setLocale('fr')
    await setLocale('en')
    await slow
    expect(currentLocale()).toBe('en')
    expect(i18n.global.te('common.actions.cancel', 'fr')).toBe(true)
  })
})
