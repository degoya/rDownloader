/**
 * A bandwidth profile that pauses downloads, named beside the queue's control (RD-1240-30): until
 * when, which profile, and in the header the way to switch to another one.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import downloads from '@/locales/en/downloads.json'
import { useQueuePauseStore } from '@/stores/queuePause'
import { mountComponent } from '@/test/mount'

import SchedulePauseNotice from './SchedulePauseNotice.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })), PUT: vi.fn(), DELETE: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'refused')
}))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: vi.fn(() => () => {}) }))

function mount(placement: 'header' | 'rail') {
  mountComponent(SchedulePauseNotice, { props: { placement }, messages: { downloads } })
  return useQueuePauseStore()
}

describe('SchedulePauseNotice', () => {
  it('says nothing while no profile pauses downloads', () => {
    mount('header')
    expect(screen.queryByTestId('schedule-pause-notice')).toBeNull()
  })

  it('names the profile and offers the switch in the header', async () => {
    const queuePause = mount('header')
    queuePause.schedulePause = { profile_id: 'day', profile_name: 'Day', until: null, manual: false }
    await nextTick()
    expect(screen.getByLabelText(downloads.schedule_pause.label_open)).toBeTruthy()
    expect(screen.getByTestId('schedule-pause-switch').textContent).toContain(downloads.schedule_pause.open_settings)
  })

  it('keeps to the badge on the rail', async () => {
    const queuePause = mount('rail')
    queuePause.schedulePause = { profile_id: 'day', profile_name: 'Day', until: '2026-10-10T20:00:00Z', manual: false }
    await nextTick()
    expect(screen.getByTestId('schedule-pause-notice')).toBeTruthy()
    expect(screen.queryByTestId('schedule-pause-switch')).toBeNull()
  })
})
