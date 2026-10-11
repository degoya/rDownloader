/**
 * The download window editor of the package dialog and the category editor (RD-1240-30): the
 * switch that sets a window, the spans it starts with and adds, and the bypass of the schedule's
 * pause, each handed back as a new draft.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import bandwidth from '@/locales/en/bandwidth.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'
import type { DownloadWindowDraft } from '@/utils/downloadWindow'

import DownloadWindowEditor from './DownloadWindowEditor.vue'

function mount(modelValue: DownloadWindowDraft) {
  return mountComponent(DownloadWindowEditor, {
    props: { modelValue, label: 'Own download window', description: 'Off: follows the schedule' },
    messages: { downloads, bandwidth }
  })
}

function last(view: ReturnType<typeof mount>): DownloadWindowDraft {
  const updates = view.emitted('update:modelValue') ?? []
  return (updates.at(-1) as [DownloadWindowDraft])[0]
}

describe('the download window editor', () => {
  it('starts a window switched on with the night from 22:00 to 06:00', async () => {
    const view = mount({ enabled: false, windows: [], ignore_schedule_pause: false })
    expect(screen.queryByTestId('download-window-span')).toBeNull()

    await fireEvent.click(screen.getByRole('switch', { name: 'Own download window' }))

    expect(last(view)).toEqual({
      enabled: true,
      windows: [{ days: 127, start_minute: 22 * 60, end_minute: 6 * 60 }],
      ignore_schedule_pause: false
    })
  })

  it('adds a span, says a span past midnight wraps, and sets the bypass', async () => {
    const view = mount({
      enabled: true,
      windows: [{ days: 127, start_minute: 22 * 60, end_minute: 6 * 60 }],
      ignore_schedule_pause: false
    })
    expect(screen.getByText(bandwidth.schedule.wraps)).toBeTruthy()
    expect(screen.getByText(downloads.window.rates_hint)).toBeTruthy()

    await fireEvent.click(screen.getByTestId('download-window-add'))
    expect(last(view).windows).toHaveLength(2)

    await fireEvent.click(screen.getByRole('switch', { name: downloads.window.ignore_label }))
    expect(last(view).ignore_schedule_pause).toBe(true)
  })

  it('says that a window without spans downloads at any time', () => {
    mount({ enabled: true, windows: [], ignore_schedule_pause: true })
    expect(screen.getByText(downloads.window.anytime)).toBeTruthy()
  })
})
