/**
 * The desktop agent's game mode, set from Settings > Clients & API > Desktop (RD-1240-19): the
 * triggers and the action are saved as a whole, a profile is asked for before anything is sent,
 * and a refusal of the service is shown. The on/off switch is the tray's (RD-1240-23).
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import settings from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import CaptureGameModeForm from './CaptureGameModeForm.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), PATCH: vi.fn() },
  responseError: vi.fn(() => 'refused by the service')
}))

const PROFILE = { id: '0192f7a0-0000-7000-8000-000000000001', name: 'Gaming' }
const OFF = { enabled: true, full_screen: false, processes: [], action: 'pause', profile_id: null }

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
/** The tags as one comma-separated text, which is how they are typed in. */
const UInputTags = {
  props: ['modelValue'],
  emits: ['update:modelValue'],
  template: '<input v-bind="$attrs" :value="(modelValue ?? []).join(\',\')" @input="$emit(\'update:modelValue\', $event.target.value.split(\',\'))" />'
}
const stubs = { UForm, UInputTags }
const LABELS = settings.capture_agent.game_mode

function mount(gameMode: Record<string, unknown> = OFF) {
  return mountComponent(CaptureGameModeForm, { messages: { settings, common }, stubs, props: { gameMode } })
}

function saved(gameMode: Record<string, unknown>) {
  return { data: { clipboard_paused: false, shortcuts: {}, default_shortcuts: {}, report: null, game_mode: gameMode } }
}

describe('CaptureGameModeForm', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset().mockResolvedValue({ data: [PROFILE] } as never)
    vi.mocked(api.PATCH).mockReset()
  })

  it('saves full screen and the named programs, pausing the queue', async () => {
    const stored = { ...OFF, full_screen: true, processes: ['game.exe', 'obs64'] }
    vi.mocked(api.PATCH).mockResolvedValue(saved(stored) as never)
    const { container, emitted } = mount()

    await fireEvent.click(screen.getByRole('switch', { name: LABELS.full_screen.label }))
    await fireEvent.update(screen.getByTestId('game-mode-processes'), 'game.exe,obs64')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)

    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/settings/capture-agent', { body: { game_mode: stored } })
    expect(await screen.findByTestId('game-mode-saved')).toBeTruthy()
    expect(emitted().saved).toHaveLength(1)
  })

  it('shows the tray’s switch and saves it off without losing the programs', async () => {
    const watching = { ...OFF, processes: ['game.exe'] }
    const off = { ...watching, enabled: false }
    vi.mocked(api.PATCH).mockResolvedValue(saved(off) as never)
    const { container, rerender } = mount(watching)
    const toggle = screen.getByRole('switch', { name: LABELS.enabled.label })
    expect(toggle.getAttribute('aria-checked')).toBe('true')

    await fireEvent.click(toggle)
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/settings/capture-agent', { body: { game_mode: off } })

    // Switched on again from the tray: the page reloads and shows it.
    await rerender({ gameMode: watching })
    await waitFor(() => expect(screen.getByRole('switch', { name: LABELS.enabled.label }).getAttribute('aria-checked')).toBe('true'))
  })

  it('reads settings stored before the switch as switched on', () => {
    const { enabled: _, ...before } = OFF
    mount(before)
    expect(screen.getByRole('switch', { name: LABELS.enabled.label }).getAttribute('aria-checked')).toBe('true')
  })

  it('asks for a profile before switching one on', async () => {
    const { container } = mount({ ...OFF, processes: ['game'] })
    await fireEvent.click(screen.getByRole('radio', { name: LABELS.action.profile }))
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    expect(api.PATCH).not.toHaveBeenCalled()

    vi.mocked(api.PATCH).mockResolvedValue(saved({ ...OFF, processes: ['game'], action: 'profile', profile_id: PROFILE.id }) as never)
    await waitFor(() => expect(screen.getByRole('option', { name: PROFILE.name })).toBeTruthy())
    await fireEvent.update(screen.getByTestId('game-mode-profile'), PROFILE.id)
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    expect(api.PATCH).toHaveBeenCalledWith('/api/v1/settings/capture-agent', {
      body: { game_mode: { ...OFF, processes: ['game'], action: 'profile', profile_id: PROFILE.id } }
    })
  })

  it('shows the service’s refusal', async () => {
    vi.mocked(api.PATCH).mockResolvedValue({ error: { code: 'capture.game_mode_process_invalid' } } as never)
    const { container } = mount()
    await fireEvent.update(screen.getByTestId('game-mode-processes'), 'C:\\game.exe')
    await fireEvent.submit(container.querySelector('form') as HTMLFormElement)
    expect((await screen.findByTestId('game-mode-error')).textContent).toContain('refused by the service')
  })

  it('renders without an axe violation', async () => {
    const { container } = mount({ ...OFF, action: 'profile', profile_id: PROFILE.id })
    await waitFor(() => expect(screen.getByRole('option', { name: PROFILE.name })).toBeTruthy())
    expect(await axeViolations(container)).toBe('')
  })
})
