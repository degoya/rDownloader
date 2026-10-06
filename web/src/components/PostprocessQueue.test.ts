/** The post-processing queue above the downloads is a card: its title in the header, a row per package (RD-1120-14). */
import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { PostprocessQueueEntry } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'

import PostprocessQueue from './PostprocessQueue.vue'

const entries = [
  { package_id: 'p1', name: 'Season 1', pending: false, percent: 40, stage: 'unpack', state: 'postprocessing', current: '/data/s1/e01.rar' },
  { package_id: 'p2', name: 'Season 2', pending: true, state: 'postprocessing' }
] as unknown as PostprocessQueueEntry[]

describe('PostprocessQueue', () => {
  it('is a card section with its title and count in the header and a row per package', () => {
    mountComponent(PostprocessQueue, { messages: { downloads }, props: { entries } })
    const title = screen.getByText(downloads.postprocess.queue.title)
    const section = title.closest('section') as HTMLElement
    expect(section).toBeTruthy()
    expect(section.textContent).toContain('2')
    expect(screen.getAllByRole('listitem').map(item => item.textContent)).toEqual([
      expect.stringContaining('Season 1'),
      expect.stringContaining('Season 2')
    ])
    expect(screen.getByText('e01.rar')).toBeTruthy()
    expect(screen.getByText(downloads.postprocess.queue.pending)).toBeTruthy()
  })
})
