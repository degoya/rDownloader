import type { Download, DownloadRates } from '@/api/types'
import { i18n } from '@/i18n'

/**
 * The Usenet connections at work, as the rates answer reports them (RD-1270-21).
 *
 * A Usenet package used to say "2 active" while its two files kept all ten of the provider's
 * connections busy, five each. Files share the servers' connections; the server counts one
 * shared by two files half for each, so the files never add up to more than the servers have
 * busy.
 */

/** One server of the pool in force: connections busy now, and the most it opens. */
export type UsenetServerLoad = DownloadRates['usenet_servers'][number]

/** A package's share of the connections, with the servers' state beside it. */
export interface PackageConnections {
  /** Connections working for the package's files. */
  busy: number
  /** Each of its running files' connections, largest first. */
  perFile: number[]
  /** Every server together: busy, and allowed. */
  serverBusy: number
  serverLimit: number
}

/** All servers together: the connections busy and the connections allowed. */
export function serverTotals(servers: UsenetServerLoad[]): { busy: number, limit: number } {
  return {
    busy: servers.reduce((sum, server) => sum + server.busy, 0),
    limit: servers.reduce((sum, server) => sum + server.limit, 0)
  }
}

/** Each file's connections from `connections` (by download id), largest first; idle files left out. */
export function connectionsPerFile(ids: string[], connections: Record<string, number>): number[] {
  return ids.map(id => connections[id] ?? 0).filter(count => count > 0).sort((a, b) => b - a)
}

/** The package's connections, or `null` while none of its files holds one. */
export function packageConnections(
  downloads: Download[],
  connections: Record<string, number>,
  servers: UsenetServerLoad[]
): PackageConnections | null {
  const perFile = connectionsPerFile(downloads.map(item => item.id), connections)
  if (!perFile.length) return null
  const totals = serverTotals(servers)
  return {
    busy: perFile.reduce((sum, count) => sum + count, 0),
    perFile,
    serverBusy: totals.busy,
    serverLimit: totals.limit
  }
}

/** "5 + 3": the split when the files do not hold the same number. */
function splitLabel(perFile: number[]): string {
  return perFile.join(' + ')
}

/** "10 connections: 2 files at once, 5 each", and the servers' state after it. */
export function packageConnectionsTitle(connections: PackageConnections): string {
  const { t } = i18n.global
  const count = t('downloads.package.connections_count', { count: connections.busy }, connections.busy)
  const files = connections.perFile.length
  const even = connections.perFile.every(value => value === connections.perFile[0])
  const summary = even
    ? t('downloads.package.connections_even', { connections: count, files, per: connections.perFile[0] }, files)
    : t('downloads.package.connections_mixed', { connections: count, files, split: splitLabel(connections.perFile) })
  if (connections.serverLimit <= 0) return summary
  const state = connections.serverBusy >= connections.serverLimit
    ? t('downloads.package.connections_full', { limit: connections.serverLimit })
    : t('downloads.package.connections_room', { busy: connections.serverBusy, limit: connections.serverLimit })
  return `${summary}. ${state}`
}

/**
 * What the per-file setting's hint says while files run: "Right now 2 files with 5 of 10
 * connections each". `null` while nothing holds a connection.
 */
export function liveConnectionsHint(connections: Record<string, number>, servers: UsenetServerLoad[]): string | null {
  const perFile = connectionsPerFile(Object.keys(connections), connections)
  if (!perFile.length) return null
  const { t } = i18n.global
  const { limit } = serverTotals(servers)
  const files = perFile.length
  if (perFile.every(value => value === perFile[0])) {
    return t('settings.nntp_connections.live_even', { files, per: perFile[0], limit }, files)
  }
  const busy = perFile.reduce((sum, count) => sum + count, 0)
  return t('settings.nntp_connections.live_mixed', { files, busy, limit, split: splitLabel(perFile) })
}
