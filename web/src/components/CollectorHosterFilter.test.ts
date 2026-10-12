/** The hoster switches above the LinkGrabber: the figure on each says in its tooltip what it counts (RD-1260-07). */
import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import CollectorHosterFilter from './CollectorHosterFilter.vue'

describe('CollectorHosterFilter', () => {
  it('names the links a hoster holds in the tooltip of its switch', () => {
    mountComponent(CollectorHosterFilter, {
      props: {
        hosters: [{ hoster: 'files.example.com', count: 12, hidden: false }, { hoster: 'other.example', count: 1, hidden: true }],
        hiddenLinks: 1,
        hiddenHosters: 1,
        busy: false
      },
      messages: { linkgrabber }
    })
    expect(screen.getByTitle('Hide links of files.example.com · 12 links')).toBeTruthy()
    expect(screen.getByTitle('Show links of other.example again · 1 link')).toBeTruthy()
  })
})
