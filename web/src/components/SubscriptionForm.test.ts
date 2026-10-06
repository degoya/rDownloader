/**
 * The poll interval is obligatory (RD-1110-10, RD-1120-09).
 *
 * The API types it as a plain number, so an emptied field is not sent as `null`: the field is
 * `required`, and the browser holds the form's submit until it holds a number again.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'

const post = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), POST: post, PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => ''
}))
vi.mock('@nuxt/ui/composables', () => ({
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(null) }) }) })
}))

import SubscriptionForm from './SubscriptionForm.vue'

describe('the poll interval of a subscription', () => {
  it('holds the submit while it is empty', async () => {
    mountComponent(SubscriptionForm, { messages: { subscriptions }, props: { categories: [], editing: null } })
    const interval = screen.getByLabelText(subscriptions.form.interval) as HTMLInputElement
    expect(interval.validity.valueMissing).toBe(false)

    await fireEvent.update(interval, '')

    expect(interval.validity.valueMissing).toBe(true)
  })
})
