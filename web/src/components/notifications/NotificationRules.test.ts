/**
 * Notification rules under the form standard (RD-150-11, RD-150-12).
 *
 * The events were a row of buttons told apart by colour alone — no `aria-pressed`, no group —
 * so a screen reader heard six buttons and nothing about which of them were chosen. A rule can
 * now also be copied to reuse its event choice.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { NotificationRule, NotificationTarget } from '@/api/types'
import common from '@/locales/en/common.json'
import notifications from '@/locales/en/notifications.json'
import { mountComponent } from '@/test/mount'

const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: (...args: unknown[]) => post(...args), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))

const { default: NotificationRules } = await import('./NotificationRules.vue')

const TARGET = { id: 'target', name: 'Ops hook', kind: 'webhook' } as NotificationTarget
const RULE = {
  id: 'r1', name: 'Failures', enabled: true, target_id: 'target',
  events: ['package_failed', 'storage_blocked'], category_id: 'films', min_severity: 'warning'
} as NotificationRule

function mount(rules: NotificationRule[] = []) {
  return mountComponent(NotificationRules, {
    messages: { notifications },
    props: { modelValue: rules, targets: [TARGET], categories: [{ id: 'films', name: 'Films' }], loading: false, loadError: null }
  })
}

describe('NotificationRules', () => {
  beforeEach(() => post.mockReset())

  it('offers the events as a named group of checkboxes that say which are chosen', async () => {
    mount()
    const group = screen.getByRole('group', { name: notifications.rule.events_label })
    const failed = within(group).getByRole('checkbox', { name: notifications.event.package_failed }) as HTMLInputElement
    expect(failed.checked).toBe(false)
    await fireEvent.click(failed)
    expect(failed.checked).toBe(true)
  })

  it('offers the operational events next to the queue events (RD-190-19)', () => {
    mount()
    const group = screen.getByRole('group', { name: notifications.rule.events_label })
    for (const event of ['backup_failed', 'backup_verify_failed', 'update_available', 'plugin_update_available', 'plugin_update_failed', 'account_expiring', 'account_invalid', 'usenet_quota_reached'] as const) {
      expect(within(group).getByRole('checkbox', { name: notifications.event[event] })).toBeTruthy()
    }
  })

  it('offers the Usenet set given up as beyond repair (RD-1100-02)', () => {
    mount()
    const group = screen.getByRole('group', { name: notifications.rule.events_label })
    expect(within(group).getByRole('checkbox', { name: notifications.event.usenet_job_hopeless })).toBeTruthy()
  })

  it('copies a rule with its target, events, category and severity, and opens the copy', async () => {
    const copy = { ...RULE, id: 'r2', name: `Failures (${common.copy_suffix})` }
    post.mockResolvedValueOnce({ data: copy })
    mount([RULE])

    await fireEvent.click(screen.getByRole('button', { name: common.actions.duplicate }))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post.mock.calls[0]?.[1]?.body).toEqual({
      name: `Failures (${common.copy_suffix})`,
      enabled: true,
      target_id: 'target',
      events: ['package_failed', 'storage_blocked'],
      category_id: 'films',
      min_severity: 'warning'
    })
    await screen.findByRole('heading', { name: notifications.rule.form_edit })
    const group = screen.getByRole('group', { name: notifications.rule.events_label })
    const checked = (within(group).getAllByRole('checkbox') as HTMLInputElement[]).filter(box => box.checked)
    expect(checked).toHaveLength(2)
    expect(screen.getByText(common.editing)).toBeTruthy()
  })
})
