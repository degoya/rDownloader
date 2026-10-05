/**
 * The three states of a fetched area, checked on the component that draws them (RD-104-07).
 *
 * The interesting assertion is the negative one: while the fetch is outstanding, and again
 * when it failed, the caller's empty state must not be in the DOM. That is the bug this
 * component exists to close — "no accounts" standing in for "still asking" and for "the
 * server said no".
 */
import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { h } from 'vue'

import { createTestI18n, uiStubs } from '@/test/mount'

import DataState from './DataState.vue'

function mount(props: Record<string, unknown>, stubs: Record<string, unknown> = uiStubs) {
  return render(DataState, {
    props,
    global: { plugins: [createTestI18n()], stubs: stubs as never },
    slots: { default: () => h('p', 'No accounts yet') }
  })
}

describe('DataState', () => {
  it('shows the loading surface and not the empty state while the fetch runs', () => {
    mount({ loading: true, empty: true })

    expect(screen.getByRole('status').textContent).toContain('Loading')
    expect(screen.queryByText('No accounts yet')).toBeNull()
  })

  it('shows the empty state once the fetch settled with nothing', () => {
    mount({ loading: false, empty: true })

    expect(screen.getByText('No accounts yet')).toBeTruthy()
    expect(screen.queryByRole('status')).toBeNull()
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('shows the failure as its own state rather than as an empty list', () => {
    mount({ loading: false, empty: true, error: 'The service did not answer' })

    expect(screen.getByRole('alert').textContent).toContain('The service did not answer')
    expect(screen.queryByText('No accounts yet')).toBeNull()
  })

  it('gives the empty state the classes the caller put on it', () => {
    const { container } = render(DataState, {
      props: { loading: false, empty: true },
      attrs: { class: 'p-5' },
      global: { plugins: [createTestI18n()], stubs: uiStubs as never },
      slots: { default: () => h('p', 'No accounts yet') }
    })

    expect(container.querySelector('.p-5')?.textContent).toBe('No accounts yet')
  })

  it('renders nothing at all once there is content', () => {
    const { container } = mount({ loading: false, empty: false })

    expect(container.textContent).toBe('')
  })

  // UAlert and UEmpty set no role of their own; the states keep the ones they announced with (RD-1110-11).
  it('draws a failure in a panel as an error notice that keeps its alert role', () => {
    const alert = { props: ['color', 'description'], template: '<div v-bind="$attrs" data-notice :data-color="color">{{ description }}</div>' }
    mount({ loading: false, error: 'The service did not answer' }, { ...uiStubs, UAlert: alert })

    const shown = screen.getByRole('alert')
    expect(shown.hasAttribute('data-notice')).toBe(true)
    expect(shown.getAttribute('data-color')).toBe('error')
    expect(shown.textContent).toContain('The service did not answer')
  })

  it('keeps an inline failure a line of text with its alert role', () => {
    mount({ loading: false, error: 'The service did not answer', variant: 'inline' })

    expect(screen.getByRole('alert').tagName).toBe('P')
  })

  it('frames the loading panel as an empty state that keeps its status role', () => {
    const empty = { template: '<div v-bind="$attrs" data-empty><slot name="body" /><slot name="footer" /></div>' }
    mount({ loading: true }, { ...uiStubs, UEmpty: empty })

    const shown = screen.getByRole('status')
    expect(shown.hasAttribute('data-empty')).toBe(true)
    expect(shown.getAttribute('aria-live')).toBe('polite')
    expect(shown.textContent).toContain('Loading')
  })
})
