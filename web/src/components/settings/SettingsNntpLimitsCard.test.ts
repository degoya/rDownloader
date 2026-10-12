import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { nextTick } from 'vue'

import type { UsenetServer } from '@/api/types'
import settingsMessages from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { useTransfersStore } from '@/stores/transfers'
import { mountComponent } from '@/test/mount'

import SettingsNntpLimitsCard from './SettingsNntpLimitsCard.vue'

/** The number field with the fraction digits its format allows, which the number tests read. */
const UInputNumber = {
  props: ['modelValue', 'formatOptions'],
  template: '<input role="spinbutton" v-bind="$attrs" :value="modelValue" :data-fraction-digits="formatOptions?.maximumFractionDigits" />'
}

function server(maxConnections: number, enabled = true): UsenetServer {
  return { id: 'srv', name: 'srv', host: 'news.example', port: 563, tls: true, priority: 0, max_connections: maxConnections, enabled } as UsenetServer
}

function mount(cap: number, servers: UsenetServer[] | null) {
  const settings = { nntp_connections_per_file: cap, nntp_parallel_files: 0 }
  return mountComponent(SettingsNntpLimitsCard, {
    messages: { settings: settingsMessages },
    props: { modelValue: settings as never, servers },
    stubs: { UInputNumber }
  })
}

/** RD-108-25: the two connection numbers no longer contradict each other silently. */
describe('SettingsNntpLimitsCard connection cap', () => {
  it('says that files at once share the enabled servers, against the largest of them', () => {
    mount(0, [server(10), server(6), server(40, false)])

    expect(screen.getByTestId('nntp-cap-hint').textContent).toContain("share the providers' connections (10 on the largest)")
    expect(screen.getByTestId('nntp-cap-hint').className).toContain('text-muted')
  })

  it('warns when the cap is below what the largest enabled server allows', () => {
    // The cap applies per server: two servers of 10 and a cap of 8 bind, and the sum is not the measure.
    mount(8, [server(10), server(10)])

    expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('allows 10 connections; files running at once get at most 8 per server together')
    expect(screen.getByTestId('nntp-cap-hint').className).toContain('text-warning')
  })

  it('says a cap at or above the largest server is not binding', () => {
    mount(16, [server(10)])

    expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('Not binding')
  })

  it('shows no hint when no enabled server exists, rather than a zero', () => {
    mount(0, [server(10, false)])

    expect(screen.queryByTestId('nntp-cap-hint')).toBeNull()
  })

  it('shows no hint while the server list is pending or failed', () => {
    mount(8, null)

    expect(screen.queryByTestId('nntp-cap-hint')).toBeNull()
  })
})

/** RD-1270-21: while files run, the hint says how they share the connections right now. */
describe('SettingsNntpLimitsCard live connections', () => {
  async function running(cap: number) {
    mount(cap, [server(10)])
    const transfers = useTransfersStore()
    transfers.downloadConnections = { a: 5, b: 5 }
    transfers.usenetServers = [{ id: 'srv', busy: 10, limit: 10 }]
    await nextTick()
    return screen.getByTestId('nntp-cap-hint').textContent ?? ''
  }

  it('replaces the general sentence with the files and their share', async () => {
    const hint = await running(0)
    expect(hint).toContain('Right now 2 files with 5 of 10 connections each.')
    expect(hint).not.toContain('on the largest')
  })

  it('adds the live sentence after a cap', async () => {
    const hint = await running(8)
    expect(hint).toContain('at most 8 per server together')
    expect(hint).toContain('Right now 2 files with 5 of 10 connections each.')
  })
})

/** RD-130-22: the number of Usenet files at once is a setting, automatic by default. */
describe('SettingsNntpLimitsCard Usenet files at once', () => {
  it('offers the setting with automatic as its value, as a whole number', () => {
    mount(0, [])

    expect(screen.getByText('Usenet files at once')).toBeTruthy()
    const field = screen.getByTestId('nntp-parallel-files') as HTMLInputElement
    expect(field.value).toBe('0')
    expect(field.dataset.fractionDigits).toBe('0')
  })

  it('says how its numbers relate to the connections of each provider (RD-1120-21)', () => {
    mount(0, [])

    expect(screen.getByText(settingsMessages.nntp_limits.description)).toBeTruthy()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount(8, [server(10)])
    expect(await axeViolations(container)).toBe('')
  })
})
