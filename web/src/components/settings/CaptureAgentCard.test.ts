/**
 * The capture agent's clipboard pause and shortcuts, set from Settings > Clients & API > Desktop
 * (RD-1180-01, RD-1180-03): the pause saves at once, the shortcuts as a set; a refusal stands
 * under the field it names; what the agent reported is shown; a change elsewhere reloads.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import settings from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import CaptureAgentCard from './CaptureAgentCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PATCH: vi.fn() },
  responseError: vi.fn(() => 'refused by the service')
}))

let changed: ((event: MessageEvent<string>) => void) | null = null
const released = vi.fn()
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (handlers: Record<string, (event: MessageEvent<string>) => void>) => {
    changed = handlers['capture.changed'] ?? null
    return () => { changed = null; released() }
  }
}))

const DEFAULTS = {
  open: 'CmdOrCtrl+Alt+O',
  start_all: 'CmdOrCtrl+Alt+G',
  pause_all: 'CmdOrCtrl+Alt+P',
  pause_half_hour: 'CmdOrCtrl+Alt+K',
  pause_hour: 'CmdOrCtrl+Alt+Shift+K',
  clipboard_watch: 'CmdOrCtrl+Alt+Z',
  send_clipboard: 'CmdOrCtrl+Alt+V',
  game_mode: null,
  install_server_update: null,
  auto_install: null,
  quit: null,
  add_all_from_linkgrabber: null,
  add_all_from_linkgrabber_paused: null,
  install_update: null,
  restart_server: null
}

function answer(overrides: Record<string, unknown> = {}) {
  return { clipboard_paused: false, shortcuts: { ...DEFAULTS }, default_shortcuts: { ...DEFAULTS }, report: null, game_mode: {}, ...overrides }
}

/** Runs `validate` and submits only without errors, as `UForm` does. */
const UForm = {
  props: ['state', 'validate'],
  emits: ['submit'],
  template: '<form v-bind="$attrs" @submit.prevent="send"><slot /></form>',
  methods: {
    async send(this: { validate?: (state: unknown) => unknown[], state: unknown, $emit: (name: string, value: unknown) => void }) {
      const errors = this.validate ? await this.validate(this.state) : []
      if (!errors.length) this.$emit('submit', { data: this.state })
    }
  }
}
const stubs = {
  UForm,
  // Its own test is CaptureGameModeForm.test.ts.
  CaptureGameModeForm: true,
  UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' },
  UBadge: { props: ['label'], template: '<span v-bind="$attrs">{{ label }}</span>' },
  // The error under the field, where the real one draws it.
  UFormField: { props: ['label', 'error'], template: '<div v-bind="$attrs"><label>{{ label }}<slot /></label><p v-if="error">{{ error }}</p></div>' }
}
const LABELS = settings.capture_agent

function mount() {
  return mountComponent(CaptureAgentCard, { messages: { settings, common }, stubs })
}

async function loaded(): Promise<void> {
  await screen.findByTestId('shortcuts-form')
}

function field(command: string): HTMLElement {
  return screen.getByTestId(`shortcut-${command}`)
}

async function record(command: string, code: string, held: Record<string, boolean>): Promise<void> {
  const button = field(command).querySelector('[data-testid="shortcut-record"]') as HTMLElement
  await fireEvent.click(button)
  await fireEvent.keyDown(button, { code, ...held })
}

describe('CaptureAgentCard', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset().mockResolvedValue({ data: answer() } as never)
    vi.mocked(api.PATCH).mockReset()
  })

  it('switches the clipboard pause at once and shows that capture is paused', async () => {
    vi.mocked(api.PATCH).mockResolvedValue({ data: answer({ clipboard_paused: true }) } as never)
    mount()
    await loaded()
    expect(screen.getByTestId('clipboard-state').textContent).toContain(LABELS.watching_badge)

    await fireEvent.click(screen.getByRole('switch', { name: LABELS.clipboard_paused.label }))

    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/settings/capture-agent', { body: { clipboard_paused: true } })
    await waitFor(() => expect(screen.getByTestId('clipboard-state').textContent).toContain(LABELS.paused_badge))
  })

  it('saves the shortcuts as one set, recorded and cleared', async () => {
    vi.mocked(api.PATCH).mockResolvedValue({ data: answer() } as never)
    const { container } = mount()
    await loaded()
    await record('quit', 'KeyQ', { ctrlKey: true, altKey: true, shiftKey: true })
    await fireEvent.click(field('open').querySelector('[data-testid="shortcut-none"]') as HTMLElement)

    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/settings/capture-agent', {
      body: { shortcuts: { ...DEFAULTS, open: null, quit: 'Ctrl+Alt+Shift+Q' } }
    })
    expect(await screen.findByTestId('shortcuts-saved')).toBeTruthy()
  })

  it('names a duplicate under its field before anything is sent', async () => {
    const { container } = mount()
    await loaded()
    await record('quit', 'KeyV', { ctrlKey: true, altKey: true })

    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    expect(api.PATCH).not.toHaveBeenCalled()
    expect(field('quit').textContent).toContain(LABELS.commands.send_clipboard)
  })

  it('puts the service’s refusal under the field it names', async () => {
    vi.mocked(api.PATCH).mockResolvedValue({
      error: { error: 'refused', code: 'capture.shortcut_reserved', params: { command: 'quit' } }
    } as never)
    const { container } = mount()
    await loaded()
    await record('quit', 'Delete', { ctrlKey: true, altKey: true })

    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    await waitFor(() => expect(field('quit').textContent).toContain('refused by the service'))
    expect(field('open').textContent).not.toContain('refused by the service')
  })

  it('shows what the agent reported about its shortcuts', async () => {
    vi.mocked(api.GET).mockResolvedValue({
      data: answer({ report: { platform: 'linux', refused: [], unavailable: 'wayland', reported_at: '2026-10-07T10:00:00Z' } })
    } as never)
    mount()
    await loaded()
    const report = screen.getByTestId('shortcut-report')
    expect(report.textContent).toContain(LABELS.report.title_unavailable)
    expect(report.textContent).toContain(LABELS.report.unavailable.wayland)
  })

  it('lists the commands another program holds', async () => {
    vi.mocked(api.GET).mockResolvedValue({
      data: answer({ report: { platform: 'windows', refused: ['pause_all'], unavailable: null, reported_at: null } })
    } as never)
    mount()
    await loaded()
    expect(screen.getByTestId('shortcut-report').textContent).toContain(LABELS.commands.pause_all)
  })

  it('reloads when the tray changed the settings, and stops listening when it goes', async () => {
    const { unmount } = mount()
    await loaded()
    vi.mocked(api.GET).mockResolvedValue({ data: answer({ clipboard_paused: true }) } as never)

    changed?.(new MessageEvent('capture.changed', { data: JSON.stringify({ payload: { resource: 'capture_agent_settings' } }) }))
    await waitFor(() => expect(screen.getByTestId('clipboard-state').textContent).toContain(LABELS.paused_badge))

    vi.mocked(api.GET).mockClear()
    changed?.(new MessageEvent('capture.changed', { data: JSON.stringify({ payload: { resource: 'capture_token' } }) }))
    expect(api.GET).not.toHaveBeenCalled()

    unmount()
    expect(released).toHaveBeenCalled()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    await loaded()
    expect(await axeViolations(container)).toBe('')
  })
})
