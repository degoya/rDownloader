/**
 * The rule editor's own wording (RD-1280-02): two fields, the placeholders the chosen events fill
 * — put at the caret of the field last in focus (RD-1290-03) — and a preview with the service's
 * sample data.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { defineComponent, h, reactive, ref } from 'vue'

import type { NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
import notifications from '@/locales/en/notifications.json'
import { axeViolations } from '@/test/axe'
import { mountComponent, settle } from '@/test/mount'

const TABLE: NotificationPlaceholderTable = {
  placeholders: [
    { name: 'event', example: 'package_completed' },
    { name: 'time', example: '14:32' },
    { name: 'title', example: 'Package finished: Holiday photos' },
    { name: 'message', example: 'Holiday photos (completed)' },
    { name: 'package', example: 'Holiday photos' },
    { name: 'size', example: '1.4 GiB' },
    { name: 'account', example: 'Main (example.com)' },
    { name: 'file', example: 'IMG_2048.jpg' },
    { name: 'count', example: '3' }
  ],
  events: [
    {
      event: 'package_completed',
      takes_template: true,
      placeholders: ['event', 'time', 'title', 'message', 'package', 'size'],
      in_bursts: false,
      examples: []
    },
    { event: 'account_invalid', takes_template: true, placeholders: ['event', 'time', 'title', 'message', 'account'], in_bursts: false, examples: [] },
    {
      event: 'download_started',
      takes_template: true,
      placeholders: ['event', 'time', 'title', 'message', 'package', 'file', 'count'],
      in_bursts: true,
      examples: [
        { name: 'title', example: '3 downloads started' },
        { name: 'message', example: 'Started: a.mkv, b.mkv, c.mkv.' },
        { name: 'package', example: '' },
        { name: 'file', example: '' },
        { name: 'count', example: '3' }
      ]
    }
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

/**
 * `UInput` and `UTextarea` with the part of their contract the editor relies on: the native field,
 * exposed as `inputRef` and `textareaRef`; focus and blur reach it as attributes.
 */
function nativeField(tag: 'input' | 'textarea', exposed: 'inputRef' | 'textareaRef') {
  return defineComponent({
    inheritAttrs: false,
    props: { modelValue: { type: String, default: '' } },
    emits: ['update:modelValue'],
    setup(props, { attrs, emit, expose }) {
      const native = ref<HTMLInputElement | HTMLTextAreaElement | null>(null)
      expose({ [exposed]: native })
      return () => h(tag, {
        ...attrs,
        ref: native,
        value: props.modelValue,
        onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value)
      })
    }
  })
}
const UInput = nativeField('input', 'inputRef')
const UTextarea = nativeField('textarea', 'textareaRef')

function mount(events: NotificationEvent[], title: string | null = null, body: string | null = null) {
  return mountComponent(NotificationTemplateFields, {
    messages: { notifications },
    props: { events, title, body },
    stubs: { UFormField, UInput, UTextarea }
  })
}

/** The editor inside a parent that keeps what it writes, as the rule form does. */
function mountEditing(title: string | null, body: string | null) {
  const wording = reactive<{ title: string | null | undefined, body: string | null | undefined }>({ title, body })
  const Host = defineComponent(() => () => h(NotificationTemplateFields, {
    events: ['package_completed'],
    title: wording.title,
    body: wording.body,
    'onUpdate:title': (value?: string | null) => { wording.title = value },
    'onUpdate:body': (value?: string | null) => { wording.body = value }
  }))
  mountComponent(Host, { messages: { notifications }, stubs: { UFormField, UInput, UTextarea } })
  return wording
}

const titleInput = () => screen.getByRole('textbox', { name: new RegExp(notifications.template.title_label) }) as HTMLInputElement
const bodyInput = () => screen.getByRole('textbox', { name: new RegExp(notifications.template.body_label) }) as HTMLTextAreaElement

/** Focuses a field, places its caret or selection and leaves it, as a click on a placeholder does. */
function leaveAt(field: HTMLInputElement | HTMLTextAreaElement, start: number, end = start): void {
  field.focus()
  field.setSelectionRange(start, end)
  field.blur()
}

async function clickPlaceholder(name: string): Promise<void> {
  await fireEvent.click(within(await placeholderGroup()).getByRole('button', { name: new RegExp(`\\{${name}\\}`) }))
  await settle()
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

  it('adds a placeholder to the end of the field last in focus when no caret was kept', async () => {
    const { emitted } = mount(['package_completed'], 'Done: ')
    const group = await placeholderGroup()
    await fireEvent.focus(titleInput())
    await fireEvent.click(within(group).getByRole('button', { name: /\{package\}/ }))
    expect(emitted()['update:title']).toEqual([['Done: {package}']])
  })

  it('adds a placeholder to the end of the text when no field was ever focused', async () => {
    const wording = mountEditing(null, 'Size: ')
    await clickPlaceholder('size')
    expect(wording.body).toBe('Size: {size}')
  })

  it('puts a placeholder at the caret of the title, focused with the caret behind it', async () => {
    const wording = mountEditing('Done: !', null)
    await placeholderGroup()
    leaveAt(titleInput(), 6)
    await clickPlaceholder('package')
    expect(wording.title).toBe('Done: {package}!')
    expect(document.activeElement).toBe(titleInput())
    expect([titleInput().selectionStart, titleInput().selectionEnd]).toEqual([15, 15])

    await clickPlaceholder('size')
    expect(wording.title).toBe('Done: {package}{size}!')
  })

  it('replaces a selection in the text with the placeholder', async () => {
    const wording = mountEditing('Done', 'Size: XX here')
    await placeholderGroup()
    leaveAt(bodyInput(), 6, 8)
    await clickPlaceholder('size')
    expect(wording.body).toBe('Size: {size} here')
    expect(document.activeElement).toBe(bodyInput())
    expect([bodyInput().selectionStart, bodyInput().selectionEnd]).toEqual([12, 12])
    expect(wording.title).toBe('Done')
  })

  it('leaves out a placeholder that would pass the field\'s maximum length', async () => {
    const full = 'x'.repeat(195)
    const { emitted } = mount(['package_completed'], full)
    await placeholderGroup()
    leaveAt(titleInput(), 195)
    await clickPlaceholder('package')
    expect(emitted()['update:title']).toBeUndefined()
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

  it('offers a burst of starts its placeholders and previews a sample burst (RD-1291-01)', async () => {
    mount(['download_started'], 'MYTITLE {count}x [{file}]')
    const group = await placeholderGroup()
    const names = within(group).getAllByRole('button').map(button => button.textContent)
    expect(names).toEqual(['{event}', '{time}', '{title}', '{message}', '{package}', '{file}', '{count}'])
    // The hint's placeholders are passed in braces, so it reads as the catalogue writes it.
    expect(screen.getByTestId('template-burst-hint').textContent?.trim()).toBe(notifications.template.burst_hint)
    expect(screen.getByTestId('template-preview-title').textContent).toBe('MYTITLE 3x []')
    expect(screen.getByTestId('template-preview-body').textContent).toBe('Started: a.mkv, b.mkv, c.mkv.')
  })

  it('says nothing about bursts for a rule without them', async () => {
    mount(['package_completed'])
    await placeholderGroup()
    expect(screen.queryByTestId('template-burst-hint')).toBeNull()
  })
})
