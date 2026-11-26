import { expect, it } from 'vitest'

import { DOWNLOADS_NAV_NOTICES, DOWNLOADS_NOTICE_LINE } from './downloadsNavbar'

it('shows the notices in the navbar or on their own line, never in both and never in neither', () => {
  const width = /@min-\[(\d+)rem\]:(\w+)/
  expect(width.exec(DOWNLOADS_NAV_NOTICES)?.slice(1)).toEqual(['80', 'contents'])
  expect(width.exec(DOWNLOADS_NOTICE_LINE)?.slice(1)).toEqual(['80', 'hidden'])
})
