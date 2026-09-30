/**
 * The question before unsaved edits are lost (RD-180-16), against a real router: the guards are
 * the router's own in-component hooks, so the view is rendered through `RouterView` as the app
 * renders it. The confirmation is Nuxt UI's overlay, which needs `#imports` the Vitest config does
 * not provide, so it is replaced by a function each test answers.
 */
import { render } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, ref } from 'vue'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import common from '@/locales/en/common.json'
import { createTestI18n } from '@/test/mount'

import { useUnsavedGuard, type UnsavedGuardOptions } from './useUnsavedGuard'

const confirm = vi.fn<(options: { title: string, destructive?: boolean }) => Promise<boolean>>()
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))

const dirty = ref(false)

async function mountGuarded(options: UnsavedGuardOptions = {}): Promise<Router> {
  const Page = defineComponent({
    setup() { useUnsavedGuard(dirty, options) },
    template: '<p>page</p>'
  })
  const router = createRouter({
    history: createMemoryHistory(),
    routes: [
      { path: '/page/:part', component: Page },
      { path: '/elsewhere', component: { template: '<p>elsewhere</p>' } }
    ]
  })
  await router.push('/page/one')
  await router.isReady()
  render({ template: '<RouterView />' }, { global: { plugins: [router, createTestI18n()] } })
  return router
}

function unload(): Event {
  const event = new Event('beforeunload', { cancelable: true })
  window.dispatchEvent(event)
  return event
}

describe('useUnsavedGuard', () => {
  beforeEach(() => {
    confirm.mockReset()
    dirty.value = false
  })

  it('lets a clean view go without asking', async () => {
    const router = await mountGuarded()

    await router.push('/elsewhere')

    expect(confirm).not.toHaveBeenCalled()
    expect(router.currentRoute.value.path).toBe('/elsewhere')
  })

  it('asks before leaving with edits, and Cancel stays on the page', async () => {
    const router = await mountGuarded()
    dirty.value = true
    confirm.mockResolvedValue(false)

    await router.push('/elsewhere')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(confirm.mock.calls[0]?.[0]).toMatchObject({ title: common.unsaved.title, destructive: true })
    expect(router.currentRoute.value.path).toBe('/page/one')
  })

  it('leaves when the edits are discarded', async () => {
    const router = await mountGuarded()
    dirty.value = true
    confirm.mockResolvedValue(true)

    await router.push('/elsewhere')

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.path).toBe('/elsewhere')
  })

  it('does not ask when the view stays mounted and keeps its edits', async () => {
    const router = await mountGuarded()
    dirty.value = true

    await router.push('/page/two?tab=other')

    expect(confirm).not.toHaveBeenCalled()
    expect(router.currentRoute.value.fullPath).toBe('/page/two?tab=other')
  })

  it('asks on such a navigation when it would drop edits', async () => {
    const router = await mountGuarded({ dropsEdits: to => to.params.part !== 'one' })
    confirm.mockResolvedValue(false)

    await router.push('/page/one?tab=other')
    expect(confirm).not.toHaveBeenCalled()

    await router.push('/page/two')
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(router.currentRoute.value.fullPath).toBe('/page/one?tab=other')
  })

  it('has the browser ask before closing or reloading only while there are edits', async () => {
    await mountGuarded()

    expect(unload().defaultPrevented).toBe(false)
    dirty.value = true
    expect(unload().defaultPrevented).toBe(true)
    expect(confirm).not.toHaveBeenCalled()
  })
})
