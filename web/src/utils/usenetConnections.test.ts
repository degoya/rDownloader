/**
 * RD-1270-21: what the connections at work read as — the package row's tooltip and the hint
 * under the per-file setting.
 */
import { beforeAll, describe, expect, it } from 'vitest'

import type { Download } from '@/api/types'
import { i18n } from '@/i18n'

import {
  connectionsPerFile,
  liveConnectionsHint,
  packageConnections,
  packageConnectionsTitle,
  serverTotals
} from './usenetConnections'

function file(id: string): Download {
  return { id } as unknown as Download
}

const server = (busy: number, limit: number) => ({ id: 'srv', busy, limit })

beforeAll(() => {
  i18n.global.locale.value = 'en'
})

describe('packageConnections', () => {
  it('adds up the files of the package that hold a connection, largest first', () => {
    const connections = packageConnections([file('a'), file('b'), file('c')], { a: 3, b: 7, other: 4 }, [server(14, 20)])
    expect(connections).toEqual({ busy: 10, perFile: [7, 3], serverBusy: 14, serverLimit: 20 })
  })

  it('is null while none of its files holds one', () => {
    expect(packageConnections([file('a')], { other: 5 }, [server(5, 10)])).toBeNull()
  })

  it('sums every server of the pool', () => {
    expect(serverTotals([server(10, 10), { id: 'backup', busy: 2, limit: 8 }])).toEqual({ busy: 12, limit: 18 })
    expect(connectionsPerFile(['a', 'b'], { a: 0, b: 2 })).toEqual([2])
  })
})

describe('packageConnectionsTitle', () => {
  it('says how the files share the connections and that the servers allow no more', () => {
    const title = packageConnectionsTitle({ busy: 10, perFile: [5, 5], serverBusy: 10, serverLimit: 10 })
    expect(title).toBe('10 connections: 2 files at once, 5 each. All 10 connections the servers allow are busy.')
  })

  it('names the split when the files hold different numbers, and the room left', () => {
    const title = packageConnectionsTitle({ busy: 8, perFile: [5, 3], serverBusy: 8, serverLimit: 20 })
    expect(title).toBe('8 connections: 2 files at once (5 + 3). 8 of the 20 connections the servers allow are busy.')
  })

  it('reads one file and one connection in the singular', () => {
    const title = packageConnectionsTitle({ busy: 1, perFile: [1], serverBusy: 1, serverLimit: 0 })
    expect(title).toBe('1 connection: 1 file')
  })
})

describe('liveConnectionsHint', () => {
  it('says how many files share how many of the connections right now', () => {
    expect(liveConnectionsHint({ a: 5, b: 5 }, [server(10, 10)])).toBe('Right now 2 files with 5 of 10 connections each.')
    expect(liveConnectionsHint({ a: 6, b: 4 }, [server(10, 10)])).toBe('Right now 2 files with 10 of 10 connections together (6 + 4).')
    expect(liveConnectionsHint({ a: 4 }, [server(4, 10)])).toBe('Right now 1 file with 4 of 10 connections.')
  })

  it('is null while nothing runs', () => {
    expect(liveConnectionsHint({}, [server(0, 10)])).toBeNull()
  })
})
