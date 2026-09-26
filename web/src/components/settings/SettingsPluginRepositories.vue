<script setup lang="ts">
/**
 * The plugin repositories (RD-140-01): the built-in official one, any the person added, their
 * last check, and the refresh interval.
 *
 * Adding a repository approves its key, the same way an unknown plugin key is approved: the
 * service fetches the index, checks that the pasted key signs it, and answers `409` with the key
 * id and fingerprint; only the fingerprint shown here, sent back, adds the repository. A
 * repository only delivers — every package from it still needs a plugin key the person trusts,
 * which the install preview asks about separately.
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  addRepository,
  groupFingerprint,
  listRepositories,
  refreshRepositories,
  removeRepository,
  setRefreshHours,
  updateRepository,
  type Answer,
  type PluginRepositories,
  type PluginRepository
} from '@/api/pluginRepositories'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useConfirm } from '@/composables/useConfirm'
import { subscribeEvents } from '@/composables/useEventStream'
import { serverMessageFrom, translateServerMessage, type ServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'

/** A repository key the service reported and nobody has approved yet. */
interface PendingApproval {
  keyId: string
  fingerprint: string
  packages: string
  url: string
}

const { t } = useI18n()
const confirm = useConfirm()
const repositories = ref<PluginRepository[]>([])
const refreshHours = ref(24)
const hoursDraft = ref('24')
const loading = ref(true)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
/** The action in flight: a repository id, `refresh`, `interval` or `add`. */
const busy = ref<string | null>(null)
const form = ref({ url: '', publicKey: '', name: '' })
const pending = ref<PendingApproval | null>(null)

const approvalOpen = computed({
  get: () => pending.value !== null,
  set: (value: boolean) => {
    if (!value) pending.value = null
  }
})
const canAdd = computed(() => form.value.url.trim() !== '' && form.value.publicKey.trim() !== '')

let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

onMounted(() => {
  void load()
  releaseEvents = subscribeEvents({ 'plugin.changed': scheduleReload })
})

onUnmounted(() => {
  releaseEvents?.()
  releaseEvents = null
  if (reloadTimer !== null) {
    window.clearTimeout(reloadTimer)
    reloadTimer = null
  }
})

function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void load()
  }, 300)
}

async function load(): Promise<void> {
  adopt(await listRepositories())
  loading.value = false
}

/** Takes a whole answer: the list and the interval are one consistent reading of the service. */
function adopt(answer: Answer<PluginRepositories>): boolean {
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return false
  }
  repositories.value = Array.isArray(answer.data?.repositories) ? answer.data.repositories : []
  if (typeof answer.data?.refresh_hours === 'number') {
    refreshHours.value = answer.data.refresh_hours
    hoursDraft.value = String(answer.data.refresh_hours)
  }
  return true
}

function fail(answer: { message: ServerMessage | null }): void {
  error.value = translateServerMessage(answer.message)
}

async function refresh(): Promise<void> {
  busy.value = 'refresh'
  error.value = null
  message.value = null
  if (adopt(await refreshRepositories())) message.value = t('plugins.repositories.refreshed')
  busy.value = null
}

async function saveInterval(): Promise<void> {
  const hours = Number.parseInt(hoursDraft.value, 10)
  busy.value = 'interval'
  error.value = null
  message.value = null
  // Sent as typed: the service owns the range and answers with its own coded sentence.
  if (adopt(await setRefreshHours(Number.isFinite(hours) ? hours : 0))) {
    message.value = t('plugins.repositories.interval_saved')
  }
  busy.value = null
}

async function setEnabled(repository: PluginRepository, enabled: boolean): Promise<void> {
  busy.value = repository.id
  error.value = null
  const answer = await updateRepository(repository.id, { enabled })
  if (answer.ok) repository.enabled = enabled
  else fail(answer)
  busy.value = null
}

