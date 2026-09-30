import { render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import { createMemoryHistory, createRouter, type Router } from 'vue-router'

import { subTabItems, useSettingsSubTab } from './useSettingsSubTab'

/**
 * The sub-tab lives in the address and nowhere else (RD-180-15). These run the composable
 * against a real router, because what it promises — back and forward, deep links, unknown
 * values — is the router's behaviour as much as its own.
 */
let exposed: ReturnType<typeof useSettingsSubTab> | null = null

const Page = defineComponent({
  setup() {
    const route = router.currentRoute
    exposed = useSettingsSubTab(() => String(route.value.params.section ?? ''))
    return () => h('p', exposed?.active.value)
  }
})

let router: Router

async function open(path: string): Promise<ReturnType<typeof useSettingsSubTab>> {
  router = createRouter({
    history: createMemoryHistory(),
    routes: [{ path: '/settings/:section', component: Page }]
  })
  await router.push(path)
  await router.isReady()
  render({ render: () => h(Page) }, { global: { plugins: [router] } })
  if (!exposed) throw new Error('the page did not mount')
  return exposed
}

/** `router.back()` resolves before the navigation it starts has landed. */
async function settled(): Promise<void> {
  await new Promise(resolve => setTimeout(resolve, 0))
  await nextTick()
}

describe('useSettingsSubTab', () => {
  it('shows the first tab on the plain page address', async () => {
    const { active, tabs } = await open('/settings/plugins')
    expect(tabs.value.map(tab => tab.value)).toEqual(['installed', 'add', 'updates', 'repositories', 'trust'])
    expect(active.value).toBe('installed')
  })

  it('opens the tab `?tab=` names', async () => {
    const { active } = await open('/settings/plugins?tab=trust')
    expect(active.value).toBe('trust')
  })

  it('falls back to the first tab for a value the page does not have', async () => {
    expect((await open('/settings/plugins?tab=nonsense')).active.value).toBe('installed')
    // A tab of another page is not one of this page's.
    expect((await open('/settings/plugins?tab=collector')).active.value).toBe('installed')
  })

  it('has no tabs on a page without them', async () => {
    const { active, tabs } = await open('/settings/backup?tab=trust')
    expect(tabs.value).toEqual([])
    expect(active.value).toBe('')
  })

  it('puts a chosen tab into the address, and the first one back out of it', async () => {
    const { active } = await open('/settings/security')
    active.value = 'proxy'
    await settled()
    expect(router.currentRoute.value.fullPath).toBe('/settings/security?tab=proxy')
    active.value = 'signin'
    await settled()
    expect(router.currentRoute.value.fullPath).toBe('/settings/security')
  })

  it('keeps the rest of the query and ignores a value the page does not have', async () => {
    const { active } = await open('/settings/network?from=search')
    active.value = 'reconnect'
    await settled()
    expect(router.currentRoute.value.query).toEqual({ from: 'search', tab: 'reconnect' })
    active.value = 'nonsense'
    await settled()
    expect(router.currentRoute.value.query).toEqual({ from: 'search', tab: 'reconnect' })
  })

  it('goes back and forward through the tabs with the browser', async () => {
    const { active } = await open('/settings/system')
    active.value = 'updates'
    await settled()
    active.value = 'retention'
    await settled()
    router.back()
    await settled()
    expect(active.value).toBe('updates')
    router.back()
    await settled()
    expect(active.value).toBe('status')
    router.forward()
    await settled()
    expect(active.value).toBe('updates')
  })

  it('follows a link to another page to that page’s first tab', async () => {
    const { active } = await open('/settings/plugins?tab=trust')
    await router.push('/settings/network')
    await settled()
    expect(active.value).toBe('proxies')
  })
})

describe('subTabItems', () => {
  it('translates the labels, names the slots after the tabs and badges only what has a count', () => {
    const items = subTabItems('plugins', key => `t:${key}`, { installed: 12, updates: undefined })
    expect(items.map(item => [item.value, item.slot, item.label, item.badge])).toEqual([
      ['installed', 'installed', 't:plugins.tabs.installed', 12],
      ['add', 'add', 't:plugins.tabs.add', undefined],
      ['updates', 'updates', 't:plugins.tabs.updates', undefined],
      ['repositories', 'repositories', 't:plugins.tabs.repositories', undefined],
      ['trust', 'trust', 't:plugins.tabs.trust', undefined]
    ])
    expect(items.every(item => !('badge' in item) || item.badge !== undefined)).toBe(true)
  })
})
