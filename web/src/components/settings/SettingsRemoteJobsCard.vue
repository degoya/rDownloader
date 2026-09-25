<script setup lang="ts">
/**
 * Jobs that run at a provider (RD-108-04).
 *
 * The flow behind this card was built by RD-108-03 and had no surface at all: a job in
 * `awaiting_choice` waited for an answer nothing could deliver, and `remote_job.changed` was
 * announced to nobody. This is where a person sees one, answers it and ends it.
 *
 * The two ways a job ends here are deliberately two buttons with two confirmations, and they
 * say different things. **Delete at the provider** reaches into somebody's account on a machine
 * that is not this one and cannot be undone; the server refuses it unless the request carries
 * the confirmation, so the dialog is not decoration. **Remove from this list** forgets the row
 * and sends nothing anywhere. Both go through `useConfirm()` with `destructive: true` and
 * `confirmIcon: 'i-lucide-trash-2'`, which is what `design.md` asks of a destructive action.
 */
import { onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Account, RemoteJob, RemoteJobState } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import RemoteJobSubmitForm from '@/components/settings/RemoteJobSubmitForm.vue'
import { useConfirm } from '@/composables/useConfirm'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFetchState } from '@/composables/useFetchState'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes, formatMoment } from '@/utils/format'

const props = defineProps<{ accounts: Account[], accountsLoading?: boolean }>()
const emit = defineEmits<{ message: [string], error: [string] }>()

const { t } = useI18n()
const confirm = useConfirm()
const { loading, loadError, load } = useFetchState()

const jobs = ref<RemoteJob[]>([])
const busyId = ref<string | null>(null)
const openId = ref<string | null>(null)
/** The entries picked per job, by the provider's own ids. Never filled in on anybody's behalf. */
const picked = ref<Record<string, number[]>>({})

/** A state is named, never implied by colour alone; the colour only reinforces the word. */
const STATE_COLORS: Record<RemoteJobState, 'neutral' | 'primary' | 'warning' | 'success' | 'error'> = {
  submitting: 'neutral',
  preparing: 'neutral',
  awaiting_choice: 'warning',
  working: 'primary',
  ready: 'success',
  failed: 'error',
  discarded: 'neutral'
}

function accountLabel(job: RemoteJob): string {
  return props.accounts.find(account => account.id === job.account_id)?.label
    ?? t('remote_jobs.account_unknown')
}

/** The provider's own word for what happened, or nothing. An absent code prints no placeholder. */
function jobMessage(job: RemoteJob): string | null {
  if (!job.code) return null
  return translateServerMessage({ code: job.code, message: job.message ?? '' })
}

/**
 * Percent, but only where the provider actually measured one (`design.md`).
 *
 * Built here rather than as `{{ percent(job) }}%` in the template, so the figure and its unit
 * are one text node: a reader's screen cannot tell the difference and a test can.
 */
function percent(job: RemoteJob): string | null {
  return typeof job.progress_permille === 'number'
    ? `${Math.round(job.progress_permille / 10)}%`
    : null
}

function entrySize(size: number | null | undefined): string {
  return typeof size === 'number' ? formatBytes(String(size)) : t('remote_jobs.choice.size_unknown')
}

function isPicked(job: RemoteJob, id: number): boolean {
  return (picked.value[job.id] ?? []).includes(id)
}

function togglePicked(job: RemoteJob, id: number, on: boolean): void {
  const current = picked.value[job.id] ?? []
  picked.value = { ...picked.value, [job.id]: on ? [...current, id] : current.filter(entry => entry !== id) }
}

async function refresh(): Promise<void> {
  await load(async () => {
    const response = await api.GET('/api/v1/remote-jobs')
    if (!response.data) return responseError(response)
    jobs.value = response.data
    return null
  })
}

/** Replaces one row in place; the list never reloads to find out what it already knows. */
function replace(job: RemoteJob): void {
  jobs.value = jobs.value.map(existing => (existing.id === job.id ? job : existing))
}

