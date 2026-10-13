/**
 * The rule editor's own wording (RD-1280-02): two fields, the placeholders the chosen events fill
 * and a preview with the service's sample data.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
import notifications from '@/locales/en/notifications.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

const TABLE: NotificationPlaceholderTable = {
  placeholders: [
    { name: 'event', example: 'package_completed' },
    { name: 'time', example: '14:32' },
    { name: 'title', example: 'Package finished: Holiday photos' },
    { name: 'message', example: 'Holiday photos (completed)' },
    { name: 'package', example: 'Holiday photos' },
    { name: 'size', example: '1.4 GiB' },
    { name: 'account', example: 'Main (example.com)' }
  ],
  events: [
    { event: 'package_completed', takes_template: true, placeholders: ['event', 'time', 'title', 'message', 'package', 'size'] },
    { event: 'account_invalid', takes_template: true, placeholders: ['event', 'time', 'title', 'message', 'account'] },
    { event: 'download_started', takes_template: false, placeholders: [] }
  ]
}

const fetchTable = vi.fn(async () => ({ ok: true as const, data: TABLE }))
vi.mock('@/api/notificationPlaceholders', () => ({ fetchNotificationPlaceholders: () => fetchTable() }))

const { default: NotificationTemplateFields } = await import('./NotificationTemplateFields.vue')

/** The form field with its error and description shown, as the real one shows them. */
const UFormField = {
  props: ['label', 'error', 'description'],
  template: '<div><label>{{ label }}<slot /></label><p v-if="description">{{ description }}</p><p v-if="error" role="alert">{{ error }}</p></div>'
}

function mount(events: NotificationEvent[], title: string | null = null, body: string | null = null) {
  return mountComponent(NotificationTemplateFields, {
    messages: { notifications },
    props: { events, title, body },
    stubs: { UFormField }
  })
}

const placeholderGroup = () => screen.findByRole('group', { name: notifications.template.placeholders_label })

describe('NotificationTemplateFields', () => {
  it('renders without an axe violation', async () => {
    const { container } = mount(['package_completed'], '{package} done')
    await placeholderGroup()
    expect(await axeViolations(container)).toBe('')
  })

  it('lists what the chosen events fill and nothing else', async () => {
    mount(['package_completed'])
    const group = await placeholderGroup()
    const names = within(group).getAllByRole('button').map(button => button.textContent)
    expect(names).toEqual(['{event}', '{time}', '{title}', '{message}', '{package}', '{size}'])
  })

  it('adds a placeholder to the field last in focus', async () => {
    const { emitted } = mount(['package_completed'], 'Done: ')
    const group = await placeholderGroup()
    await fireEvent.focus(screen.getByRole('textbox', { name: new RegExp(notifications.template.title_label) }))
    await fireEvent.click(within(group).getByRole('button', { name: /\{package\}/ }))
    expect(emitted()['update:title']).toEqual([['Done: {package}']])
  })

  it('previews the wording with sample data, an unfilled placeholder empty', async () => {
    mount(['package_completed'], '{package} ✓ {time}', 'Size: {size}{account}')
    await placeholderGroup()
    expect(screen.getByTestId('template-preview-title').textContent).toBe('Holiday photos ✓ 14:32')
    expect(screen.getByTestId('template-preview-body').textContent).toBe('Size: 1.4 GiB')
  })

  it('previews the built-in wording for a part left empty', async () => {
    mount(['package_completed'])
    await placeholderGroup()
    expect(screen.getByTestId('template-preview-title').textContent).toBe('Package finished: Holiday photos')
    expect(screen.getByTestId('template-preview-body').textContent).toBe('Holiday photos (completed)')
  })

  it('names a placeholder that does not exist before the save does', async () => {
    mount(['package_completed'], 'Done: {pakage}')
    await placeholderGroup()
    expect(screen.getByRole('alert').textContent).toBe('“{pakage}” is not a placeholder.')
  })

  it('says that a burst of starts keeps its summary', async () => {
    mount(['download_started'])
    await waitFor(() => expect(screen.getByText(notifications.template.summary_only)).toBeTruthy())
    expect(screen.queryByRole('group', { name: notifications.template.placeholders_label })).toBeNull()
  })
})
