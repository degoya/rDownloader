/**
 * RD-1260-12: the level shortcut and "debug for a while" above the log — choosing a level saves
 * it, a window is started with its area, plugin and minutes, the open one is shown with its end
 * and ended on a click, it is read again once its end has passed, and `RUST_LOG` locks it all.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import type { LogLevelState } from '@/api/logLevel'
import common from '@/locales/en/common.json'
import logs from '@/locales/en/logs.json'
import server from '@/locales/en/server.json'
import { mountComponent } from '@/test/mount'
import { formatMoment } from '@/utils/format'

const fetchLogLevel = vi.fn()
const saveLogLevel = vi.fn()
const startLogDebug = vi.fn()
const stopLogDebug = vi.fn()
vi.mock('@/api/logLevel', () => ({
  SERVICE_LOG_LEVELS: ['error', 'warn', 'info', 'debug'],
  fetchLogLevel: () => fetchLogLevel(),
  saveLogLevel: (level: string) => saveLogLevel(level),
  startLogDebug: (area: string, minutes: number, pluginId: string | null) => startLogDebug(area, minutes, pluginId),
  stopLogDebug: () => stopLogDebug()
}))
vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async () => ({
      data: {
        installed: [
          { id: 'p-mega', name: 'MEGA', active: true },
          { id: 'p-old', name: 'Old MEGA', active: false },
          { id: 'p-ddl', name: 'DDownload', active: true }
        ]
      }
    }))
  }
}))

const { default: LogLevelPanel } = await import('./LogLevelPanel.vue')

const labels = logs.recording

function state(patch: Partial<LogLevelState> = {}): LogLevelState {
  return {
    level: 'info',
    debug: null,
    rust_log: null,
    areas: ['plugin', 'provider', 'usenet', 'torrent', 'media', 'linkgrabber', 'subscriptions', 'http', 'scheduler'],
    minutes: [15, 30, 60],
    ...patch
  }
}

async function mount(shown: LogLevelState): Promise<void> {
  fetchLogLevel.mockResolvedValue({ ok: true, data: shown })
  mountComponent(LogLevelPanel, { messages: { logs, common, server } })
  await vi.waitFor(() => expect((screen.getByTestId('log-level-choice') as HTMLSelectElement).disabled).toBe(false))
}

describe('LogLevelPanel', () => {
  beforeEach(() => {
    for (const mock of [fetchLogLevel, saveLogLevel, startLogDebug, stopLogDebug]) mock.mockReset()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it('offers the four levels and saves the one chosen', async () => {
    await mount(state())
    const select = screen.getByTestId('log-level-choice') as HTMLSelectElement
    expect([...select.options].map(option => option.value)).toEqual(['error', 'warn', 'info', 'debug'])
    expect(select.value).toBe('info')

    saveLogLevel.mockResolvedValue({ ok: true, data: state({ level: 'debug' }) })
    await fireEvent.update(select, 'debug')

    expect(saveLogLevel).toHaveBeenCalledWith('debug')
    await vi.waitFor(() => expect(select.value).toBe('debug'))
  })

  it('starts a window with its area and minutes, and narrows the plugin area to one plugin', async () => {
    await mount(state())
    const plugins = screen.getByTestId('log-debug-plugin') as HTMLSelectElement
    expect([...plugins.options].map(option => option.textContent)).toEqual([labels.plugin_all, 'DDownload', 'MEGA'])

    startLogDebug.mockResolvedValue({ ok: true, data: state() })
    await fireEvent.update(plugins, 'p-mega')
    await fireEvent.update(screen.getByTestId('log-debug-minutes'), '30')
    await fireEvent.click(screen.getByTestId('log-debug-start'))
    expect(startLogDebug).toHaveBeenLastCalledWith('plugin', 30, 'p-mega')

    await fireEvent.update(screen.getByTestId('log-debug-area'), 'usenet')
    expect(screen.queryByTestId('log-debug-plugin')).toBeNull()
    await fireEvent.click(screen.getByTestId('log-debug-start'))
    expect(startLogDebug).toHaveBeenLastCalledWith('usenet', 30, null)
  })

  it('shows what is on debug until when, and ends it on a click', async () => {
    const until = '2026-10-12T18:30:00Z'
    await mount(state({ debug: { area: 'usenet', until } }))
    const notice = screen.getByTestId('log-debug-active')
    expect(notice.textContent).toContain(
      labels.active.replace('{area}', labels.areas.usenet).replace('{until}', formatMoment(until))
    )

    stopLogDebug.mockResolvedValue({ ok: true, data: state() })
    await fireEvent.click(screen.getByTestId('log-debug-stop'))
    expect(stopLogDebug).toHaveBeenCalled()
    await vi.waitFor(() => expect(screen.queryByTestId('log-debug-active')).toBeNull())
  })

  it('reads the state again once the window has ended by itself', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
    vi.setSystemTime(new Date('2026-10-12T18:00:00Z'))
    await mount(state({ debug: { area: 'http', until: '2026-10-12T18:15:00Z' } }))
    expect(screen.getByTestId('log-debug-active')).toBeTruthy()

    fetchLogLevel.mockResolvedValue({ ok: true, data: state() })
    await vi.advanceTimersByTimeAsync(15 * 60 * 1000 + 1000)

    expect(fetchLogLevel).toHaveBeenCalledTimes(2)
    await vi.waitFor(() => expect(screen.queryByTestId('log-debug-active')).toBeNull())
  })

  it('locks every control under RUST_LOG and names its value', async () => {
    fetchLogLevel.mockResolvedValue({ ok: true, data: state({ rust_log: 'rd_http=debug' }) })
    mountComponent(LogLevelPanel, { messages: { logs, common, server } })
    const notice = await screen.findByTestId('log-level-locked')

    expect(notice.textContent).toContain(labels.locked.replace('{value}', 'rd_http=debug'))
    expect((screen.getByTestId('log-level-choice') as HTMLSelectElement).disabled).toBe(true)
    expect((screen.getByTestId('log-debug-start') as HTMLButtonElement).disabled).toBe(true)
  })

  it('says a refusal under the controls in the reader\'s words', async () => {
    await mount(state())
    startLogDebug.mockResolvedValue({
      ok: false,
      status: 400,
      message: { code: 'diagnostics.debug_minutes_invalid', message: 'A debug window lasts 15, 30 or 60 minutes', params: {} }
    })
    await fireEvent.click(screen.getByTestId('log-debug-start'))

    const refusal = await screen.findByTestId('log-level-refusal')
    expect(refusal.textContent).toContain(server.codes['diagnostics.debug_minutes_invalid'])
  })
})
