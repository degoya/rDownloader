import { useToast } from '@nuxt/ui/composables'
import { ref, type Ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, errorMessage } from '@/api/client'
import type { Category, DownloadPriority, PostprocessLevel } from '@/api/types'
import { useErrorToast } from '@/composables/useErrorToast'
import { isContainerFile } from '@/composables/nzbImportRequest'
import { useFileImportModal, type FileImportEntry } from '@/composables/useNzbImportModal'
import { useCollectorStore } from '@/stores/collector'
import { useNzbImportsStore, type NzbBatchResult, type NzbImportResult } from '@/stores/nzbImports'

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
  const showError = useErrorToast()
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
      const options = { categoryId: input.categoryId, priority: input.priority, passphrase: input.passphrase, enqueue: input.enqueue }
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
        ...await eachFile(torrentEntries, entry => importTorrent(entry, options)),
        ...await eachFile(containerEntries, entry => importContainer(entry, options))
      ]
      reportFileBatch(batch, containers)
    } finally {
      importing.value = false
    }
  }

  /**
   * One result per file, whatever the others did: a single rejection in a `Promise.all` threw away
   * the outcome of every other file of the batch, uploaded or not (WEB-02).
   */
  async function eachFile(
    entries: FileImportEntry[],
    run: (entry: FileImportEntry) => Promise<ContainerImportResult>
  ): Promise<ContainerImportResult[]> {
    const settled = await Promise.allSettled(entries.map(run))
    return settled.map((result, index) => result.status === 'fulfilled'
      ? result.value
      : { status: 'error', name: entries[index]!.file.name, message: errorMessage(errorText(result.reason)) })
  }

  function errorText(reason: unknown): string | undefined {
    return reason instanceof Error ? reason.message : undefined
  }

  function isTorrent(file: File): boolean {
    return /\.torrent$/i.test(file.name)
  }

  /** The container formats the server opens for us; NZBs and torrents have their own endpoints. */
  function isContainer(file: File): boolean {
    return isContainerFile(file.name)
  }

  /** Outcome of one uploaded container, torrent or DLC alike. */
  interface ContainerImportResult {
    status: 'created' | 'duplicate' | 'error'
    name: string
    message?: string
    /** Links a DLC brought in; a torrent reports none. */
    links?: number
    /** NZBs an `.rdlinks` file carried, imported like a dropped NZB (RD-1220-02). */
    nzbs?: number
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
    // Multipart goes through the client too; `openapi-fetch` passes a `FormData` body as it is.
    const response = await api.POST('/api/v1/torrents/import', { body: body as never })
    if (!response.data) {
      return { status: 'error', name: entry.file.name, message: errorMessage(response.error) }
    }
    await collector.refresh()
    return {
      status: torrentResponseIsDuplicate(response.data) ? 'duplicate' : 'created',
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
      showError(t('linkgrabber.files.torrent_failed'), result.message)
    }
  }

  /**
   * A container lands in the LinkGrabber as one package per package it declares. Nothing starts
   * downloading on its own. DLC and CCF are opened by the service configured in the settings;
   * RSDF and plain link lists are read on the server without leaving the machine.
   */
  async function importContainer(
    entry: FileImportEntry,
    options: { categoryId: string | null, priority: DownloadPriority, passphrase?: string | undefined, enqueue?: boolean | undefined }
  ): Promise<ContainerImportResult> {
    const body = new FormData()
    body.append('file', entry.file)
    if (entry.name.trim()) body.append('name', entry.name.trim())
    if (options.categoryId) body.append('category_id', options.categoryId)
    body.append('priority', options.priority)
    // An exported link file (RD-1210-01): its passphrase, and whether to queue once checked.
    if (options.passphrase) body.append('passphrase', options.passphrase)
    if (options.enqueue) body.append('enqueue', 'true')
    const response = await api.POST('/api/v1/containers/import', { body: body as never })
    const name = entry.name.trim() || entry.file.name.replace(/\.[^.]+$/, '')
    if (!response.data) {
      return { status: 'error', name: entry.file.name, message: errorMessage(response.error) }
    }
    const nzbs = listLength(response.data, 'nzb_imports')
    await Promise.all([collector.refresh(), ...(nzbs ? [nzb.refresh()] : [])])
    return { status: 'created', name, links: listLength(response.data, 'candidates'), nzbs }
  }

  function listLength(value: unknown, key: 'candidates' | 'nzb_imports'): number {
    if (typeof value !== 'object' || value === null || !(key in value)) return 0
    const list = (value as Record<string, unknown>)[key]
    return Array.isArray(list) ? list.length : 0
  }

  function reportSingleDlc(result: ContainerImportResult): void {
    if (result.status === 'error') {
      showError(t('linkgrabber.files.container_failed'), result.message)
      return
    }
    const links = t('linkgrabber.files.container_imported_description', { name: result.name, count: result.links ?? 0 }, result.links ?? 0)
    const nzbs = result.nzbs ? ` ${t('linkgrabber.files.container_nzbs', { count: result.nzbs }, result.nzbs)}` : ''
    toast.add({
      title: t('linkgrabber.files.container_imported'),
      description: `${links}${nzbs}`,
      color: 'success',
      icon: 'i-lucide-package-open'
    })
  }

  /** Single-file import: keep the four existing detailed toasts unchanged. */
  function reportSingleImport(result: NzbImportResult): void {
    if (result.status === 'error') {
      showError(t('linkgrabber.nzb.import_failed'), result.message)
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
