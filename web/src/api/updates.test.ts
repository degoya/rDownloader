/** The points of an update's notes (RD-1150-02), shared by the dialog and the Updates page. */
import { describe, expect, it } from 'vitest'

import { releaseNotePoints } from './updates'

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
