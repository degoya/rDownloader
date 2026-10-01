import { ref } from 'vue'

import {
  INSTALL_ENDED,
  checkForUpdates,
  downloadUpdate,
  fetchUpdateStatus,
  installUpdate,
  type UpdateStatus
} from '@/api/updates'
import type { ServerMessage } from '@/i18n/server'

/**
 * One update status for the whole page (RD-180-01): the settings card and the sidebar notice
 * read the same answer, so a "check now" on the card shows in the sidebar at once.
 *
 * An install (RD-180-02) is followed from here too: the status is read every few seconds until
 * the install ended, through the restart, when the service does not answer for a while. It has
 * ended when the status says so — done, rolled back or failed with its reason — or when the
 * service answers as the version it was installing. So is the background download of the offered
 * version, every second until it is ready or failed (owner, 2026-10-01).
 */
const status = ref<UpdateStatus | null>(null)
const checking = ref(false)
const failure = ref<ServerMessage | null>(null)
/** Why the install request was refused. */
const installFailure = ref<ServerMessage | null>(null)
/** Why the download request was refused. */
const downloadFailure = ref<ServerMessage | null>(null)
/** The service does not answer: it is restarting. */
const reconnecting = ref(false)
/** It did not come back within {@link FOLLOW_LIMIT_MS}. */
const lost = ref(false)
/** An install this page followed, so its outcome is shown even when nothing is offered any more. */
const followed = ref(false)
let following = false
let downloading = false

export const FOLLOW_INTERVAL_MS = 2000
/**
 * Longer than the updater's worst case: two minutes for the stop, the switch, a minute and a half
 * for the new version's health, as long again for the old one after a roll-back.
 */
export const FOLLOW_LIMIT_MS = 6 * 60 * 1000
export const DOWNLOAD_INTERVAL_MS = 1000

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

function running(value: UpdateStatus | null): boolean {
  const state = value?.install?.state
  return state !== undefined && !INSTALL_ENDED.includes(state)
}

async function load(): Promise<void> {
  const answer = await fetchUpdateStatus()
  if (!answer.ok) return
  status.value = answer.data
  // A page opened, or reloaded, while an install or a download runs follows it.
  if (running(answer.data)) void follow()
  if (answer.data.download?.state === 'downloading') void followDownload()
}

/** Checks now; `true` when the check ran, whatever it found. */
async function check(): Promise<boolean> {
  checking.value = true
  failure.value = null
  try {
    const answer = await checkForUpdates()
    if (answer.ok) {
      status.value = answer.data
      return true
    }
    failure.value = answer.message ?? { code: 'update.failed' }
    return false
  } finally {
    checking.value = false
  }
}

/** Starts the install and follows it; `false` when the service refused, see `installFailure`. */
async function install(allowActive = false): Promise<boolean> {
  installFailure.value = null
  lost.value = false
  const answer = await installUpdate(allowActive)
  if (!answer.ok) {
    installFailure.value = answer.message ?? { code: 'update.failed' }
    return false
  }
  if (status.value) status.value = { ...status.value, install: answer.data }
  void follow()
  return true
}

/** Downloads the offered version in the background and follows it; `false` when refused. */
async function download(): Promise<boolean> {
  downloadFailure.value = null
  const answer = await downloadUpdate()
  if (!answer.ok) {
    downloadFailure.value = answer.message ?? { code: 'update.failed' }
    return false
  }
  if (status.value) status.value = { ...status.value, download: answer.data }
  void followDownload()
  return true
}

/** Reads the status until the download is ready or failed. */
async function followDownload(): Promise<void> {
  if (downloading) return
  downloading = true
  const deadline = Date.now() + FOLLOW_LIMIT_MS
  try {
    while (status.value?.download?.state === 'downloading' && Date.now() < deadline) {
      await pause(DOWNLOAD_INTERVAL_MS)
      const answer = await fetchUpdateStatus()
      if (answer.ok) status.value = answer.data
    }
  } finally {
    downloading = false
  }
}

/**
 * The status as the follow reads it: a service that answers as the version it was installing has
 * installed it, even when it tells nothing about an install any more.
 */
function settled(answer: UpdateStatus, target: string | undefined, from: string | undefined): UpdateStatus {
  if (answer.install || !target || answer.current_version !== target) return answer
  const now = new Date().toISOString()
  return {
    ...answer,
    install: { state: 'done', from_version: from ?? '', target_version: target, reason: null, started_at: now, updated_at: now }
  }
}

/** Reads the status until the install ended; an unanswered read is the restart. */
async function follow(): Promise<void> {
  if (following) return
  following = true
  followed.value = true
  const target = status.value?.install?.target_version
  const from = status.value?.install?.from_version ?? status.value?.current_version
  const deadline = Date.now() + FOLLOW_LIMIT_MS
  try {
    while (Date.now() < deadline) {
      await pause(FOLLOW_INTERVAL_MS)
      const answer = await fetchUpdateStatus()
      if (!answer.ok) {
        reconnecting.value = true
        continue
      }
      reconnecting.value = false
      status.value = settled(answer.data, target, from)
      if (!running(status.value)) return
    }
    lost.value = true
  } finally {
    following = false
    reconnecting.value = false
  }
}

export function useUpdateStatus() {
  return {
    status, checking, failure, installFailure, downloadFailure, reconnecting, lost, followed,
    load, check, install, download
  }
}
