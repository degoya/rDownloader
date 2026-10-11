/**
 * RD-1240-15: a section is committed only as a readable range, and pauses are the link's own
 * only while switched on.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { MediaPauses, MediaSection } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

import MediaSectionPausesField from './MediaSectionPausesField.vue'

function mount(props: { section?: MediaSection | null, pauses?: MediaPauses | null, canCut?: boolean } = {}) {
  return mountComponent(MediaSectionPausesField, {
    messages: { linkgrabber: en },
    props: { section: null, pauses: null, canCut: true, ...props }
  })
}

async function type(testId: string, value: string): Promise<void> {
  await fireEvent.update(screen.getByTestId(testId), value)
  await fireEvent.blur(screen.getByTestId(testId))
}

describe('MediaSectionPausesField', () => {
  it('shows a stored section as times', () => {
    mount({ section: { start_seconds: 90, end_seconds: 3723 } })
    expect((screen.getByTestId('media-section-start') as HTMLInputElement).value).toBe('1:30')
    expect((screen.getByTestId('media-section-end') as HTMLInputElement).value).toBe('1:02:03')
  })

  it('commits an open end and a start of zero as the beginning', async () => {
    const { emitted } = mount()
    await type('media-section-start', '45')
    expect(emitted().section?.at(-1)).toEqual([{ start_seconds: 45, end_seconds: null }])
    await type('media-section-start', '0')
    await type('media-section-end', '20')
    expect(emitted().section?.at(-1)).toEqual([{ start_seconds: null, end_seconds: 20 }])
  })

  it('refuses an unreadable time and an end before the start, and commits neither', async () => {
    const { emitted } = mount()
    await type('media-section-start', '1:75')
    expect(screen.getByTestId('media-section-error').textContent).toBe(en.media.section.invalid)
    await fireEvent.update(screen.getByTestId('media-section-start'), '2:00')
    await type('media-section-end', '1:00')
    expect(screen.getByTestId('media-section-error').textContent).toBe(en.media.section.order)
    expect(emitted().section).toBeUndefined()
  })

  it('answers null once both fields are emptied', async () => {
    const { emitted } = mount({ section: { start_seconds: 10, end_seconds: 20 } })
    await fireEvent.update(screen.getByTestId('media-section-start'), '')
    await type('media-section-end', '')
    expect(emitted().section?.at(-1)).toEqual([null])
  })

  it('sets the word before each time beside its field, not over the placeholder', () => {
    // In the input's `#leading` slot "From" overlapped "Beginning" (RD-1240-28, live test).
    mount()
    for (const [which, label] of [['start', en.media.section.start], ['end', en.media.section.end]] as const) {
      const input = screen.getByTestId(`media-section-${which}`)
      const prefix = screen.getByTestId(`media-section-${which}-prefix`)
      expect(prefix.textContent).toBe(label)
      // A badge of its own in the field's group, ahead of the input rather than inside it.
      expect(prefix.parentElement).toBe(input.parentElement)
      expect(prefix.nextElementSibling).toBe(input)
    }
  })

  it('disables the section without ffmpeg to cut with', () => {
    mount({ canCut: false })
    expect(screen.getByTestId('media-section-start').hasAttribute('disabled')).toBe(true)
    expect(screen.getByTestId('media-section-end').hasAttribute('disabled')).toBe(true)
  })

  it('switches own pauses on and off and passes a changed pause', async () => {
    const { emitted, rerender } = mount()
    await fireEvent.click(screen.getByRole('switch', { name: en.media.pauses.own }))
    expect(emitted().pauses?.at(-1)).toEqual([{ sleep_requests_seconds: 0, sleep_interval_seconds: 0 }])

    await rerender({ pauses: { sleep_requests_seconds: 0, sleep_interval_seconds: 0 } })
    await fireEvent.update(screen.getByLabelText(en.media.pauses.requests), '3')
    expect(emitted().pauses?.at(-1)).toEqual([{ sleep_requests_seconds: 3, sleep_interval_seconds: 0 }])

    await fireEvent.click(screen.getByRole('switch', { name: en.media.pauses.own }))
    expect(emitted().pauses?.at(-1)).toEqual([null])
  })
})
