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

import { createTestI18n } from '@/test/mount'

import DataState from './DataState.vue'

function mount(props: Record<string, unknown>) {
  return render(DataState, {
    props,
    global: { plugins: [createTestI18n()] },
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
      global: { plugins: [createTestI18n()] },
      slots: { default: () => h('p', 'No accounts yet') }
    })

    expect(container.querySelector('.p-5')?.textContent).toBe('No accounts yet')
  })

  it('renders nothing at all once there is content', () => {
    const { container } = mount({ loading: false, empty: false })

    expect(container.textContent).toBe('')
  })
})
