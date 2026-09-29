<script setup lang="ts">
/**
 * The full backup's destinations (RD-160-02): a folder or NAS path, a folder of an object
 * storage bucket reached through one of the object storage profiles, or an rclone remote —
 * WebDAV included, which has no destination of its own. Each gets its own copy of every
 * archive and its own retention, which can be previewed before it is saved; nothing is deleted
 * by looking. "Verify newest" fetches the newest archive back and checks it where it lies.
 *
 * No destination carries a secret: a profile is chosen by name, a remote named as `name:path`,
 * and their credentials stay where they are configured.
 */
import { computed, onMounted, reactive, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type {
  BackupDestination,
  BackupDestinationKind,
  BackupVerification,
  BackupVerifyState,
  ObjectStorageProfile
} from '@/api/types'
import { translateServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'
import { positiveCount } from '@/utils/positiveCount'

const props = defineProps<{ destinations: BackupDestination[] }>()
const emit = defineEmits<{ changed: [] }>()

const { t } = useI18n()
const toast = useToast()

const KINDS: BackupDestinationKind[] = ['local', 'object_storage', 'rclone']
const VERIFY_COLORS: Record<BackupVerifyState, 'primary' | 'success' | 'error' | 'warning'> = {
  running: 'primary',
  passed: 'success',
  failed: 'error',
  interrupted: 'warning'
}

interface DestinationForm {
  kind: BackupDestinationKind
  name: string
  enabled: boolean
  path: string
  profileId: string
  prefix: string
  remote: string
  keepLast: string | number
  keepDays: string | number
}

const profiles = ref<ObjectStorageProfile[]>([])
const verifications = ref<BackupVerification[]>([])
const editing = ref<string | 'new' | null>(null)
const saving = ref(false)
const busy = ref<string | null>(null)
const error = ref<string | null>(null)
const preview = ref<{ keep: number, remove: string[] } | null>(null)
const form = reactive<DestinationForm>(blank())

const kindItems = computed(() => KINDS.map(kind => ({ label: t(`system.backup.full.destinations.kind.${kind}`), value: kind })))
const profileItems = computed(() => profiles.value.map(profile => ({ label: profile.name, value: String(profile.id) })))

function blank(): DestinationForm {
  return { kind: 'local', name: '', enabled: true, path: '', profileId: '', prefix: '', remote: '', keepLast: '', keepDays: '' }
}


/** The address a destination writes to, as the list shows it. */
function address(destination: BackupDestination): string {
  if (destination.kind === 'local') return destination.path ?? ''
  if (destination.kind === 'rclone') return destination.remote ?? ''
  const profile = profiles.value.find(candidate => String(candidate.id) === destination.profile_id)
  return `${profile?.name ?? destination.profile_id ?? ''} · ${destination.prefix || '—'}`
}

function retention(destination: BackupDestination): string {
  const rules = [
    destination.keep_last ? t('system.backup.full.destinations.retention.last', { count: destination.keep_last }) : null,
    destination.keep_days ? t('system.backup.full.destinations.retention.days', { count: destination.keep_days }) : null
  ].filter(Boolean)
  return rules.length ? rules.join(' · ') : t('system.backup.full.destinations.retention.none')
}

function edit(destination: BackupDestination | null): void {
  error.value = null
  preview.value = null
  Object.assign(form, blank())
  if (destination) {
    Object.assign(form, {
      kind: destination.kind as BackupDestinationKind,
      name: destination.name,
      enabled: destination.enabled,
      path: destination.path ?? '',
      profileId: destination.profile_id ?? '',
      prefix: destination.prefix ?? '',
      remote: destination.remote ?? '',
      keepLast: destination.keep_last ? String(destination.keep_last) : '',
      keepDays: destination.keep_days ? String(destination.keep_days) : ''
    })
  }
  editing.value = destination?.id ?? 'new'
}

function body() {
  return {
    kind: form.kind,
    name: form.name.trim() || null,
    enabled: form.enabled,
    path: form.kind === 'local' ? form.path.trim() : null,
    profile_id: form.kind === 'object_storage' ? form.profileId || null : null,
    prefix: form.kind === 'object_storage' ? form.prefix.trim() : null,
    remote: form.kind === 'rclone' ? form.remote.trim() : null,
    keep_last: positiveCount(form.keepLast),
    keep_days: positiveCount(form.keepDays)
  }
}

async function save(): Promise<void> {
  error.value = null
  saving.value = true
  const response = editing.value === 'new'
    ? await api.POST('/api/v1/backups/destinations', { body: body() })
    : await api.PUT('/api/v1/backups/destinations/{id}', {
      params: { path: { id: editing.value ?? '' } },
      body: body()
    })
  saving.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  editing.value = null
  toast.add({ title: t('system.backup.full.destinations.saved'), color: 'success', icon: 'i-lucide-hard-drive' })
  emit('changed')
}

async function remove(destination: BackupDestination): Promise<void> {
  error.value = null
  busy.value = destination.id
  const response = await api.DELETE('/api/v1/backups/destinations/{id}', { params: { path: { id: destination.id } } })
  busy.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  toast.add({ title: t('system.backup.full.destinations.deleted'), color: 'success', icon: 'i-lucide-trash-2' })
  emit('changed')
}

/** What retention would do with the form's rules; nothing is deleted by asking. */
async function previewRetention(): Promise<void> {
  if (!editing.value || editing.value === 'new') return
  error.value = null
  const query: { keep_last?: number, keep_days?: number } = {}
  const keepLast = positiveCount(form.keepLast)
  const keepDays = positiveCount(form.keepDays)
  if (keepLast != null) query.keep_last = keepLast
  if (keepDays != null) query.keep_days = keepDays
  const response = await api.GET('/api/v1/backups/destinations/{id}/retention', {
    params: { path: { id: editing.value }, query }
  })
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  preview.value = { keep: response.data.keep.length, remove: response.data.remove.map(archive => archive.archive_name) }
}

/** Verifies the newest archive this installation wrote to a destination. */
async function verifyNewest(destination: BackupDestination): Promise<void> {
  error.value = null
  busy.value = destination.id
  const archives = await api.GET('/api/v1/backups/archives', { params: { query: { destination_id: destination.id } } })
  const newest = archives.data?.[0]
  if (!newest) {
    busy.value = null
    error.value = archives.data ? t('system.backup.full.destinations.verify_none') : responseError(archives)
    return
  }
  const response = await api.POST('/api/v1/backups/archives/{id}/verify', { params: { path: { id: newest.id } } })
  busy.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  const started = response.data
  verifications.value = [started, ...verifications.value.filter(row => row.id !== started.id)]
  toast.add({ title: t('system.backup.full.destinations.verify_started'), color: 'info', icon: 'i-lucide-shield-check' })
}

function verificationError(row: BackupVerification): string | null {
  if (!row.error_code) return null
  return translateServerMessage({ code: row.error_code, message: row.error_detail ?? row.error_code })
}

onMounted(async () => {
  const [profileResponse, verificationResponse] = await Promise.all([
    api.GET('/api/v1/object-storage/profiles'),
    api.GET('/api/v1/backups/verifications')
  ])
  profiles.value = profileResponse.data ?? []
  verifications.value = verificationResponse.data ?? []
})
</script>

<template>
  <div class="space-y-4" data-testid="backup-destinations">
    <div class="flex flex-wrap items-center justify-between gap-2">
      <div>
        <h3 class="text-sm font-semibold text-highlighted">{{ t('system.backup.full.destinations.title') }}</h3>
        <p class="text-xs text-muted">{{ t('system.backup.full.destinations.description') }}</p>
      </div>
      <UButton
        icon="i-lucide-plus"
        size="sm"
        :label="t('system.backup.full.destinations.add')"
        :disabled="editing !== null"
        data-testid="backup-destination-add"
        @click="edit(null)"
      />
    </div>
    <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />

    <p v-if="!props.destinations.length && editing === null" class="text-sm text-muted">
      {{ t('system.backup.full.destinations.empty') }}
    </p>
    <ul v-if="props.destinations.length" class="divide-y divide-muted border border-muted" data-testid="backup-destination-list">
      <li v-for="destination in props.destinations" :key="destination.id" class="flex flex-wrap items-center gap-x-3 gap-y-1 p-3 text-sm">
        <UBadge color="neutral" variant="outline">{{ t(`system.backup.full.destinations.kind.${destination.kind}`) }}</UBadge>
        <span class="font-medium text-highlighted">{{ destination.name }}</span>
        <span class="min-w-0 truncate font-mono text-xs text-muted">{{ address(destination) }}</span>
        <UBadge v-if="!destination.enabled" color="warning" variant="subtle">{{ t('system.backup.full.destinations.off') }}</UBadge>
        <UBadge
          v-if="destination.last_verify_state"
          :color="VERIFY_COLORS[destination.last_verify_state]"
          variant="subtle"
        >
          {{ t(`system.backup.full.destinations.verify_state.${destination.last_verify_state}`) }}
        </UBadge>
        <span class="w-full text-xs text-toned">
          {{ retention(destination) }} · {{ t('system.backup.full.destinations.archives', { count: destination.archive_count }) }}
        </span>
        <div class="flex w-full flex-wrap gap-2">
          <UButton
            size="xs"
            color="neutral"
            variant="outline"
            icon="i-lucide-shield-check"
            :label="t('system.backup.full.destinations.verify')"
            :disabled="destination.archive_count === 0"
            :loading="busy === destination.id"
            @click="verifyNewest(destination)"
          />
          <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :label="t('system.backup.full.destinations.edit')" @click="edit(destination)" />
          <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('system.backup.full.destinations.delete')" @click="remove(destination)" />
        </div>
      </li>
    </ul>

    <form v-if="editing !== null" class="space-y-3 border border-muted bg-elevated p-4" data-testid="backup-destination-form" @submit.prevent="save">
      <UFormField name="backup-destination-kind" :label="t('system.backup.full.destinations.kind.label')">
        <USelect v-model="form.kind" :items="kindItems" class="w-full" data-testid="backup-destination-kind" />
      </UFormField>
      <UFormField name="backup-destination-name" :label="t('system.backup.full.destinations.name')" :description="t('system.backup.full.destinations.name_description')">
        <UInput v-model="form.name" class="w-full" />
      </UFormField>
      <UFormField
        v-if="form.kind === 'local'"
        name="backup-destination-path"
        :label="t('system.backup.full.destinations.path')"
        :description="t('system.backup.full.destinations.path_description')"
        required
      >
        <UInput v-model="form.path" class="w-full font-mono" placeholder="/mnt/nas/rdownloader" data-testid="backup-destination-path" />
      </UFormField>
      <template v-if="form.kind === 'object_storage'">
        <UFormField name="backup-destination-profile" :label="t('system.backup.full.destinations.profile')" required>
          <USelect
            v-model="form.profileId"
            :items="profileItems"
            class="w-full"
            :placeholder="profiles.length ? t('system.backup.full.destinations.profile_placeholder') : t('system.backup.full.destinations.profile_none')"
          />
        </UFormField>
        <UFormField name="backup-destination-prefix" :label="t('system.backup.full.destinations.prefix')" :description="t('system.backup.full.destinations.prefix_description')">
          <UInput v-model="form.prefix" class="w-full font-mono" placeholder="bucket/rdownloader" />
        </UFormField>
      </template>
      <UFormField
        v-if="form.kind === 'rclone'"
        name="backup-destination-remote"
        :label="t('system.backup.full.destinations.remote')"
        :description="t('system.backup.full.destinations.remote_description')"
        required
      >
        <UInput v-model="form.remote" class="w-full font-mono" placeholder="webdav:rdownloader" data-testid="backup-destination-remote" />
      </UFormField>
      <div class="grid gap-3 sm:grid-cols-2">
        <UFormField name="backup-destination-keep-last" :label="t('system.backup.full.destinations.keep_last')" :description="t('system.backup.full.destinations.keep_last_description')">
          <UInput v-model="form.keepLast" type="number" min="1" class="w-full" data-testid="backup-destination-keep-last" />
        </UFormField>
        <UFormField name="backup-destination-keep-days" :label="t('system.backup.full.destinations.keep_days')" :description="t('system.backup.full.destinations.keep_days_description')">
          <UInput v-model="form.keepDays" type="number" min="1" class="w-full" />
        </UFormField>
      </div>
      <p class="text-xs text-muted">{{ t('system.backup.full.destinations.retention.hint') }}</p>
      <UFormField name="backup-destination-enabled" :label="t('system.backup.full.destinations.enabled')" orientation="horizontal">
        <USwitch v-model="form.enabled" />
      </UFormField>
      <div v-if="preview" class="border border-muted bg-default p-3 text-xs text-toned" data-testid="backup-destination-preview">
        <p>{{ t('system.backup.full.destinations.retention.result', { keep: preview.keep, remove: preview.remove.length }) }}</p>
        <ul v-if="preview.remove.length" class="mt-1 font-mono text-muted">
          <li v-for="name in preview.remove" :key="name">{{ name }}</li>
        </ul>
      </div>
      <div class="flex flex-wrap gap-2">
        <UButton type="submit" icon="i-lucide-save" :label="t('system.backup.full.destinations.save')" :loading="saving" />
        <UButton
          v-if="editing !== 'new'"
          type="button"
          color="neutral"
          variant="outline"
          icon="i-lucide-list-checks"
          :label="t('system.backup.full.destinations.retention.preview')"
          data-testid="backup-destination-preview-button"
          @click="previewRetention"
        />
        <UButton type="button" color="neutral" variant="ghost" :label="t('system.backup.full.destinations.cancel')" @click="editing = null" />
      </div>
    </form>

    <div v-if="verifications.length">
      <h4 class="mb-2 text-xs font-semibold text-highlighted">{{ t('system.backup.full.destinations.verifications') }}</h4>
      <ul class="divide-y divide-muted border border-muted" data-testid="backup-verifications">
        <li v-for="row in verifications.slice(0, 5)" :key="row.id" class="flex flex-wrap items-center gap-x-3 gap-y-1 p-2 text-xs">
          <UBadge :color="VERIFY_COLORS[row.state]" variant="subtle">{{ t(`system.backup.full.destinations.verify_state.${row.state}`) }}</UBadge>
          <span class="text-toned">{{ formatMoment(row.started_at) }}</span>
          <span class="text-toned">{{ row.destination }}</span>
          <span class="min-w-0 truncate font-mono text-muted">{{ row.archive_name }}</span>
          <span v-if="row.content_checked === false" class="text-muted">{{ t('system.backup.full.destinations.digest_only') }}</span>
          <p v-if="verificationError(row)" class="w-full text-error">{{ verificationError(row) }}</p>
        </li>
      </ul>
    </div>
  </div>
</template>
