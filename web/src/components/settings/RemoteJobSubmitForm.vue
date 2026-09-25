<script setup lang="ts">
/**
 * Handing a magnet, or any number of `.torrent`/`.nzb` files, to one provider account.
 *
 * Split out of `SettingsRemoteJobsCard` when it learned to take several files (RD-120-51); the
 * card keeps the list of jobs, this keeps what starts one.
 *
 * **Several files are several requests, one after another.** Each file becomes its own job
 * through the same `POST /api/v1/accounts/{id}/remote-jobs` a single file always used, so every
 * limit that route enforces -- the 16 MiB ceiling, the decoder, the duplicate lock that answers
 * `already_running` before anything leaves the machine -- applies to each file exactly as it
 * did to one, with nothing to keep in step. A batch endpoint was not built: it would have had
 * to report per file anyway, and one request carrying all of them would put up to n × 16 MiB
 * of base64 into a single body the server has to hold. Sequential rather than parallel, because
 * the jobs are charged to one account and the provider sees them in the order they were listed.
 * A file that fails keeps its reason on its own row and the next one is sent regardless.
 */
import { computed, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Account, RemoteJob } from '@/api/types'
import DataState from '@/components/DataState.vue'
import { claimFileDrops } from '@/composables/nzbImportRequest'
import { formatBytes } from '@/utils/format'

const props = defineProps<{
  accounts: Account[]
  /** True while the page is still reading the accounts themselves. */
  accountsLoading?: boolean
}>()
const emit = defineEmits<{ submitted: [RemoteJob], message: [string], error: [string] }>()

const { t } = useI18n()

/** The server's own ceiling for a remote job's container; it refuses more as `container.too_large`. */
const MAX_CONTAINER_MIB = 16
/** What a remote job takes as a file. Anything else is refused on its row, never read. */
const CONTAINER_SUFFIX = /\.(?:torrent|nzb)$/i

type FileState = 'waiting' | 'sending' | 'started' | 'already_running' | 'failed' | 'refused'
interface QueuedFile { key: string, file: File, state: FileState, reason: string | null }

const FILE_STATE_COLORS: Record<FileState, 'neutral' | 'primary' | 'success' | 'error' | 'warning'> = {
  waiting: 'neutral',
  sending: 'primary',
  started: 'success',
  already_running: 'neutral',
  failed: 'error',
  refused: 'warning'
}

const accountId = ref('')
const magnet = ref('')
const files = ref<QueuedFile[]>([])
const fileInput = ref<HTMLInputElement | null>(null)
const submitting = ref(false)
const dragging = ref(false)

/**
 * The provider slugs an installed `remote-job` plugin runs jobs on; `null` until asked.
 *
 * Read from the server, which reads it from the installed manifests. There is deliberately no
 * list of providers here: one exists exactly as long as its plugin does (RD-120-23).
 */
const jobProviders = ref<string[] | null>(null)
const providersFailed = ref(false)

/**
 * Still waiting for either half of the answer. The picker is not drawn meanwhile: an empty
 * selection says nothing, and on a first open after a start this used to be all the page
 * showed while the server compiled every plugin (RD-120-51).
 */
const pending = computed(() => !providersFailed.value && (jobProviders.value === null || Boolean(props.accountsLoading)))

const accountItems = computed(() => {
  const providers = jobProviders.value
  if (!providers) return []
  return props.accounts
    .filter(account => providers.includes(account.provider.toLowerCase()))
    .map(account => ({ label: `${account.label} · ${account.provider}`, value: account.id }))
})
/** Nothing to offer, and the answer was read rather than still being waited for. */
const noUsableAccount = computed(() => !pending.value && jobProviders.value !== null && accountItems.value.length === 0)
const showForm = computed(() => !providersFailed.value && !pending.value && !noUsableAccount.value)

/** Rows that a press of the button would send: new ones, and ones the server refused before. */
const sendable = computed(() => files.value.filter(row => row.state === 'waiting' || row.state === 'failed'))
const canSubmit = computed(() =>
  Boolean(accountId.value)
  && !submitting.value
  && (files.value.length ? sendable.value.length > 0 : magnet.value.trim().length > 0)
)

/**
 * Lists files, checking each on its own. A wrong type or an oversized file is listed with its
 * reason and never read; a file already listed is not listed twice.
 */
