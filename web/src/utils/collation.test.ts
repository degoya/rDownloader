import { afterEach, describe, expect, it } from 'vitest'

import { i18n } from '@/i18n'

import { compareNatural, compareText } from './collation'

describe('sorting in the interface language', () => {
  afterEach(() => { i18n.global.locale.value = 'en' })

  it('orders by the language that is set, not the browser one, and follows a switch', () => {
    const names = ['oz', 'ñu', 'nz']
    expect([...names].sort(compareText)).toEqual(['ñu', 'nz', 'oz'])
    i18n.global.locale.value = 'es'
    // ñ is a letter of its own after n in Spanish.
    expect([...names].sort(compareText)).toEqual(['nz', 'ñu', 'oz'])
    expect([...names].sort(compareNatural)).toEqual(['nz', 'ñu', 'oz'])
  })

  it('reads numbers by their value where names are numbered', () => {
    expect(['1080p', '10', '720p', '2'].sort(compareNatural)).toEqual(['2', '10', '720p', '1080p'])
    expect(compareNatural('Ä', 'a')).toBe(0)
    expect(compareText('Ä', 'a')).not.toBe(0)
  })
})
