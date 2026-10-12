/**
 * RD-1260-12: the level in Settings › System is a field of the settings document, saved with the
 * page; under `RUST_LOG` it is locked and the notice names the value.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ref } from 'vue'

import type { LogLevelSettings } from '@/api/logLevel'
import logs from '@/locales/en/logs.json'
import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { mountComponent } from '@/test/mount'

const fetchLogLevel = vi.fn()
vi.mock('@/api/logLevel', () => ({
  SERVICE_LOG_LEVELS: ['error', 'warn', 'info', 'debug'],
  fetchLogLevel: () => fetchLogLevel()
}))

const { default: SettingsLogLevel } = await import('./SettingsLogLevel.vue')

function mount(document: LogLevelSettings, rustLog: string | null = null) {
  fetchLogLevel.mockResolvedValue({
    ok: true,
    data: { level: 'info', debug: null, rust_log: rustLog, areas: [], minutes: [15, 30, 60] }
  })
  const model = ref(document)
  mountComponent(SettingsLogLevel, {
    props: { modelValue: model.value, 'onUpdate:modelValue': (next: LogLevelSettings) => { model.value = next } },
    messages: { logs, settings: settingsMessages }
  })
  return model
}

describe('SettingsLogLevel', () => {
  beforeEach(() => fetchLogLevel.mockReset())

  it('shows info for a document without a level and writes the choice into the document', async () => {
    const document: LogLevelSettings = defaultSettings()
    const model = mount(document)
    const select = screen.getByTestId('settings-log-level-select') as HTMLSelectElement
    expect(select.value).toBe('info')

    await fireEvent.update(select, 'warn')

    expect(model.value.log_level).toBe('warn')
  })

  it('is locked under RUST_LOG and names its value', async () => {
    mount({ ...defaultSettings(), log_level: 'debug' }, 'rd_=trace')
    const notice = await screen.findByTestId('settings-log-level-locked')

    expect(notice.textContent).toContain(logs.recording.locked.replace('{value}', 'rd_=trace'))
    expect((screen.getByTestId('settings-log-level-select') as HTMLSelectElement).disabled).toBe(true)
    expect((screen.getByTestId('settings-log-level-select') as HTMLSelectElement).value).toBe('debug')
  })
})
