<script setup lang="ts">
/**
 * The scheduled encrypted full backup (RD-160-01): passphrase, schedule, a run by hand and the
 * history, and its destinations with retention and verification (RD-160-02,
 * `SettingsBackupDestinations`).
 *
 * The passphrase is typed here once and never shown again: the server derives the key, keeps
 * the key in its secret store and answers only with the key's fingerprint. Replacing it asks
 * for the current one. Without a key no
 * backup is written at all, so "run now" and the schedule switch stay unavailable until one
 * is set up. A run happens in the background; the list follows it with a short poll while a
 * run is going, because nothing else on the page needs to know.
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BackupConfig, BackupRun, BackupRunState } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import TimezoneSelect from '@/components/TimezoneSelect.vue'
import SettingsBackupDestinations from '@/components/settings/SettingsBackupDestinations.vue'
import { useFetchState } from '@/composables/useFetchState'
import { translateServerMessage } from '@/i18n/server'
import { formatBytes, formatMoment } from '@/utils/format'

/** Shortest passphrase the server accepts; the settings bundle's rule. */
const MIN_PASSPHRASE = 8
/** How often the history is read again while a run is going. */
const POLL_MS = 3000

const { t } = useI18n()
const toast = useToast()
const { loading, loadError, load } = useFetchState()

const config = ref<BackupConfig | null>(null)
const runs = ref<BackupRun[]>([])
const enabled = ref(false)
const schedule = ref('0 3 * * *')
const timezone = ref('UTC')
const verifySchedule = ref('')
const currentPassphrase = ref('')
const passphrase = ref('')
const confirmation = ref('')
const savingKey = ref(false)
const savingSchedule = ref(false)
const starting = ref(false)
const error = ref<string | null>(null)

const STATE_COLORS: Record<BackupRunState, 'neutral' | 'primary' | 'success' | 'error' | 'warning'> = {
  running: 'primary',
  succeeded: 'success',
  failed: 'error',
  interrupted: 'warning'
}

const running = computed(() => config.value?.running === true || runs.value.some(run => run.state === 'running'))
const keyConfigured = computed(() => config.value?.key_configured === true)
const passphraseReady = computed(() => passphrase.value.length >= MIN_PASSPHRASE
  && passphrase.value === confirmation.value
  && (!keyConfigured.value || currentPassphrase.value.length > 0))
const hasDestination = computed(() => config.value?.destinations.some(destination => destination.enabled) === true)
const canRun = computed(() => keyConfigured.value && hasDestination.value && !running.value)

function adopt(next: BackupConfig): void {
  config.value = next
  enabled.value = next.enabled
  schedule.value = next.schedule
  timezone.value = next.timezone
  verifySchedule.value = next.verify_schedule ?? ''
}

/** Reads the configuration and the history; the error message when either failed. */
async function read(): Promise<string | null> {
  const [configResponse, runsResponse] = await Promise.all([
    api.GET('/api/v1/backups'),
    api.GET('/api/v1/backups/runs')
  ])
  if (!configResponse.data) return responseError(configResponse)
  if (!runsResponse.data) return responseError(runsResponse)
  // The form keeps what is being typed while a poll brings the history up to date.
  if (config.value === null) adopt(configResponse.data)
  else config.value = configResponse.data
  runs.value = runsResponse.data
  return null
}

async function refresh(): Promise<void> {
  await load(read)
  schedulePoll()
}

let poll: ReturnType<typeof setTimeout> | null = null

/**
 * Reads the state again shortly while a run is going; stops by itself once none is. A poll
 * does not go through `load`, so the card does not fall back to its loading surface every
 * three seconds.
 */
function schedulePoll(): void {
  if (poll) clearTimeout(poll)
  poll = running.value
    ? setTimeout(() => {
      void read().then((failure) => {
        if (failure) error.value = failure
        schedulePoll()
      })
    }, POLL_MS)
    : null
}

async function savePassphrase(): Promise<void> {
  error.value = null
  if (passphrase.value.length < MIN_PASSPHRASE) {
    error.value = t('system.backup.full.key.short', { min: MIN_PASSPHRASE })
    return
  }
  if (passphrase.value !== confirmation.value) {
    error.value = t('system.backup.full.key.mismatch')
    return
  }
  if (keyConfigured.value && !currentPassphrase.value) {
    error.value = t('system.backup.full.key.current_required')
    return
  }
  savingKey.value = true
  // Replacing a passphrase asks for the one in force (owner's decision, 2026-09-28).
  const body = keyConfigured.value
    ? { passphrase: passphrase.value, current_passphrase: currentPassphrase.value }
    : { passphrase: passphrase.value }
  const response = await api.PUT('/api/v1/backups/passphrase', { body })
  savingKey.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  currentPassphrase.value = ''
  passphrase.value = ''
  confirmation.value = ''
  adopt(response.data)
  toast.add({ title: t('system.backup.full.key.saved'), color: 'success', icon: 'i-lucide-key-round' })
}

