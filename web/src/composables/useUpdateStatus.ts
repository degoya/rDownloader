import { ref } from 'vue'

import {
  INSTALL_ENDED,
  checkForUpdates,
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
 * the install ended, through the restart, when the service does not answer for a while.
 */
const status = ref<UpdateStatus | null>(null)
const checking = ref(false)
const failure = ref<ServerMessage | null>(null)
/** Why the install request was refused. */
const installFailure = ref<ServerMessage | null>(null)
/** The service does not answer: it is restarting. */
const reconnecting = ref(false)
/** It did not come back within {@link FOLLOW_LIMIT_MS}. */
const lost = ref(false)
/** An install this page followed, so its outcome is shown even when nothing is offered any more. */
const followed = ref(false)
let following = false

export const FOLLOW_INTERVAL_MS = 2000
export const FOLLOW_LIMIT_MS = 5 * 60 * 1000

function running(value: UpdateStatus | null): boolean {
  const state = value?.install?.state
  return state !== undefined && !INSTALL_ENDED.includes(state)
}

async function load(): Promise<void> {
  const answer = await fetchUpdateStatus()
  if (!answer.ok) return
  status.value = answer.data
  // A page opened, or reloaded, while an install runs follows it.
  if (running(answer.data)) void follow()
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

/** Reads the status until the install ended; an unanswered read is the restart. */
async function follow(): Promise<void> {
  if (following) return
  following = true
  followed.value = true
  const deadline = Date.now() + FOLLOW_LIMIT_MS
  try {
    while (Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, FOLLOW_INTERVAL_MS))
      const answer = await fetchUpdateStatus()
      if (!answer.ok) {
        reconnecting.value = true
        continue
      }
      reconnecting.value = false
      status.value = answer.data
      if (!running(answer.data)) return
    }
    lost.value = true
  } finally {
    following = false
    reconnecting.value = false
  }
}

export function useUpdateStatus() {
  return { status, checking, failure, installFailure, reconnecting, lost, followed, load, check, install }
}
