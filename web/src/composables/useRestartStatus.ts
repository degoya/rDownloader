import { onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { fetchRestartStatus, requestRestart, type RestartStatus } from '@/api/restart'
import { useConfirm } from '@/composables/useConfirm'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useErrorToast } from '@/composables/useErrorToast'
import { onEventStreamOpened } from '@/composables/useEventStream'
import { translateServerMessage, type ServerMessage } from '@/i18n/server'

/**
 * One restart status for the whole page (RD-1240-32): the sidebar badge, the notice on the
 * Updates page and the "Restart now" beside a plugin's answer read the same answer.
 *
 * Read on start, when the stream says the plugins changed, when the stream opens again (a service
 * that restarted by itself), and when the window gets the focus back — never on a timer while
 * nothing happens. After a restart this page asked for, the status is read every two seconds: an
 * unanswered read is the service going down, an answer from a process that started at another
 * moment is the new one, and the page reloads into it.
 */
const status = ref<RestartStatus | null>(null)
/** The request is on its way. */
const starting = ref(false)
/** The service took the restart this page asked for; the page follows it. */
const restarting = ref(false)
/** The service does not answer: it is down between the two processes. */
const reconnecting = ref(false)
/** It did not come back within {@link RESTART_FOLLOW_LIMIT_MS}. */
const lost = ref(false)
/** Why the last request was refused. */
const failure = ref<ServerMessage | null>(null)
let following = false

export const RESTART_FOLLOW_INTERVAL_MS = 2000
/** Longer than a stop that saves every running transfer and a start that opens the database. */
export const RESTART_FOLLOW_LIMIT_MS = 5 * 60 * 1000

/** The events after which a restart may be pending — or no longer is. */
export const RESTART_EVENTS = ['plugin.changed', 'plugin_trust.changed', 'plugin_catalog.changed'] as const

/** The reload into the restarted service; an object so a test can replace it (jsdom's cannot be). */
export const pageReload = { run: (): void => { window.location.reload() } }

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms))

async function load(): Promise<void> {
  if (following) return
  const answer = await fetchRestartStatus()
  if (answer.ok) status.value = answer.data
}

/** Asks for the restart and follows it; `false` when the service refused, see `failure`. */
async function request(allowActive = false): Promise<boolean> {
  if (starting.value || following) return false
  failure.value = null
  starting.value = true
  try {
    if (!status.value) await load()
    const before = status.value?.started_at ?? null
    const answer = await requestRestart(allowActive)
    if (!answer.ok) {
      failure.value = answer.message ?? {}
      return false
    }
    void follow(before)
    return true
  } finally {
    starting.value = false
  }
}

/**
 * Reads the status until another process answers, then reloads. Without a known start time (the
 * status could not be read before), the first answer after an unanswered read counts as the new
 * process.
 */
async function follow(before: string | null): Promise<void> {
  following = true
  restarting.value = true
  lost.value = false
  let wentDown = false
  const deadline = Date.now() + RESTART_FOLLOW_LIMIT_MS
  try {
    while (Date.now() < deadline) {
      await pause(RESTART_FOLLOW_INTERVAL_MS)
      const answer = await fetchRestartStatus()
      if (!answer.ok) {
        wentDown = true
        reconnecting.value = true
        continue
      }
      status.value = answer.data
      if (before !== null ? answer.data.started_at !== before : wentDown) {
        pageReload.run()
        return
      }
      reconnecting.value = false
    }
    lost.value = true
    restarting.value = false
  } finally {
    following = false
    reconnecting.value = false
  }
}

export function useRestartStatus() {
  return { status, starting, restarting, reconnecting, lost, failure, load, request }
}

/**
 * The status kept current for the whole page, from the component that is always mounted (the
 * sidebar): read on mount, after plugin events, when the stream opens again and on focus.
 */
export function useRestartRefresh(): void {
  const refresh = () => { void load() }
  useDebouncedEventRefresh(RESTART_EVENTS, load)
  let stopOpened: (() => void) | null = null
  onMounted(() => {
    void load()
    stopOpened = onEventStreamOpened(refresh)
    window.addEventListener('focus', refresh)
  })
  onUnmounted(() => {
    stopOpened?.()
    window.removeEventListener('focus', refresh)
  })
}

/**
 * "Restart now" with its one question: when downloads are running, the service refuses with
 * their count, and restarting anyway is the person's decision — they are saved by the stop and
 * continue after it. Any other refusal is said in a toast. `action()` is the same as a button
 * for a notice's `actions`.
 */
export function useRestartAction() {
  const { t } = useI18n()
  const confirm = useConfirm()
  const showError = useErrorToast()

  async function restartNow(): Promise<boolean> {
    if (await request()) return true
    const refusal = failure.value
    if (refusal?.code === 'restart.transfers_active') {
      const agreed = await confirm({
        title: t('system.restart.confirm_title'),
        description: translateServerMessage(refusal),
        confirmLabel: t('system.restart.anyway'),
        confirmIcon: 'i-lucide-rotate-ccw'
      })
      if (!agreed) return false
      if (await request(true)) return true
    }
    if (failure.value) showError(t('system.restart.failed_title'), translateServerMessage(failure.value))
    return false
  }

  function action() {
    return {
      label: t('system.restart.now'),
      icon: 'i-lucide-rotate-ccw',
      color: 'warning' as const,
      variant: 'outline' as const,
      loading: starting.value,
      disabled: restarting.value || status.value?.can_restart === false,
      onClick: () => { void restartNow() }
    }
  }

  return { restartNow, action }
}
