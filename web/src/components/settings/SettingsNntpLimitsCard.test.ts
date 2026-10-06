import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { UsenetServer } from '@/api/types'
import settingsMessages from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
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
  it('says that 0 follows the enabled servers, against the largest of them', () => {
    mount(0, [server(10), server(6), server(40, false)])

    expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('currently 10 on the largest')
    expect(screen.getByTestId('nntp-cap-hint').className).toContain('text-muted')
  })

  it('warns when the cap is below what the largest enabled server allows', () => {
    // The cap applies per server: two servers of 10 and a cap of 8 bind, and the sum is not the measure.
    mount(8, [server(10), server(10)])

    expect(screen.getByTestId('nntp-cap-hint').textContent).toContain('allows 10 connections; one file gets at most 8 per server')
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
