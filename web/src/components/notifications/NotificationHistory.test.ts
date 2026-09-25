/**
 * The history's own clear button (RD-130-08). It sits at the list it empties rather than under
 * System, and its question names the number a clear would take -- which is not the length of
 * the list: queued and retrying deliveries stay, and the list shows only the newest fifty.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import notifications from '@/locales/en/notifications.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

// The button has its own test (`SettingsDataResetButton.test.ts`); here it only has to be
// findable at the list, carrying the count, and able to say it cleared.
vi.mock('@/components/settings/SettingsDataResetButton.vue', () => ({
  default: {
    props: ['target', 'count'],
    emits: ['cleared'],
    template: '<button :data-testid="`data-reset-${target}`" :data-count="count" @click="$emit(\'cleared\')" />'
  }
}))

const { default: NotificationHistory } = await import('./NotificationHistory.vue')

function delivery(state: string, title: string) {
  return {
    id: title,
    rule_id: 'rule',
    target_id: 'target',
    idempotency_key: title,
    event: 'download_completed',
    title,
    body: '',
    state,
    attempt: 1,
    created_at: '2026-09-25T10:00:00Z',
    updated_at: '2026-09-25T10:00:00Z'
  }
}

function answer(deliveries: unknown[], clearable: number): void {
  get.mockImplementation(async (path: string) =>
    path === '/api/v1/system/data-reset'
      ? { data: { logs: 0, audit: 0, stats: 0, notifications: clearable } }
      : { data: deliveries }
  )
}

describe('NotificationHistory clearing', () => {
  beforeEach(() => get.mockReset())

  it('offers the clear at the list, with the count a clear would take', async () => {
    answer([delivery('retrying', 'still owed'), delivery('delivered', 'sent'), delivery('failed', 'gave up')], 2)

    mountComponent(NotificationHistory, { messages: { notifications } })

    await screen.findByText('still owed')
    const button = screen.getByTestId('data-reset-notifications')
    await waitFor(() => expect(button.getAttribute('data-count')).toBe('2'))
  })

  it('reads the list and the count again once the history was cleared', async () => {
    answer([delivery('delivered', 'sent')], 1)
    mountComponent(NotificationHistory, { messages: { notifications } })
    await screen.findByText('sent')

    answer([delivery('retrying', 'still owed')], 0)
    await fireEvent.click(screen.getByTestId('data-reset-notifications'))

    await screen.findByText('still owed')
    expect(screen.queryByText('sent')).toBeNull()
    await waitFor(() => expect(screen.getByTestId('data-reset-notifications').getAttribute('data-count')).toBe('0'))
  })
})
