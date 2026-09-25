import { describe, expect, it } from 'vitest'

import {
  CARD_RATIOS,
  TILE_COLOURS,
  cardAspect,
  cardRatio,
  hitColour,
  hitGroup,
  hitInitials,
  hitIsMusic,
  hitName,
  promotedFacts
} from './subscriptionHit'

describe('subscriptionHit', () => {
  it('takes up to three initials and skips a leading article', () => {
    expect(hitInitials('The Big Bang Theory')).toBe('BBT')
    expect(hitInitials('Die Pfefferkoerner')).toBe('PF')
    expect(hitInitials('Anwalte der Toten')).toBe('ADT')
    expect(hitInitials('Pfefferkoerner')).toBe('PF')
    expect(hitInitials('The')).toBe('TH')
    expect(hitInitials('— … —')).toBeNull()
  })

  it('gives the same name the same colour, every time and whatever the case', () => {
    const first = hitColour('The Big Bang Theory')
    expect(hitColour('The Big Bang Theory')).toBe(first)
    expect(hitColour('  the big bang theory ')).toBe(first)
    expect(TILE_COLOURS).toContain(first)
    // Not one colour for everything: a handful of different series spread over the palette.
    const spread = new Set(['Die Pfefferkoerner', 'The Big Bang Theory', 'Anwalte der Toten', 'Jokair', 'Tatort', 'Dark']
      .map(hitColour))
    expect(spread.size).toBeGreaterThan(2)
  })

  it('names a hit by what the indexer stated, and otherwise by the head of the release name', () => {
    expect(hitName('The.Big.Bang.Theory.S11E23.German.1080p-FUZEER', { tvtitle: 'The Big Bang Theory' }))
      .toBe('The Big Bang Theory')
    expect(hitName('Die.Pfefferkoerner.S13.GERMAN.1080p.WEB.H264-RWP', {})).toBe('Die Pfefferkoerner')
    expect(hitName('Some.Movie.2024.1080p', {})).toBe('Some Movie')
    expect(hitName('x', { artist: 'Jokair', album: 'Samedi Soir' })).toBe('Jokair – Samedi Soir')
    expect(hitName('Plain title with spaces', {})).toBe('Plain title with spaces')
  })

  it('reads the release group from the team attribute or the scene suffix, and invents none', () => {
    expect(hitGroup('Die.Pfefferkoerner.S13.GERMAN.1080p.WEB.H264-RWP', {})).toBe('RWP')
    expect(hitGroup('anything', { team: 'OND' })).toBe('OND')
    expect(hitGroup('A title - with a dash', {})).toBeNull()
    expect(hitGroup('No.Group.Here.1080p', {})).toBeNull()
  })

  it('knows a music hit by its Newznab category', () => {
    expect(hitIsMusic('3010', {})).toBe(true)
    expect(hitIsMusic(null, { category: '3000' })).toBe(true)
    expect(hitIsMusic('5040', {})).toBe(false)
    expect(hitIsMusic(undefined, {})).toBe(false)
  })

  it('promotes only the facts the indexer sent, in a fixed order', () => {
    const t = (key: string) => key.split('.').pop() as string
    expect(promotedFacts({ language: 'German', imdbscore: '8.1' }, t).map(fact => fact.key))
      .toEqual(['imdbscore', 'language'])
    expect(promotedFacts({}, t)).toEqual([])
  })

  it('knows the six card ratios, reads anything else as 2:1 and spells each for CSS (RD-120-42)', () => {
    expect(CARD_RATIOS).toEqual(['1:1', '2:3', '3:2', '16:9', '4:3', '2:1'])
    for (const ratio of CARD_RATIOS) expect(cardRatio(ratio)).toBe(ratio)
    for (const unknown of ['21:9', '', null, undefined]) expect(cardRatio(unknown)).toBe('2:1')
    expect(cardAspect('16:9')).toBe('16 / 9')
    expect(cardAspect('1:1')).toBe('1 / 1')
  })
})
