/**
 * Forms outside the settings document ask before a leave drops what was typed into them (191-01
 * N3, RD-1120-15): a new subscription and an automation being edited. Each view is rendered
 * through `RouterView` at its own address, because the router's leave guard only reaches a
 * component the router rendered; the confirmation is a function each test answers. The proxy
 * form's case sits with the settings document's in `SettingsUnsavedGuard.test.ts`.
 */
import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { type Component } from 'vue'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import automation from '@/locales/en/automation.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { createTestI18n, uiStubs } from '@/test/mount'

const VOCABULARY = {
  triggers: ['download_completed'],
  fields: ['name'],
  operators: ['equals'],
  action_kinds: ['pause_package'],
  max_actions: 10,
  max_condition_depth: 3
}

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => ({ data: path === '/api/v1/automations/vocabulary' ? VOCABULARY : [] })),
    POST: vi.fn(),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: () => 'The service did not answer',
  resultMessage: () => ''
}))
const confirm = vi.fn<() => Promise<boolean>>()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@/composables/useEventStream', () => ({ subscribeEvents: () => () => {} }))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))

const { default: SubscriptionsView } = await import('./SubscriptionsView.vue')
const { default: AutomationView } = await import('./AutomationView.vue')

async function mountAt(path: string, view: Component, messages: Record<string, unknown>): Promise<Router> {
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path, component: view },
      { path: '/queue', component: { template: '<p>queue</p>' } }
    ]
  })
  await router.push(path)
  await router.isReady()
  setActivePinia(createPinia())
  render({ template: '<RouterView />' }, {
    global: {
      plugins: [router, createTestI18n(messages)],
      stubs: { ...uiStubs, ConditionTree: true, AreaBackupButtons: true } as never
    }
  })
  return router
}

beforeEach(() => {
  confirm.mockReset()
  confirm.mockResolvedValue(false)
})

describe('a new subscription', () => {
  it('leaves without a question while the form is untouched', async () => {
    const router = await mountAt('/subscriptions', SubscriptionsView, { subscriptions })
    await screen.findByTestId('subscription-name')

    await router.push('/queue')

    expect(confirm).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/queue')
  })

  it('asks before the typed name is dropped, and Cancel keeps it', async () => {
    const router = await mountAt('/subscriptions', SubscriptionsView, { subscriptions })
    await fireEvent.update(await screen.findByTestId('subscription-name'), 'Nightly builds')

    await router.push('/queue')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/subscriptions')
    expect((screen.getByTestId('subscription-name') as HTMLInputElement).value).toBe('Nightly builds')
  })
})

describe('the automation editor', () => {
  it('asks before an edited draft is dropped, and not once the editor is closed', async () => {
    const router = await mountAt('/automation', AutomationView, { automation })
    await fireEvent.click(await screen.findByRole('button', { name: automation.create }))
    const form = await screen.findByTestId('automation-form')
    await waitFor(() => expect(form.querySelector('input[required]')).toBeTruthy())

    await router.push('/queue')
    expect(confirm).not.toHaveBeenCalled()
    await router.push('/automation')

    await fireEvent.click(await screen.findByRole('button', { name: automation.create }))
    const name = (await screen.findByTestId('automation-form')).querySelector('input[required]') as HTMLInputElement
    await fireEvent.update(name, 'Pause big ones')
    await router.push('/queue')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/automation')
  })
})
