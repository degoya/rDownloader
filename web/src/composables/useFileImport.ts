import { useToast } from '@nuxt/ui/composables'
import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import type { Category, DownloadPriority, PostprocessLevel } from '@/api/types'
import { useFileImportModal, type FileImportEntry } from '@/composables/useNzbImportModal'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore, type NzbBatchResult, type NzbImportResult } from '@/stores/nzbImports'
import { withBase } from '@/basePath'

/**
 * Taking dropped or chosen files into the LinkGrabber (RD-106-11).
 *
 * Three formats arrive through the same file picker and leave through three different endpoints:
 * an NZB goes to the import store, a torrent and a container each to their own upload. Eleven
 * functions used to sit in `LinkGrabberView` for this, six of them doing nothing but turning a
 * result into a toast, and they were most of what made that file twice the length the rest of
 * this codebase keeps to.
 *
 * The reporting is deliberately asymmetric and stays that way: one file gets the detailed toast
 * that names it and says exactly what happened to it, while a batch gets a single counted
 * summary. Four toasts per file is not a report, it is a wall.
 */
export function useFileImport(categories: Ref<Category[]>) {
  const { t } = useI18n()
  const toast = useToast()
  const nzb = useNzbImportsStore()
  const collector = useCollectorStore()
  const openFileImport = useFileImportModal()

  /** True while an import is in flight, for the button that started it. */
  const importing = ref(false)

  async function importFiles(initialFiles?: File[]): Promise<void> {
    const input = await openFileImport(categories.value, initialFiles)
    if (!input) return
    importing.value = true
    try {
      const options = { categoryId: input.categoryId, priority: input.priority }
      if (input.entries.length === 1) {
        const entry = input.entries[0]!
        if (isContainer(entry.file)) {
          reportSingleDlc(await importContainer(entry, options))
        } else if (isTorrent(entry.file)) {
          reportSingleTorrent(await importTorrent(entry, options))
        } else {
          const result = await nzb.importNzb(entry.file, { name: entry.name, ...options })
          reportSingleImport(result)
        }
        return
      }
      const nzbEntries = input.entries.filter(entry => !isTorrent(entry.file) && !isContainer(entry.file))
      const torrentEntries = input.entries.filter(entry => isTorrent(entry.file))
      const containerEntries = input.entries.filter(entry => isContainer(entry.file))
      const batch = nzbEntries.length
        ? await nzb.importMany(nzbEntries, options)
        : { created: [], duplicates: [], errors: [] }
      const containers = [
        ...await Promise.all(torrentEntries.map(entry => importTorrent(entry, options))),
        ...await Promise.all(containerEntries.map(entry => importContainer(entry, options)))
      ]
      reportFileBatch(batch, containers)
    } finally {
      importing.value = false
    }
  }

  function isTorrent(file: File): boolean {
    return /\.torrent$/i.test(file.name)
  }

  /**
   * The container formats the server opens for us. Torrents and NZBs are handled by their own
   * endpoints, so they are deliberately absent here.
   */
  function isContainer(file: File): boolean {
    return /\.(?:dlc|ccf|rsdf|txt|text)$/i.test(file.name)
  }

  /** Outcome of one uploaded container, torrent or DLC alike. */
  interface ContainerImportResult {
    status: 'created' | 'duplicate' | 'error'
    name: string
    message?: string
    /** Links a DLC brought in; a torrent reports none. */
    links?: number
  }

  /** A torrent import creates a reviewable collector package; it does not start a download. */
  async function importTorrent(
    entry: FileImportEntry,
    options: { categoryId: string | null, priority: DownloadPriority }
  ): Promise<ContainerImportResult> {
    const body = new FormData()
    body.append('file', entry.file)
    if (entry.name.trim()) body.append('name', entry.name.trim())
    if (options.categoryId) body.append('category_id', options.categoryId)
    body.append('priority', options.priority)
    const response = await fetch(withBase('/api/v1/torrents/import'), {
      method: 'POST',
      credentials: 'include',
      body
    })
    const payload: unknown = await response.json().catch(() => null)
    if (!response.ok) {
      return { status: 'error', name: entry.file.name, message: errorMessage(payload) }
    }
    await collector.refresh()
    return {
      status: torrentResponseIsDuplicate(payload) ? 'duplicate' : 'created',
      name: entry.name.trim() || entry.file.name.replace(/\.torrent$/i, '')
    }
  }

  function torrentResponseIsDuplicate(value: unknown): boolean {
    if (typeof value !== 'object' || value === null || !('candidates' in value)) return false
    const candidates = (value as { candidates?: unknown }).candidates
    return Array.isArray(candidates) && candidates.some(candidate =>
      typeof candidate === 'object' && candidate !== null && 'state' in candidate && candidate.state === 'duplicate')
  }

  function reportSingleTorrent(result: ContainerImportResult): void {
    if (result.status === 'created') {
      toast.add({ title: t('linkgrabber.files.torrent_imported'), description: t('linkgrabber.files.torrent_imported_description', { name: result.name }), color: 'success', icon: 'i-lucide-magnet' })
    } else if (result.status === 'duplicate') {
      toast.add({ title: t('linkgrabber.files.torrent_duplicate'), description: t('linkgrabber.files.torrent_duplicate_description', { name: result.name }), color: 'warning', icon: 'i-lucide-copy-check' })
    } else {
      toast.add({ title: t('linkgrabber.files.torrent_failed'), ...(result.message ? { description: result.message } : {}), color: 'error', icon: 'i-lucide-circle-alert' })
    }
  }

  /**
   * A container lands in the LinkGrabber as one package per package it declares. Nothing starts
   * downloading on its own. DLC and CCF are opened by the service configured in the settings;
   * RSDF and plain link lists are read on the server without leaving the machine.
   */
  async function importContainer(
    entry: FileImportEntry,
    options: { categoryId: string | null, priority: DownloadPriority }
  ): Promise<ContainerImportResult> {
    const body = new FormData()
    body.append('file', entry.file)
    if (entry.name.trim()) body.append('name', entry.name.trim())
    if (options.categoryId) body.append('category_id', options.categoryId)
    body.append('priority', options.priority)
    const response = await fetch(withBase('/api/v1/containers/import'), {
      method: 'POST',
      credentials: 'include',
      body
    })
    const payload: unknown = await response.json().catch(() => null)
    const name = entry.name.trim() || entry.file.name.replace(/\.[^.]+$/, '')
    if (!response.ok) {
      return { status: 'error', name: entry.file.name, message: errorMessage(payload) }
    }
    await collector.refresh()
    return { status: 'created', name, links: dlcLinkCount(payload) }
  }

  function dlcLinkCount(value: unknown): number {
    if (typeof value !== 'object' || value === null || !('candidates' in value)) return 0
    const candidates = (value as { candidates?: unknown }).candidates
    return Array.isArray(candidates) ? candidates.length : 0
  }

  function reportSingleDlc(result: ContainerImportResult): void {
    if (result.status === 'error') {
      toast.add({ title: t('linkgrabber.files.container_failed'), ...(result.message ? { description: result.message } : {}), color: 'error', icon: 'i-lucide-circle-alert' })
      return
    }
    toast.add({
      title: t('linkgrabber.files.container_imported'),
      description: t('linkgrabber.files.container_imported_description', { name: result.name, count: result.links ?? 0 }),
      color: 'success',
      icon: 'i-lucide-package-open'
    })
  }

  /** Single-file import: keep the four existing detailed toasts unchanged. */
  function reportSingleImport(result: NzbImportResult): void {
    if (result.status === 'error') {
      toast.add({ title: t('linkgrabber.nzb.import_failed'), description: result.message, color: 'error', icon: 'i-lucide-circle-alert' })
      return
    }
    const name = result.item.name
    if (result.status === 'created') {
      toast.add({ title: t('linkgrabber.nzb.toast.imported'), description: t('linkgrabber.nzb.toast.imported_description', { name }), color: 'success', icon: 'i-lucide-file-up' })
      return
    }
    if (result.item.state === 'enqueued') {
      toast.add({ title: t('linkgrabber.nzb.toast.duplicate_enqueued'), description: t('linkgrabber.nzb.toast.duplicate_enqueued_description', { name }), color: 'warning', icon: 'i-lucide-copy-check' })
      return
    }
    toast.add({ title: t('linkgrabber.nzb.toast.duplicate'), description: t('linkgrabber.nzb.toast.duplicate_description', { name }), color: 'info', icon: 'i-lucide-copy' })
  }

  function reportFileBatch(batch: NzbBatchResult, containers: ContainerImportResult[]): void {
    const created = batch.created.length + containers.filter(result => result.status === 'created').length
    const duplicates = batch.duplicates.length + containers.filter(result => result.status === 'duplicate').length
    const containerErrors = containers.filter(result => result.status === 'error')
    const errors = batch.errors.length + containerErrors.length
    const color = errors ? 'error' : duplicates ? 'warning' : 'success'
    const description = t('linkgrabber.files.batch_description', { created, duplicates, errors })
    const firstError = batch.errors[0]?.message ?? containerErrors[0]?.message
    toast.add({
      title: t('linkgrabber.files.batch'),
      description: firstError ? `${description} ${firstError}` : description,
      color,
      icon: errors ? 'i-lucide-circle-alert' : duplicates ? 'i-lucide-copy-check' : 'i-lucide-files'
    })
  }

  return { importing, importFiles }
}