function addFiles(list: File[]): void {
  // Rows that are done make room: the new selection is what the reader is looking at now.
  const kept = files.value.filter(row => row.state !== 'started' && row.state !== 'already_running')
  for (const file of list) {
    const key = `${file.name}:${file.size}:${file.lastModified}`
    if (kept.some(row => row.key === key)) continue
    let reason: string | null = null
    if (!CONTAINER_SUFFIX.test(file.name)) reason = t('remote_jobs.files.wrong_type')
    else if (file.size > MAX_CONTAINER_MIB * 1024 * 1024) reason = t('remote_jobs.submit.file_too_large', { max_mib: MAX_CONTAINER_MIB })
    kept.push({ key, file, state: reason ? 'refused' : 'waiting', reason })
  }
  files.value = kept
  if (files.value.length) magnet.value = ''
}

function pickFiles(event: Event): void {
  const input = event.target as HTMLInputElement
  addFiles(Array.from(input.files ?? []))
  input.value = ''
}

function drop(event: DragEvent): void {
  dragging.value = false
  addFiles(Array.from(event.dataTransfer?.files ?? []))
}

function removeFile(key: string): void {
  files.value = files.value.filter(row => row.key !== key)
}

/** The file as base64, without the data-URL prefix the reader puts in front of it. */
function base64Of(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result).slice(String(reader.result).indexOf(',') + 1))
    reader.onerror = () => reject(reader.error ?? new Error('the file could not be read'))
    reader.readAsDataURL(file)
  })
}

function setState(row: QueuedFile, state: FileState, reason: string | null = null): void {
  files.value = files.value.map(existing => (existing.key === row.key ? { ...existing, state, reason } : existing))
}

/** One file, one request. Answers the state the row ended in. */
async function sendFile(row: QueuedFile, account: string): Promise<FileState> {
  setState(row, 'sending')
  let container: string
  try {
    container = await base64Of(row.file)
  } catch {
    setState(row, 'failed', t('remote_jobs.files.unreadable'))
    return 'failed'
  }
  const response = await api.POST('/api/v1/accounts/{id}/remote-jobs', {
    params: { path: { id: account } },
    body: { container }
  })
  if (!response.data) {
    setState(row, 'failed', responseError(response))
    return 'failed'
  }
  emit('submitted', response.data.job)
  const state = response.data.already_running ? 'already_running' : 'started'
  setState(row, state)
  return state
}

async function submitFiles(): Promise<void> {
  const account = accountId.value
  const queue = [...sendable.value]
  const outcomes: FileState[] = []
  for (const row of queue) outcomes.push(await sendFile(row, account))
  const failed = outcomes.filter(state => state === 'failed').length
  if (queue.length === 1) {
    // One file reads as one job always did, the server's own reason included.
    const only = files.value.find(row => row.key === queue[0]?.key)
    if (failed) return void emit('error', only?.reason ?? '')
    return void emit('message', t(outcomes[0] === 'already_running' ? 'remote_jobs.submit.already_running' : 'remote_jobs.submit.started'))
  }
  const summary = { done: queue.length - failed, total: queue.length, failed }
  if (failed) emit('error', t('remote_jobs.files.summary_failed', summary))
  else emit('message', t('remote_jobs.files.summary', summary))
}

async function submitMagnet(): Promise<void> {
  const response = await api.POST('/api/v1/accounts/{id}/remote-jobs', {
    params: { path: { id: accountId.value } },
    body: { magnet: magnet.value.trim() }
  })
  if (!response.data) return void emit('error', responseError(response))
  magnet.value = ''
  emit('submitted', response.data.job)
  emit('message', t(response.data.already_running ? 'remote_jobs.submit.already_running' : 'remote_jobs.submit.started'))
}

async function submit(): Promise<void> {
  submitting.value = true
  try {
    await (files.value.length ? submitFiles() : submitMagnet())
  } finally {
    submitting.value = false
  }
}

/** Which services can take a job at all. Asked once, when the form appears. */
async function loadProviders(): Promise<void> {
  const response = await api.GET('/api/v1/remote-jobs/providers')
  if (!response.data) {
    providersFailed.value = true
    return
  }
  jobProviders.value = response.data.map(slug => slug.toLowerCase())
}

/**
 * While the form is there, a file dropped anywhere on the page is meant for it, not for the
 * LinkGrabber the window drop zone would otherwise send it to.
 */
