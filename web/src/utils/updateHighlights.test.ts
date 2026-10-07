/** The points of an offered version's notes the update page's notice shows (RD-1150-01). */
import { describe, expect, it } from 'vitest'

import { updateHighlights } from './updateHighlights'

describe('updateHighlights', () => {
  it('takes the first three entries and skips the headings', () => {
    const notes = 'Added\n- Parallel hoster downloads\n- A regex diagram\nFixed\n- Stale update notice\n- Fourth point'
    expect(updateHighlights({ notes })).toEqual(['Parallel hoster downloads', 'A regex diagram', 'Stale update notice'])
  })

  it('drops a bold marker a headline lost at a line break', () => {
    expect(updateHighlights({ notes: '- **A file waiting for its turn' })).toEqual(['A file waiting for its turn'])
  })

  it('reads the user notes of 1.15 on: one point per line, or one sentence', () => {
    expect(updateHighlights({ notes: '- Faster hoster downloads\n\n- A clearer update page' }))
      .toEqual(['Faster hoster downloads', 'A clearer update page'])
    expect(updateHighlights({ notes: 'Maintenance release: nothing changes for you.' }))
      .toEqual(['Maintenance release: nothing changes for you.'])
  })

  it('is empty for empty notes', () => {
    expect(updateHighlights({ notes: '' })).toEqual([])
    expect(updateHighlights({ notes: '\n  \n' })).toEqual([])
  })
})
