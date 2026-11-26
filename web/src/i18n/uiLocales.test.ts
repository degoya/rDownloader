import { describe, expect, it } from 'vitest'

import languages from '@/locales/languages.json'

import { uiLocaleOf } from './uiLocales'

describe("Nuxt UI's strings", () => {
  it.each(Object.keys(languages))('come in %s', (language) => {
    expect(uiLocaleOf(language).code).toBe(language)
  })

  it('stay English for a language they do not come in', () => {
    expect(uiLocaleOf('xx').code).toBe('en')
  })
})