let releaseDrops: (() => void) | null = null
watch(showForm, (shown) => {
  releaseDrops?.()
  releaseDrops = shown ? claimFileDrops(addFiles) : null
})
onUnmounted(() => releaseDrops?.())

void loadProviders()
</script>

<template>
  <UAlert
    v-if="providersFailed"
    color="warning"
    variant="subtle"
    icon="i-lucide-circle-alert"
    :description="t('remote_jobs.accounts_unavailable')"
  />
  <DataState
    v-else-if="pending"
    loading
    variant="inline"
    :rows="1"
    :label="t('remote_jobs.accounts_loading')"
    data-testid="remote-job-accounts-loading"
  />
  <!--
    An empty selection says nothing, so it is not shown. When no account belongs to a service
    that has a remote-job plugin, the form says that instead of offering a picker with nothing
    in it.
  -->
  <UAlert
    v-else-if="noUsableAccount"
    color="neutral"
    variant="subtle"
    icon="i-lucide-info"
    :description="t('remote_jobs.no_remote_job_accounts')"
  />
  <div
    v-else
    class="border border-dashed p-3"
    :class="dragging ? 'border-primary bg-primary/5' : 'border-transparent'"
    data-testid="remote-job-drop"
    @dragenter.prevent="dragging = true"
    @dragover.prevent
    @dragleave.self="dragging = false"
    @drop.prevent.stop="drop"
  >
    <div class="grid gap-3 sm:grid-cols-[minmax(0,1fr)_2fr_auto_auto] sm:items-end">
      <UFormField :label="t('remote_jobs.account')">
        <USelect v-model="accountId" :items="accountItems" class="w-full" />
      </UFormField>
      <UFormField :label="t('remote_jobs.submit.magnet')" :description="t('remote_jobs.submit.description')">
        <UInput
          v-if="!files.length"
          v-model="magnet"
          class="w-full font-mono"
          :placeholder="t('remote_jobs.submit.magnet_placeholder')"
        />
        <div v-else class="flex min-h-8 items-center gap-2 border border-muted bg-elevated px-2">
          <UIcon name="i-lucide-files" class="shrink-0 text-muted" />
          <span class="min-w-0 flex-1 truncate text-sm">{{ t('remote_jobs.files.count', { count: files.length }, files.length) }}</span>
          <UButton
            size="xs"
            color="neutral"
            variant="ghost"
            icon="i-lucide-x"
            :aria-label="t('remote_jobs.files.clear')"
            :disabled="submitting"
            @click="files = []"
          />
        </div>
      </UFormField>
      <input
        ref="fileInput"
        class="hidden"
        type="file"
        multiple
        accept=".torrent,.nzb,application/x-bittorrent,application/x-nzb"
        data-testid="remote-job-file"
        @change="pickFiles"
      >
      <UButton
        type="button"
        color="neutral"
        variant="outline"
        icon="i-lucide-file-up"
        :label="t('remote_jobs.submit.file_action')"
        @click="fileInput?.click()"
      />
      <UButton
        type="button"
        icon="i-lucide-cloud-upload"
        :label="t('remote_jobs.submit.action')"
        :disabled="!canSubmit"
        :loading="submitting"
        @click="submit"
      />
    </div>
    <p class="mt-2 text-xs text-muted">{{ t('remote_jobs.files.drop_hint') }}</p>

    <ul v-if="files.length" class="mt-2 divide-y divide-muted border border-muted" :aria-label="t('remote_jobs.files.list')">
      <li v-for="row in files" :key="row.key" class="flex flex-wrap items-center gap-2 px-2 py-1.5">
        <UIcon name="i-lucide-file" class="shrink-0 text-muted" />
        <span class="min-w-0 flex-1 truncate font-mono text-sm">{{ row.file.name }}</span>
        <span class="font-mono text-[11px] tabular-nums text-muted">{{ formatBytes(String(row.file.size)) }}</span>
        <UBadge :color="FILE_STATE_COLORS[row.state]" variant="subtle" size="sm">{{ t(`remote_jobs.files.states.${row.state}`) }}</UBadge>
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-x"
          :aria-label="t('remote_jobs.submit.file_clear')"
          :disabled="row.state === 'sending'"
          @click="removeFile(row.key)"
        />
        <p v-if="row.reason" class="w-full text-xs" :class="row.state === 'failed' ? 'text-error' : 'text-warning'">{{ row.reason }}</p>
      </li>
    </ul>
  </div>
</template>