async function saveSchedule(): Promise<void> {
  error.value = null
  savingSchedule.value = true
  const response = await api.PUT('/api/v1/backups', {
    body: {
      enabled: enabled.value,
      schedule: schedule.value,
      timezone: timezone.value,
      verify_schedule: verifySchedule.value.trim() || null
    }
  })
  savingSchedule.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  adopt(response.data)
  toast.add({ title: t('system.backup.full.schedule.saved'), color: 'success', icon: 'i-lucide-calendar-clock' })
}

async function runNow(): Promise<void> {
  error.value = null
  starting.value = true
  const response = await api.POST('/api/v1/backups/runs')
  starting.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  const started = response.data
  runs.value = [started, ...runs.value.filter(run => run.id !== started.id)]
  toast.add({ title: t('system.backup.full.run.started'), color: 'info', icon: 'i-lucide-archive' })
  schedulePoll()
}

function runError(run: BackupRun): string | null {
  if (!run.error_code) return null
  return translateServerMessage({ code: run.error_code, message: run.error_detail ?? run.error_code })
}

/** Each destination of a run that did not get the archive, and why, in words. */
function destinationErrors(run: BackupRun): string[] {
  return run.destinations
    .filter(row => row.error_code && row.state !== 'succeeded')
    .map(row => `${row.destination}: ${translateServerMessage({ code: row.error_code ?? '', message: row.error_detail ?? row.error_code ?? '' })}`)
}

function pruned(run: BackupRun): number {
  return run.destinations.reduce((sum, row) => sum + row.pruned, 0)
}

/** The destinations changed: the configuration is read again, the form keeps what is typed. */
async function destinationsChanged(): Promise<void> {
  const failure = await read()
  if (failure) error.value = failure
}

onMounted(() => void refresh())
onUnmounted(() => {
  if (poll) clearTimeout(poll)
})
</script>