/** A job the form started, or the one already running for that content, at the top of the list. */
function submitted(job: RemoteJob): void {
  jobs.value = jobs.value.some(existing => existing.id === job.id)
    ? jobs.value.map(existing => (existing.id === job.id ? job : existing))
    : [job, ...jobs.value]
}

async function sendChoice(job: RemoteJob): Promise<void> {
  const entries = picked.value[job.id] ?? []
  if (!entries.length) return void emit('error', t('remote_jobs.choice.none'))
  busyId.value = job.id
  const response = await api.POST('/api/v1/remote-jobs/{id}/choice', {
    params: { path: { id: job.id } },
    body: { entries }
  })
  busyId.value = null
  if (!response.data) return void emit('error', responseError(response))
  replace(response.data)
  openId.value = null
  emit('message', t('remote_jobs.choice.sent'))
}

/**
 * Deleting at the provider, and the only place this application asks for it.
 *
 * The dialog names what it reaches — the account at the provider — and says that the entry
 * stays behind afterwards as the record. The server asks the same question again in the shape
 * of `confirmed: true`, so a client that skipped this dialog still deletes nothing.
 */
async function discard(job: RemoteJob): Promise<void> {
  const confirmed = await confirm({
    title: t('remote_jobs.discard.confirm_title'),
    description: t('remote_jobs.discard.confirm_description'),
    confirmLabel: t('remote_jobs.discard.confirm_label'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  busyId.value = job.id
  const response = await api.POST('/api/v1/remote-jobs/{id}/discard', {
    params: { path: { id: job.id } },
    body: { confirmed: true }
  })
  busyId.value = null
  if (!response.data) return void emit('error', responseError(response))
  replace(response.data)
  emit('message', t('remote_jobs.discard.done'))
}

/** Forgetting the row. Nothing leaves this machine, and the dialog says so. */
async function forget(job: RemoteJob): Promise<void> {
  const confirmed = await confirm({
    title: t('remote_jobs.forget.confirm_title'),
    description: t('remote_jobs.forget.confirm_description'),
    confirmLabel: t('remote_jobs.forget.confirm_label'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  busyId.value = job.id
  const response = await api.DELETE('/api/v1/remote-jobs/{id}', { params: { path: { id: job.id } } })
  busyId.value = null
  if (!response.data) return void emit('error', responseError(response))
  jobs.value = jobs.value.filter(existing => existing.id !== job.id)
  emit('message', t('remote_jobs.forget.done'))
}

let releaseEvents: (() => void) | null = null

onMounted(() => {
  void refresh()
  // The sweep advances these rows on its own timer, so the list follows the bus rather than a
  // poll of its own: a job that reaches `awaiting_choice` has to ask its question by itself.
  releaseEvents = subscribeEvents({ 'remote_job.changed': () => void refresh() })
})
onUnmounted(() => releaseEvents?.())
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-start justify-between gap-3">
      <SectionHeader
        :eyebrow="t('remote_jobs.eyebrow')"
        :title="t('remote_jobs.title')"
        :description="t('remote_jobs.description')"
        level="sub"
      />
      <UBadge color="neutral" variant="outline">{{ jobs.length }}</UBadge>
    </div>

    <RemoteJobSubmitForm
      :accounts="props.accounts"
      :accounts-loading="props.accountsLoading"
      @submitted="submitted"
      @message="(text: string) => emit('message', text)"
      @error="(text: string) => emit('error', text)"
    />

    <div class="mt-4 divide-y divide-muted border border-muted">
      <div v-for="job in jobs" :key="job.id" class="p-3">
        <div class="flex flex-wrap items-center gap-3">
          <span class="grid size-8 place-items-center bg-elevated text-primary">
            <UIcon name="i-lucide-cloud-cog" />
          </span>
          <div class="min-w-0 flex-1">
            <p class="text-sm font-medium text-highlighted">{{ accountLabel(job) }}</p>
            <p class="truncate font-mono text-[11px] text-muted">{{ job.content_key }}</p>
          </div>
          <UBadge :color="STATE_COLORS[job.state]" variant="subtle">{{ t(`remote_jobs.states.${job.state}`) }}</UBadge>
          <span v-if="percent(job)" class="font-mono text-xs tabular-nums text-muted">{{ percent(job) }}</span>
          <span class="font-mono text-[11px] text-muted">{{ formatMoment(job.updated_at) }}</span>
          <UButton
            v-if="job.state === 'awaiting_choice'"
            size="xs"
            color="neutral"
            variant="ghost"
            :icon="openId === job.id ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
            :aria-expanded="openId === job.id"
            :label="t('remote_jobs.choice.toggle')"
            @click="openId = openId === job.id ? null : job.id"
          />
          <UButton
            v-if="job.state !== 'discarded' && job.remote_id"
            size="xs"
            color="error"
            variant="ghost"
            icon="i-lucide-cloud-off"
            :label="t('remote_jobs.discard.action')"
            :title="t('remote_jobs.discard.confirm_description')"
            :loading="busyId === job.id"
            @click="discard(job)"
          />
          <UButton
            size="xs"
            color="error"
            variant="ghost"
            icon="i-lucide-trash-2"
            :aria-label="t('remote_jobs.forget.action')"
            :title="t('remote_jobs.forget.confirm_description')"
            :loading="busyId === job.id"
            @click="forget(job)"
          />
        </div>

        <dl class="mt-2 flex flex-wrap gap-x-6 gap-y-1 text-[11px] text-muted">
          <div v-if="job.remote_id" class="flex gap-2">
            <dt>{{ t('remote_jobs.remote_id') }}</dt>
            <dd class="font-mono text-highlighted">{{ job.remote_id }}</dd>
          </div>
          <div class="flex gap-2">
            <dt>{{ t('remote_jobs.started') }}</dt>
            <dd class="font-mono">{{ formatMoment(job.created_at) }}</dd>
          </div>
        </dl>
        <p v-if="jobMessage(job)" class="mt-1 text-xs leading-5" :class="job.state === 'failed' ? 'text-error' : 'text-muted'">
          {{ jobMessage(job) }}
        </p>

        <!--
          Bound to the job's own state as well as to the open row: the sweep, or a second tab,
          can answer the question while this panel is open, and a panel offering entries for a
          job that is no longer waiting invites an answer the server would refuse.
        -->
        <div
          v-if="openId === job.id && job.state === 'awaiting_choice'"
          class="mt-3 border border-warning bg-warning/5 p-3"
        >
          <p class="text-sm font-medium text-highlighted">{{ t('remote_jobs.choice.title') }}</p>
          <p class="mt-1 max-w-prose text-xs leading-5 text-muted">{{ t('remote_jobs.choice.description') }}</p>
          <div class="mt-2 max-h-64 space-y-1 overflow-y-auto">
            <div v-for="entry in job.entries ?? []" :key="entry.id" class="flex items-center gap-2">
              <UCheckbox
                :model-value="isPicked(job, entry.id)"
                :label="entry.path"
                @update:model-value="(on: boolean) => togglePicked(job, entry.id, on)"
              />
              <span class="font-mono text-[11px] text-muted">{{ entrySize(entry.size) }}</span>
              <UBadge v-if="entry.selected" color="neutral" variant="outline" size="sm">{{ t('remote_jobs.choice.preselected') }}</UBadge>
            </div>
          </div>
          <UButton
            class="mt-3"
            size="xs"
            icon="i-lucide-check"
            :label="t('remote_jobs.choice.action')"
            :loading="busyId === job.id"
            @click="sendChoice(job)"
          />
        </div>
      </div>
      <DataState :loading="loading" :error="loadError" :empty="!jobs.length" variant="inline">
        <p class="p-5 text-center text-sm text-muted">{{ t('remote_jobs.empty') }}</p>
      </DataState>
    </div>
  </section>
</template>
