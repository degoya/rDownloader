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
  UInput: { props: ['modelValue'], template: '<input :value="modelValue" />' },
  UIcon: { props: ['name'], template: '<span :data-icon="name" />' },
  USwitch: { template: '<input type="checkbox" />' },
  UButton: { template: '<button><slot /></button>' }
}

function server(maxConnections: number, enabled = true) {
  return { id: 'srv', name: 'srv', host: 'news.example', port: 563, tls: true, priority: 0, max_connections: maxConnections, enabled }
}

function mount(cap: number) {
  const settings = { nntp_connections_per_file: cap, max_active_files: 3, max_chunks_per_file: 4, max_connections_per_host: 6, max_retries: 8, ui_port: null, storage_minimum_free_bytes: '0' }
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
