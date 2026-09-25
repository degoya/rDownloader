<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { CredentialCategory, ReplayPreview } from '@/api/types'
import { formatMoment } from '@/utils/format'

const props = defineProps<{
  preview: ReplayPreview
  /** Inspection only: the dialog explains what would be sent but cannot approve it. */
  readonly?: boolean
}>()

const emit = defineEmits<{
  close: [result: { approvedOrigins: string[] } | null]
}>()

const { t } = useI18n()

/** Origins start fully approved; unchecking narrows the set, which the server allows. */
const selected = ref<string[]>([...props.preview.approved_origins])

const host = computed(() => props.preview.target_origin.replace(/^https?:\/\//, ''))

/**
 * The host each credential category would actually be sent to.
 *
 * A cookie or token travels with the auth profile's scope; a signed parameter and a form
 * body travel with the request itself. Showing them apart is the whole point of the dialog.
 */
function hostFor(category: CredentialCategory): string {
  const profileHost = props.preview.auth_profile?.scope_host
  return category === 'signed_query' || category === 'form_fields'
    ? host.value
    : (profileHost ?? host.value)
}

const canConfirm = computed(
  () => !props.readonly && props.preview.replayable && selected.value.length > 0
)

function submit(): void {
  if (!canConfirm.value) return
  emit('close', { approvedOrigins: [...selected.value] })
}
</script>

<template>
  <UModal
    :title="t('linkgrabber.replay.title')"
    :description="t('linkgrabber.replay.description')"
    :close="{ onClick: () => emit('close', null) }"
    :ui="{ footer: 'justify-end', content: 'sm:max-w-2xl' }"
  >
    <template #body>
      <div class="space-y-4">
        <UAlert
          v-if="!preview.replayable"
          color="warning"
          variant="subtle"
          icon="i-lucide-shield-alert"
          :title="t('linkgrabber.replay.blocked.title')"
          :description="t(`linkgrabber.replay.blocked.${preview.blocked_reason ?? 'refresh_unavailable'}`)"
        />

        <div class="grid gap-3 sm:grid-cols-2">
          <div>
            <p class="text-xs text-muted">{{ t('linkgrabber.replay.target') }}</p>
            <p class="font-mono text-base break-all">{{ host }}</p>
          </div>
          <div>
            <p class="text-xs text-muted">{{ t('linkgrabber.replay.method') }}</p>
            <UBadge :label="preview.method" variant="subtle" />
          </div>
        </div>

        <div>
          <p class="text-xs text-muted">{{ t('linkgrabber.replay.url') }}</p>
          <p class="font-mono text-xs break-all" :title="preview.effective_url ?? preview.url">
            {{ preview.effective_url ?? preview.url }}
          </p>
        </div>

        <div v-if="preview.expires_at">
          <p class="text-xs text-muted">{{ t('linkgrabber.replay.expires_at') }}</p>
          <p class="text-sm">{{ formatMoment(preview.expires_at) }}</p>
        </div>

        <UFormField
          :label="t('linkgrabber.replay.origins.title')"
          :description="t('linkgrabber.replay.origins.hint')"
        >
          <div class="space-y-1">
            <UCheckbox
              v-for="origin in preview.approved_origins"
              :key="origin"
              v-model="selected"
              :value="origin"
              :disabled="readonly"
              :label="origin"
              class="font-mono text-xs"
            />
          </div>
        </UFormField>

        <div>
          <p class="text-xs text-muted">{{ t('linkgrabber.replay.credentials.title') }}</p>
          <p v-if="!preview.credential_categories.length" class="text-sm">
            {{ t('linkgrabber.replay.credentials.none') }}
          </p>
          <div v-else class="mt-1 flex flex-wrap gap-2">
            <UBadge
              v-for="category in preview.credential_categories"
              :key="category"
              color="warning"
              variant="subtle"
              :label="`${t(`linkgrabber.replay.credentials.${category}`)} → ${hostFor(category)}`"
            />
          </div>
          <p v-if="preview.auth_profile" class="mt-2 text-xs text-muted">
            {{ t('linkgrabber.replay.auth_profile', { name: preview.auth_profile.name }) }}
          </p>
        </div>

        <div v-if="preview.body" class="rounded border border-default p-3">
          <p class="text-xs text-muted">{{ t('linkgrabber.replay.body.title') }}</p>
          <p class="font-mono text-xs">{{ preview.body.content_type }}</p>
          <p class="text-xs">
            {{ t('linkgrabber.replay.body.size', { bytes: preview.body.byte_len }) }}
          </p>
          <div v-if="preview.body.field_names?.length" class="mt-2">
            <p class="text-xs text-muted">{{ t('linkgrabber.replay.body.fields') }}</p>
            <div class="mt-1 flex flex-wrap gap-1">
              <UBadge
                v-for="name in preview.body.field_names"
                :key="name"
                variant="outline"
                class="font-mono"
                :label="name"
              />
            </div>
          </div>
          <p class="mt-2 text-xs text-muted">{{ t('linkgrabber.replay.body.values_hidden') }}</p>
        </div>
      </div>
    </template>

    <template #footer>
      <UButton
        :label="t('common.actions.cancel')"
        color="neutral"
        variant="outline"
        @click="emit('close', null)"
      />
      <UButton
        v-if="!readonly"
        :label="t('linkgrabber.replay.consent.grant')"
        icon="i-lucide-shield-check"
        :disabled="!canConfirm"
        @click="submit"
      />
    </template>
  </UModal>
</template>
