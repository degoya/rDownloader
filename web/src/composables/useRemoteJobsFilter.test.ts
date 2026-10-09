/**
 * The remote jobs list's filter in the address (RD-1200-01): read on a reload, written as it
 * changes, plain while it filters nothing, and an unknown state is no filter.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { effectScope, nextTick, type EffectScope } from 'vue'

import { useRemoteJobsFilter } from './useRemoteJobsFilter'

const router = vi.hoisted(() => ({
  route: null as unknown as { path: string, hash: string, query: Record<string, string | undefined> },
  replace: null as unknown as ReturnType<typeof vi.fn>
}))
vi.mock('vue-router', async () => {
  const { reactive } = await import('vue')
  router.route = reactive({ path: '/remote-jobs', hash: '', query: {} })
  router.replace = vi.fn(async ({ query }: { query: Record<string, string | undefined> }) => {
    router.route.query = { ...query }
  })
  return { useRoute: () => router.route, useRouter: () => ({ replace: router.replace }) }
})

let scope: EffectScope | null = null

function start(query: Record<string, string>) {
  router.route.query = { ...query }
  router.replace.mockClear()
  scope = effectScope()
  return scope.run(() => useRemoteJobsFilter())!
}

afterEach(() => scope?.stop())

describe('useRemoteJobsFilter', () => {
  it('opens on what the address names, which is what a reload keeps', () => {
    const filter = start({ provider: 'TorBox', state: 'failed', tab: 'x' })
    expect(filter.value).toEqual({ provider: 'torbox', state: 'failed' })
    expect(router.replace).not.toHaveBeenCalled()
  })

  it('writes a change into the address by replacing it, and leaves other parameters alone', async () => {
    const filter = start({ tab: 'x' })
    expect(filter.value).toEqual({ provider: 'all', state: 'all' })
    filter.value.provider = 'realdebrid'
    await nextTick()
    expect(router.replace).toHaveBeenLastCalledWith({ path: '/remote-jobs', query: { tab: 'x', provider: 'realdebrid' }, hash: '' })
    filter.value.state = 'ready'
    await nextTick()
    expect(router.route.query).toEqual({ tab: 'x', provider: 'realdebrid', state: 'ready' })
    filter.value = { provider: 'all', state: 'all' }
    await nextTick()
    expect(router.route.query).toEqual({ tab: 'x' })
  })

  it('takes an unknown state as no filter', () => {
    expect(start({ state: 'exploded' }).value).toEqual({ provider: 'all', state: 'all' })
  })

  it('follows a link that changes the address while the list is open', async () => {
    const filter = start({})
    router.route.query = { state: 'awaiting_choice' }
    await nextTick()
    expect(filter.value).toEqual({ provider: 'all', state: 'awaiting_choice' })
  })
})