<template>
  <section class="border border-muted bg-default p-5 xl:col-span-2" data-testid="full-backup">
    <SectionHeader
      :eyebrow="t('system.backup.full.eyebrow')"
      :title="t('system.backup.full.title')"
      :description="t('system.backup.full.description')"
    />
    <UAlert v-if="error" class="mt-5" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />

    <DataState :loading="loading" :error="loadError" class="mt-5" />
    <template v-if="!loading && !loadError">
      <div class="mt-5 grid gap-6 lg:grid-cols-2">
        <form class="space-y-4" data-testid="full-backup-key" @submit.prevent="savePassphrase">
          <h3 class="text-sm font-semibold text-highlighted">{{ t('system.backup.full.key.title') }}</h3>
          <div class="flex items-center gap-2 border border-muted bg-elevated p-3 text-xs text-toned">
            <UIcon :name="keyConfigured ? 'i-lucide-lock-keyhole' : 'i-lucide-lock-keyhole-open'" class="size-4 text-primary" />
            <span v-if="keyConfigured && config">
              {{ t('system.backup.full.key.configured', { fingerprint: config.key_fingerprint ?? '', date: formatMoment(config.key_set_at) }) }}
            </span>
            <span v-else>{{ t('system.backup.full.key.missing') }}</span>
          </div>
          <p class="text-xs text-muted">{{ t('system.backup.full.key.hint') }}</p>
          <UFormField v-if="keyConfigured" name="full-backup-current" :label="t('system.backup.full.key.current')" required>
            <UInput v-model="currentPassphrase" type="password" autocomplete="current-password" class="w-full" data-testid="full-backup-current" />
          </UFormField>
          <UFormField name="full-backup-passphrase" :label="t('system.backup.full.key.passphrase')" required>
            <UInput v-model="passphrase" type="password" autocomplete="new-password" class="w-full" />
          </UFormField>
          <UFormField name="full-backup-confirmation" :label="t('system.backup.full.key.confirm')" required>
            <UInput v-model="confirmation" type="password" autocomplete="new-password" class="w-full" />
          </UFormField>
          <UButton
            type="submit"
            icon="i-lucide-key-round"
            :label="keyConfigured ? t('system.backup.full.key.change') : t('system.backup.full.key.setup')"
            :disabled="!passphraseReady"
            :loading="savingKey"
          />
        </form>

        <form class="space-y-4" data-testid="full-backup-schedule" @submit.prevent="saveSchedule">
          <h3 class="text-sm font-semibold text-highlighted">{{ t('system.backup.full.schedule.title') }}</h3>
          <UFormField
            name="full-backup-enabled"
            :label="t('system.backup.full.schedule.enabled')"
            :description="t('system.backup.full.schedule.enabled_description')"
            orientation="horizontal"
          >
            <USwitch v-model="enabled" :disabled="!keyConfigured && !enabled" />
          </UFormField>
          <UFormField
            name="full-backup-cron"
            :label="t('system.backup.full.schedule.cron')"
            :description="t('system.backup.full.schedule.cron_description')"
          >
            <UInput v-model="schedule" class="w-full font-mono" />
          </UFormField>
          <UFormField name="full-backup-timezone" :label="t('system.backup.full.schedule.timezone')">
            <TimezoneSelect v-model="timezone" :aria-label="t('system.backup.full.schedule.timezone')" />
          </UFormField>
          <UFormField
            name="full-backup-verify"
            :label="t('system.backup.full.schedule.verify')"
            :description="t('system.backup.full.schedule.verify_description')"
          >
            <UInput v-model="verifySchedule" class="w-full font-mono" placeholder="0 5 * * 0" data-testid="full-backup-verify" />
          </UFormField>
          <p class="text-xs text-muted">
            {{ config?.next_run_at
              ? t('system.backup.full.schedule.next', { date: formatMoment(config.next_run_at) })
              : t('system.backup.full.schedule.off') }}
            <template v-if="config?.verify_next_run_at">
              · {{ t('system.backup.full.schedule.verify_next', { date: formatMoment(config.verify_next_run_at) }) }}
            </template>
          </p>
          <p v-if="!hasDestination" class="text-xs text-warning">{{ t('system.backup.full.schedule.no_destination') }}</p>
          <div class="flex flex-wrap gap-2">
            <UButton type="submit" icon="i-lucide-calendar-clock" :label="t('system.backup.full.schedule.save')" :loading="savingSchedule" />
            <UButton
              type="button"
              icon="i-lucide-archive"
              color="neutral"
              variant="outline"
              :label="running ? t('system.backup.full.run.running') : t('system.backup.full.run.button')"
              :disabled="!canRun"
              :loading="starting"
              data-testid="full-backup-run"
              @click="runNow"
            />
          </div>
        </form>
      </div>

      <SettingsBackupDestinations
        v-if="config"
        class="mt-6"
        :destinations="config.destinations"
        @changed="destinationsChanged"
      />

      <div class="mt-6">
        <h3 class="mb-3 text-sm font-semibold text-highlighted">{{ t('system.backup.full.history.title') }}</h3>
        <p v-if="!runs.length" class="text-sm text-muted">{{ t('system.backup.full.history.empty') }}</p>
        <ul v-else class="divide-y divide-muted border border-muted" data-testid="full-backup-history">
          <li v-for="run in runs" :key="run.id" class="flex flex-wrap items-center gap-x-3 gap-y-1 p-3 text-sm">
            <UBadge :color="STATE_COLORS[run.state]" variant="subtle">{{ t(`system.backup.full.history.state.${run.state}`) }}</UBadge>
            <span class="text-toned">{{ formatMoment(run.started_at) }}</span>
            <UBadge color="neutral" variant="outline">{{ t(`system.backup.full.history.origin.${run.origin}`) }}</UBadge>
            <span v-if="run.archive_name" class="min-w-0 truncate font-mono text-xs text-muted">{{ run.archive_name }}</span>
            <span v-if="run.size_bytes != null" class="ml-auto font-mono text-xs text-muted">
              {{ formatBytes(String(run.size_bytes)) }} · {{ t('system.backup.full.history.parts', { count: run.parts.length }) }}
            </span>
            <span v-if="pruned(run)" class="w-full text-xs text-muted">{{ t('system.backup.full.history.pruned', { count: pruned(run) }) }}</span>
            <p v-if="runError(run)" class="w-full text-xs text-error">{{ runError(run) }}</p>
            <p v-for="line in destinationErrors(run)" :key="line" class="w-full text-xs text-error">{{ line }}</p>
          </li>
        </ul>
      </div>
    </template>
  </section>
</template>
