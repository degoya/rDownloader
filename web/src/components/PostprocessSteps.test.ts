/** A step's time is written in the interface's language through `formatMoment`, not the browser's (RD-1120-14). */
import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { PostprocessStep } from '@/api/types'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'
import { formatMoment } from '@/utils/format'

import PostprocessSteps from './PostprocessSteps.vue'

describe('PostprocessSteps', () => {
  it('writes when a step last changed the way every other timestamp is written', () => {
    const updatedAt = '2026-10-06T08:15:00Z'
    const step = {
      owner_id: 'p1', kind: 'unpack', state: 'completed', source_path: '/data/a.rar', updated_at: updatedAt
    } as unknown as PostprocessStep
    mountComponent(PostprocessSteps, { messages: { downloads }, props: { steps: [step] } })
    expect(formatMoment(updatedAt)).not.toBe('')
    expect(screen.getByText(formatMoment(updatedAt))).toBeTruthy()
  })
})
