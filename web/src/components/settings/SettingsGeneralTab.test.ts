import { render, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import settingsMessages from '@/locales/en/settings.json'

import SettingsGeneralTab from './SettingsGeneralTab.vue'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { settings: settingsMessages } } })
const components = {
  UFormField: { props: ['label', 'description'], template: '<label><span>{{ label }}</span><slot /></label>' },
  UInput: { props: ['modelValue'], template: '<input v-bind="$attrs" :value="modelValue" />' },
  UIcon: { props: ['name'], template: '<span :data-icon="name" />' },
  USwitch: { template: '<input type="checkbox" />' },
  UButton: { template: '<button><slot /></button>' }
}

function server(maxConnections: number, enabled = true) {
  return { id: 'srv', name: 'srv', host: 'news.example', port: 563, tls: true, priority: 0, max_connections: maxConnections, enabled }
}

function mount(cap: number, minimumFreeBytes = '0') {
  const settings = { nntp_connections_per_file: cap, nntp_parallel_files: 0, max_active_files: 3, max_chunks_per_file: 4, max_connections_per_host: 6, max_retries: 8, ui_port: null, storage_minimum_free_bytes: minimumFreeBytes }
  return render(SettingsGeneralTab, {
    props: { modelValue: settings as never, speedMib: null },
    global: { plugins: [i18n], components }
  })
}

/** RD-108-25: the two connection numbers no longer contradict each other silently. */
describe('SettingsGeneralTab NNTP connection cap', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
  })

  it('says that 0 follows the enabled servers, against the largest of them', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [server(10), server(6), server(40, false)] } as never)

    mount(0)

    await waitFor(() => {
      expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('currently 10 on the largest')
    })
    expect(screen.getByTestId('nntp-cap-hint').className).toContain('text-muted')
  })

  it('warns when the cap is below what the largest enabled server allows', async () => {
    // The cap applies per server: two servers of 10 and a cap of 8 bind, and the sum is not the measure.
    vi.mocked(api.GET).mockResolvedValue({ data: [server(10), server(10)] } as never)

    mount(8)

    await waitFor(() => {
      expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('allows 10 connections; one file gets at most 8 per server')
    })
    expect(screen.getByTestId('nntp-cap-hint').className).toContain('text-warning')
  })

  it('says a cap at or above the largest server is not binding', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [server(10)] } as never)

    mount(16)

    await waitFor(() => {
      expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('Not binding')
    })
  })

  it('shows no hint when no enabled server exists, rather than a zero', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [server(10, false)] } as never)

    mount(0)

    await waitFor(() => {
      expect(vi.mocked(api.GET)).toHaveBeenCalled()
    })
    expect(screen.queryByTestId('nntp-cap-hint')).toBeNull()
  })

  it('shows no hint when the server list cannot be loaded', async () => {
    vi.mocked(api.GET).mockRejectedValue(new Error('network down') as never)

    mount(8)

    await waitFor(() => {
      expect(vi.mocked(api.GET)).toHaveBeenCalled()
    })
    expect(screen.queryByTestId('nntp-cap-hint')).toBeNull()
  })
})

/** RD-130-22: the number of Usenet files at once is a setting, automatic by default. */
describe('SettingsGeneralTab Usenet files at once', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('offers the setting with automatic as its value', async () => {
    mount(0)

    expect(screen.getByText('Usenet files at once')).toBeTruthy()
    expect((screen.getByTestId('nntp-parallel-files') as HTMLInputElement).value).toBe('0')
    await waitFor(() => {
      expect(vi.mocked(api.GET)).toHaveBeenCalled()
    })
  })
})

/** RD-150-15: the hand-set upload limit sits beside the speed limit, in MiB/s. */
describe('SettingsGeneralTab upload limit', () => {
  it('shows the stored bytes per second as MiB/s', () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    const settings = { nntp_connections_per_file: 0, nntp_parallel_files: 0, max_active_files: 3, max_chunks_per_file: 4, max_connections_per_host: 6, max_retries: 8, ui_port: null, storage_minimum_free_bytes: '0', upload_limit_bytes_per_second: String(3 * 1024 ** 2) }
    render(SettingsGeneralTab, {
      props: { modelValue: settings as never, speedMib: null },
      global: { plugins: [i18n], components }
    })

    expect(screen.getByText('Upload limit')).toBeTruthy()
    expect((screen.getByTestId('upload-limit') as HTMLInputElement).value).toBe('3')
  })
})

describe('SettingsGeneralTab number fields', () => {
  it('accepts the fraction a stored byte value converts to, so the page loads valid', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)

    // 0.25 GiB: with `step="1"` the field was `:invalid` on load and the save refused it.
    const { container } = mount(0, String(1024 ** 3 / 4))

    const fields = [...container.querySelectorAll<HTMLInputElement>('input[type="number"]')]
    const minimumFree = fields.find(field => field.value === '0.25')
    expect(minimumFree).toBeTruthy()
    expect(minimumFree?.validity.valid).toBe(true)
  })
})

/** RD-191-12: the automatic retry of failed downloads, its interval and rounds under its switch. */
describe('SettingsGeneralTab automatic retry', () => {
  function mountRetry(enabled: boolean) {
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    const settings = { nntp_connections_per_file: 0, nntp_parallel_files: 0, max_active_files: 3, max_chunks_per_file: 4, max_connections_per_host: 6, max_retries: 8, ui_port: null, storage_minimum_free_bytes: '0', auto_retry_failed: enabled, auto_retry_interval_hours: 12, auto_retry_max_rounds: 0 }
    return render(SettingsGeneralTab, {
      props: { modelValue: settings as never, speedMib: null },
      global: { plugins: [i18n], components }
    })
  }

  it('offers the switch and keeps its options out of sight while it is off', () => {
    mountRetry(false)

    expect(screen.getByText('Retry failed downloads automatically')).toBeTruthy()
    expect(screen.getByTestId('auto-retry-switch')).toBeTruthy()
    expect(screen.queryByTestId('auto-retry-options')).toBeNull()
  })

  it('shows the interval in hours and the rounds once it is on', () => {
    mountRetry(true)

    const interval = screen.getByTestId('auto-retry-interval') as HTMLInputElement
    expect(interval.value).toBe('12')
    expect(interval.min).toBe('1')
    expect(interval.max).toBe('24')
    const rounds = screen.getByTestId('auto-retry-rounds') as HTMLInputElement
    expect(rounds.value).toBe('0')
    expect(rounds.max).toBe('100')
    expect(screen.getByText('Rounds per download')).toBeTruthy()
  })
})