async function remove(repository: PluginRepository): Promise<void> {
  const confirmed = await confirm({
    title: t('plugins.repositories.remove_title'),
    description: t('plugins.repositories.remove_description', { name: repository.name }),
    confirmLabel: t('plugins.repositories.remove'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  busy.value = repository.id
  error.value = null
  const answer = await removeRepository(repository.id)
  if (answer.ok) message.value = translateServerMessage(serverMessageFrom(answer.data))
  else fail(answer)
  busy.value = null
  await load()
}

/**
 * Sends the form; without a fingerprint the service answers with the key to approve, with the
 * fingerprint the person was shown it adds the repository.
 */
async function add(trustFingerprint?: string): Promise<void> {
  busy.value = 'add'
  error.value = null
  message.value = null
  const name = form.value.name.trim()
  const answer = await addRepository(
    { url: form.value.url.trim(), public_key: form.value.publicKey.trim(), ...(name ? { name } : {}) },
    trustFingerprint
  )
  busy.value = null
  if (answer.ok) {
    pending.value = null
    form.value = { url: '', publicKey: '', name: '' }
    message.value = t('plugins.repositories.added')
    await load()
    return
  }
  if (answer.status === 409 && answer.message?.code === 'plugin_repository.key_unconfirmed') {
    const params = answer.message.params ?? {}
    pending.value = {
      keyId: params.key_id ?? '',
      fingerprint: params.fingerprint ?? '',
      packages: params.packages ?? '0',
      url: params.url ?? form.value.url
    }
    return
  }
  fail(answer)
}

async function approve(): Promise<void> {
  const fingerprint = pending.value?.fingerprint
  if (!fingerprint) return
  await add(fingerprint)
}

function lastError(repository: PluginRepository): string {
  const code = repository.last_error ?? ''
  return translateServerMessage({ code, message: code })
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
      <SectionHeader :eyebrow="t('plugins.repositories.eyebrow')" :title="t('plugins.repositories.title')" level="sub" />
      <UButton
        size="xs"
        variant="outline"
        icon="i-lucide-refresh-cw"
        :label="t('plugins.repositories.refresh')"
        :loading="busy === 'refresh'"
        :disabled="busy !== null"
        @click="refresh"
      />
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.repositories.description') }}</p>
    <UAlert v-if="message" class="mb-4" color="success" variant="subtle" :description="message" />
    <UAlert v-if="error" class="mb-4" color="error" variant="subtle" :description="error" />

    <div class="space-y-2" data-plugin-repositories>
      <div v-for="repository in repositories" :key="repository.id" class="flex flex-wrap items-start justify-between gap-4 border border-muted p-3">
        <div class="min-w-0 flex-1">
          <div class="flex flex-wrap items-center gap-2">
            <p class="font-medium text-highlighted">{{ repository.name }}</p>
            <UBadge v-if="repository.kind === 'official'" color="primary" variant="subtle">{{ t('plugins.repositories.official') }}</UBadge>
          </div>
          <p class="mt-1 truncate font-mono text-[11px] text-muted" :title="repository.url">{{ repository.url }}</p>
          <template v-if="repository.key_id && repository.fingerprint">
            <p class="mt-1 text-xs text-muted">{{ t('plugins.repositories.key', { key_id: repository.key_id }) }}</p>
            <p class="break-all font-mono text-[11px] text-muted">{{ groupFingerprint(repository.fingerprint) }}</p>
          </template>
          <p v-if="repository.last_error" class="mt-1 text-xs leading-5 text-error">
            {{ t('plugins.repositories.last_error', { reason: lastError(repository) }) }}
          </p>
          <p class="mt-1 text-xs text-muted">
            {{ repository.last_success_at ? t('plugins.repositories.checked', { when: formatMoment(repository.last_success_at) }) : t('plugins.repositories.never_checked') }}
          </p>
          <p v-if="repository.expires_at" class="text-xs text-muted">{{ t('plugins.repositories.expires', { when: formatMoment(repository.expires_at) }) }}</p>
        </div>
        <div class="flex shrink-0 items-center gap-2">
          <USwitch
            :model-value="repository.enabled"
            :aria-label="t('plugins.repositories.enabled_label', { name: repository.name })"
            :disabled="busy !== null"
            @update:model-value="(value: boolean) => setEnabled(repository, value)"
          />
          <UButton
            v-if="repository.kind !== 'official'"
            size="xs"
            color="error"
            variant="ghost"
            icon="i-lucide-trash-2"
            :aria-label="t('plugins.repositories.remove')"
            :title="t('plugins.repositories.remove')"
            :disabled="busy !== null"
            @click="remove(repository)"
          />
        </div>
      </div>
      <DataState :loading="loading" :error="null" :empty="!repositories.length">
        <p class="border border-dashed border-muted p-6 text-center text-sm text-muted">{{ t('plugins.repositories.empty') }}</p>
      </DataState>
    </div>

    <form class="mt-4 flex flex-col gap-3 sm:flex-row sm:items-end" @submit.prevent="saveInterval">
      <UFormField class="sm:w-64" :label="t('plugins.repositories.interval_label')" :description="t('plugins.repositories.interval_hint')">
        <UInput v-model="hoursDraft" class="mt-2 w-full" type="number" min="1" max="168" />
      </UFormField>
      <UButton type="submit" variant="outline" :label="t('common.actions.save')" :loading="busy === 'interval'" :disabled="busy !== null || hoursDraft === String(refreshHours)" />
    </form>

    <form class="mt-6 space-y-3 border-t border-muted pt-4" @submit.prevent="add()">
      <p class="text-xs font-medium text-highlighted">{{ t('plugins.repositories.add_title') }}</p>
      <UFormField :label="t('plugins.repositories.url_label')">
        <UInput v-model="form.url" class="mt-2 w-full" type="url" placeholder="https://" />
      </UFormField>
      <UFormField :label="t('plugins.repositories.key_label')" :description="t('plugins.repositories.key_hint')">
        <UInput v-model="form.publicKey" class="mt-2 w-full font-mono" />
      </UFormField>
      <UFormField :label="t('plugins.repositories.name_label')">
        <UInput v-model="form.name" class="mt-2 w-full" :maxlength="120" />
      </UFormField>
      <UButton type="submit" icon="i-lucide-plus" :label="t('plugins.repositories.add')" :loading="busy === 'add'" :disabled="!canAdd || busy !== null" />
    </form>

    <UModal v-model:open="approvalOpen" :title="t('plugins.repositories.approve_title')">
      <template #body>
        <div v-if="pending" class="space-y-4" data-repository-approval>
          <p class="break-words text-sm leading-6 text-toned">{{ t('plugins.repositories.approve_intro', { url: pending.url, count: pending.packages }) }}</p>
          <div class="border border-muted bg-elevated p-3">
            <p class="text-xs text-muted">{{ t('plugins.trust.key_id') }}</p>
            <p class="font-mono text-sm text-highlighted">{{ pending.keyId }}</p>
            <p class="mt-3 text-xs text-muted">{{ t('plugins.trust.fingerprint') }}</p>
            <p class="break-all font-mono text-sm text-highlighted">{{ groupFingerprint(pending.fingerprint) }}</p>
          </div>
          <UAlert color="warning" variant="subtle" :description="t('plugins.repositories.approve_warning')" />
        </div>
      </template>
      <template #footer>
        <div v-if="pending" class="flex w-full justify-end gap-2">
          <UButton color="neutral" variant="ghost" :label="t('common.actions.cancel')" @click="pending = null" />
          <UButton color="primary" icon="i-lucide-shield-check" :label="t('plugins.repositories.approve')" :loading="busy === 'add'" @click="approve" />
        </div>
      </template>
    </UModal>
  </section>
</template>
