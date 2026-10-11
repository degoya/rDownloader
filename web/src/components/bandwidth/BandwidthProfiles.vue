<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, BandwidthProfileRequest, BandwidthScopeLimit } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { GIB, MIB, byteModel, formatBytes } from '@/utils/format'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { DECIMAL, WHOLE, orNull } from '@/utils/numberInput'
import { editingRowClass } from '@/utils/editingRow'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'

const profiles = defineModel<BandwidthProfile[]>({ required: true })
const props = defineProps<{
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const emit = defineEmits<{ changed: [] }>()
const { t } = useI18n()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const scopeKind = ref<'protocol' | 'host' | 'account' | 'category'>('protocol')
const scopeValue = ref('')
const scopeMiB = ref<number | null>(null)
const copyName = useCopyName()
const duplicatingId = ref<string | null>(null)
/** Matches the name check in `crates/rd-api-queue/src/bandwidth_handlers.rs`. */
const MAX_PROFILE_NAME = 100

function emptyForm(): BandwidthProfileRequest {
  return {
    name: '',
    download_bytes_per_second: null,
    upload_bytes_per_second: null,
    max_active_files: null,
    daily_budget_bytes: null,
    monthly_budget_bytes: null,
    scopes: [],
    pause_downloads: false
  }
}

const form = reactive<BandwidthProfileRequest>(emptyForm())

const list = useEditableList<BandwidthProfile, BandwidthProfileRequest>({
  list: profiles,
  create: body => api.POST('/api/v1/bandwidth/profiles', { body }),
  update: (id, body) => api.PUT('/api/v1/bandwidth/profiles/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/bandwidth/profiles/{id}', { params: { path: { id } } }),
  reset: () => Object.assign(form, emptyForm()),
  confirmDelete: profile => ({
    title: t('bandwidth.profile.delete_title'),
    description: t('bandwidth.profile.delete_description', { name: profile.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

/** Rates are entered in MiB/s and budgets in GiB; the API stores raw bytes. */
function scaled(key: 'download_bytes_per_second' | 'upload_bytes_per_second' | 'daily_budget_bytes' | 'monthly_budget_bytes', factor: number) {
  return byteModel(() => form[key], (raw) => { form[key] = raw }, factor)
}

const downloadMiB = scaled('download_bytes_per_second', MIB)
const uploadMiB = scaled('upload_bytes_per_second', MIB)
const dailyGiB = scaled('daily_budget_bytes', GIB)
const monthlyGiB = scaled('monthly_budget_bytes', GIB)

const scopeKinds = computed(() =>
  (['protocol', 'host', 'account', 'category'] as const).map(value => ({
    value,
    label: t(`bandwidth.scope.${value}`)
  }))
)

function addScope(): void {
  const value = scopeValue.value.trim()
  if (!value || !scopeMiB.value || scopeMiB.value <= 0) return
  form.scopes = [
    ...(form.scopes ?? []),
    {
      kind: scopeKind.value,
      value,
      bytes_per_second: Math.round(scopeMiB.value * MIB)
    } as BandwidthScopeLimit
  ]
  scopeValue.value = ''
  scopeMiB.value = null
}

function removeScope(index: number): void {
  form.scopes = (form.scopes ?? []).filter((_, position) => position !== index)
}

function edit(profile: BandwidthProfile): void {
  list.edit(profile)
  Object.assign(form, {
    name: profile.name,
    download_bytes_per_second: profile.download_bytes_per_second,
    upload_bytes_per_second: profile.upload_bytes_per_second,
    max_active_files: profile.max_active_files,
    daily_budget_bytes: profile.daily_budget_bytes,
    monthly_budget_bytes: profile.monthly_budget_bytes,
    scopes: [...(profile.scopes ?? [])],
    pause_downloads: profile.pause_downloads ?? false
  })
  void focusForm()
}

async function submit(): Promise<void> {
  // An emptied parallel count inherits; the number field reports it as `undefined` (RD-1110-10).
  const saved = await list.submit({ ...form, max_active_files: orNull(form.max_active_files), scopes: [...(form.scopes ?? [])] })
  if (saved) emit('changed')
}

/**
 * Copies a profile's limits under a free name and opens the copy for editing (RD-150-12). The
 * schedule windows that point at the original stay with it; the copy is used by nothing yet.
 */
async function duplicate(profile: BandwidthProfile): Promise<void> {
  duplicatingId.value = profile.id
  error.value = null
  const response = await api.POST('/api/v1/bandwidth/profiles', {
    body: {
      name: copyName(profile.name, profiles.value.map(entry => entry.name), MAX_PROFILE_NAME),
      download_bytes_per_second: profile.download_bytes_per_second ?? null,
      upload_bytes_per_second: profile.upload_bytes_per_second ?? null,
      max_active_files: profile.max_active_files ?? null,
      daily_budget_bytes: profile.daily_budget_bytes ?? null,
      monthly_budget_bytes: profile.monthly_budget_bytes ?? null,
      scopes: [...(profile.scopes ?? [])],
      pause_downloads: profile.pause_downloads ?? false
    }
  })
  duplicatingId.value = null
  if (!response.data) return void (error.value = responseError(response))
  profiles.value = [...profiles.value, response.data]
  emit('changed')
  edit(response.data)
}

async function remove(profile: BandwidthProfile): Promise<void> {
  if ((await list.remove(profile)).removed) emit('changed')
}

function rateLabel(value: string | null | undefined): string {
  return value ? `${formatBytes(value)}/s` : t('bandwidth.status.unlimited')
}

/**
 * One line per profile that names everything it limits. It used to carry only the download
 * rate and the number of scope limits, so a profile holding nothing but a monthly budget or a
 * parallel-file cap read "Unlimited · 0 scope limits" (RD-120-53).
 */
function summary(profile: BandwidthProfile): string {
  const scopes = profile.scopes?.length ?? 0
  return [
    rateLabel(profile.download_bytes_per_second),
    profile.upload_bytes_per_second ? t('bandwidth.profile.summary_upload', { rate: rateLabel(profile.upload_bytes_per_second) }) : null,
    profile.max_active_files ? t('bandwidth.profile.summary_parallel', { count: profile.max_active_files }, profile.max_active_files) : null,
    profile.daily_budget_bytes ? t('bandwidth.profile.summary_daily', { size: formatBytes(profile.daily_budget_bytes) }) : null,
    profile.monthly_budget_bytes ? t('bandwidth.profile.summary_monthly', { size: formatBytes(profile.monthly_budget_bytes) }) : null,
    profile.pause_downloads ? t('bandwidth.profile.summary_pause') : null,
    t('bandwidth.profile.summary_scopes', { count: scopes }, scopes)
  ].filter(part => part !== null).join(' · ')
}
</script>

<template>
  <UCard as="section" data-settings-anchor="bandwidth.profiles">
    <FormListLayout :list-title="t('bandwidth.profile.title')" :count="profiles.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('bandwidth.profile.eyebrow')"
          :title="editingId ? t('bandwidth.profile.form_edit') : t('bandwidth.profile.form_new')"
        />
        <p class="mt-2 mb-4 text-xs leading-5 text-muted">{{ t('bandwidth.profile.description') }}</p>
        <UAlert v-if="error" class="mb-3" color="error" :description="error" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('bandwidth.profile.name_label')" required>
            <UInput v-model="form.name" required maxlength="100" class="w-full" icon="i-lucide-gauge" :placeholder="t('bandwidth.profile.name_placeholder')" />
          </UFormField>
          <UFormField :label="t('bandwidth.profile.parallel_label')" :description="t('bandwidth.profile.parallel_description')">
            <UInputNumber v-model="form.max_active_files" :min="1" :max="32" :format-options="WHOLE" class="w-full" :placeholder="t('bandwidth.profile.inherit')" />
          </UFormField>
          <UFormField :label="t('bandwidth.profile.download_label')">
            <NumberWithUnit v-model="downloadMiB" unit="MiB/s" :min="0" :format-options="DECIMAL" :step-snapping="false" class="w-full" :placeholder="t('bandwidth.status.unlimited')" />
          </UFormField>
          <UFormField :label="t('bandwidth.profile.upload_label')" :description="t('bandwidth.profile.upload_description')">
            <NumberWithUnit v-model="uploadMiB" unit="MiB/s" :min="0" :format-options="DECIMAL" :step-snapping="false" class="w-full" :placeholder="t('bandwidth.status.unlimited')" />
          </UFormField>
          <UFormField :label="t('bandwidth.profile.daily_label')">
            <NumberWithUnit v-model="dailyGiB" unit="GiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="w-full" :placeholder="t('bandwidth.profile.no_budget')" />
          </UFormField>
          <UFormField data-settings-anchor="bandwidth.monthly" :label="t('bandwidth.profile.monthly_label')">
            <NumberWithUnit v-model="monthlyGiB" unit="GiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="w-full" :placeholder="t('bandwidth.profile.no_budget')" />
          </UFormField>
          <SettingsCrossLink class="-mt-2" anchor="unattended.power" title-key="power.context.metered_label" />
          <!-- The schedule's download window (RD-1240-30): a profile that pauses downloads. -->
          <UFormField orientation="horizontal" data-settings-anchor="bandwidth.pause_downloads" :label="t('bandwidth.profile.pause_label')" :description="t('bandwidth.profile.pause_description')">
            <USwitch v-model="form.pause_downloads" :aria-label="t('bandwidth.profile.pause_label')" data-testid="profile-pause-downloads" />
          </UFormField>

          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('bandwidth.scope.title') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('bandwidth.scope.description') }}</p>
            <div class="mt-2 flex flex-wrap items-end gap-2">
              <USelect v-model="scopeKind" :items="scopeKinds" value-key="value" class="w-40" :aria-label="t('bandwidth.scope.title')" />
              <UInput v-model="scopeValue" class="w-56" :placeholder="t(`bandwidth.scope.placeholder_${scopeKind}`)" />
              <UFieldGroup class="w-36">
                <UInputNumber v-model="scopeMiB" :min="0" :format-options="DECIMAL" :step-snapping="false" :aria-label="t('bandwidth.scope.limit')" />
                <UBadge color="neutral" variant="outline" label="MiB/s" class="font-mono" />
              </UFieldGroup>
              <UButton type="button" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('bandwidth.scope.add')" @click="addScope" />
            </div>
            <ul v-if="form.scopes?.length" class="mt-3 divide-y divide-muted border border-muted">
              <li v-for="(scope, index) in form.scopes ?? []" :key="`${scope.kind}-${scope.value}-${index}`" class="flex items-center gap-3 p-2">
                <UBadge color="neutral" variant="subtle">{{ t(`bandwidth.scope.${scope.kind}`) }}</UBadge>
                <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ scope.value }}</span>
                <span class="numeric text-xs text-muted">{{ formatBytes(String(scope.bytes_per_second)) }}/s</span>
                <UButton size="xs" color="error" variant="ghost" icon="i-lucide-x" :aria-label="t('common.actions.delete')" @click="removeScope(index)" />
              </li>
            </ul>
          </div>

          <FormActions :editing="editingId !== null" :create-label="t('bandwidth.profile.create')" :loading="pending" @cancel="list.reset" />
        </form>
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="profile in profiles" :key="profile.id" class="flex items-center gap-3 p-3" :class="editingRowClass(editingId === profile.id, 'stripe')">
            <UIcon name="i-lucide-gauge" class="text-primary" />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ profile.name }}</p>
              <p class="numeric text-2xs text-muted">
                {{ summary(profile) }}
              </p>
            </div>
            <UBadge v-if="editingId === profile.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-copy-plus" :label="t('common.actions.duplicate')" :title="t('common.duplicate_hint')" :loading="duplicatingId === profile.id" @click="duplicate(profile)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" :title="t('common.actions.edit')" @click="edit(profile)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" @click="remove(profile)" />
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!profiles.length" variant="inline" class="p-5">
            <UEmpty :description="t('bandwidth.profile.empty')" />
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </UCard>
</template>
