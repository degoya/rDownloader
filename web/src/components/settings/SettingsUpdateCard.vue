<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import UpdateDetailsModal from '@/components/UpdateDetailsModal.vue'
import { useUpdateStatus } from '@/composables/useUpdateStatus'
import { translateServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'

/**
 * Settings > System > Updates (RD-180-01): whether and how often the service checks, which
 * channel it reads, what it found, and "check now". The three settings are fields of the
 * settings document and are saved with it; the check itself runs on the service.
 */
const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const { status, checking, failure, load, check } = useUpdateStatus()
const detailsOpen = ref(false)

onMounted(() => { void load() })

const channelItems = computed(() => [
  { label: t('system.updates.channel_stable'), value: 'stable' },
  { label: t('system.updates.channel_beta'), value: 'beta' }
])

/**
 * The tap, the bucket, winget and the AUR publish no betas, so such an installation reads the
 * stable channel whatever is chosen; said here as soon as beta is picked, before it is saved.
 */
const NO_BETAS = new Set(['homebrew', 'scoop', 'winget', 'aur'])
const channelForced = computed(() =>
  settings.value.update_channel === 'beta' && status.value !== null && NO_BETAS.has(status.value.install_kind))

/** Why the last check could not run, or what it refused; the status itself shows what it found. */
const lastError = computed(() => {
  if (failure.value) return translateServerMessage(failure.value)
  return status.value?.error_code ? translateServerMessage({ code: status.value.error_code }) : null
})

/** How the last self-update ended, or where the running one stands (RD-180-02). */
const lastInstall = computed(() => {
  const install = status.value?.install
  if (!install) return null
  const text = t(`system.updates.install.state.${install.state}`, {
    version: install.target_version,
    from: install.from_version
  })
  const reason = install.reason ? translateServerMessage({ code: install.reason }) : null
  const failed = install.state === 'rolled_back' || install.state === 'failed'
  return { failed, text: reason ? `${text} ${reason}` : text }
})

async function checkNow(): Promise<void> {
  await check()
}
</script>

<template>
  <section data-settings-anchor="system.updates" class="mt-6 border border-muted bg-default p-5" data-testid="update-settings">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <SectionHeader
        :eyebrow="t('system.updates.eyebrow')"
        :title="t('system.updates.title')"
        :description="t('system.updates.description')"
      />
      <UButton
        icon="i-lucide-refresh-cw"
        color="neutral"
        variant="subtle"
        :label="t('system.updates.check_now')"
        :loading="checking"
        :disabled="status !== null && !status.configured"
        data-testid="update-check-now"
        @click="checkNow()"
      />
    </div>

    <UAlert
      v-if="status && !status.configured"
      class="mt-4"
      color="neutral"
      variant="subtle"
      icon="i-lucide-info"
      :description="t('system.updates.not_configured')"
    />

    <div v-if="status" class="mt-4 grid gap-1 text-sm" data-testid="update-status" aria-live="polite">
      <p class="text-highlighted">
        {{ t('system.updates.current', { version: status.current_version }) }}
        <span class="text-muted">· {{ t('system.updates.installed_as', { kind: t(`system.updates.kind.${status.install_kind}`) }) }}</span>
      </p>
      <p class="text-muted">
        {{ status.last_checked_at ? t('system.updates.last_checked', { when: formatMoment(status.last_checked_at) }) : t('system.updates.never_checked') }}
        <template v-if="status.next_check_at"> · {{ t('system.updates.next_check', { when: formatMoment(status.next_check_at) }) }}</template>
      </p>
      <div v-if="status.available" class="mt-2 flex flex-wrap items-center gap-3" data-testid="update-available">
        <UBadge color="primary" variant="subtle" icon="i-lucide-sparkles">
          {{ t('system.updates.available', { version: status.available.version }) }}
        </UBadge>
        <UButton size="xs" color="neutral" variant="link" :label="t('system.updates.show_details')" @click="detailsOpen = true" />
      </div>
      <p v-else-if="status.last_checked_at && !status.error_code" class="mt-2 text-success" data-testid="update-current">
        {{ t('system.updates.up_to_date') }}
      </p>
      <p v-if="lastInstall" class="mt-2" :class="lastInstall.failed ? 'text-error' : 'text-toned'" data-testid="update-install-last">{{ lastInstall.text }}</p>
      <p v-if="lastError" class="mt-2 text-error" data-testid="update-error">{{ lastError }}</p>
    </div>

    <div class="mt-4 grid gap-4">
      <UFormField :label="t('system.updates.enabled_label')" :description="t('system.updates.enabled_description')" orientation="horizontal">
        <USwitch v-model="settings.update_check_enabled" data-testid="update-enabled" />
      </UFormField>
      <UFormField :label="t('system.updates.channel_label')" :description="t('system.updates.channel_description')">
        <USelect v-model="settings.update_channel" :items="channelItems" value-key="value" class="mt-2 w-full" data-testid="update-channel" />
      </UFormField>
      <p v-if="channelForced" class="text-xs text-muted" data-testid="update-channel-forced">{{ t('system.updates.channel_forced_stable') }}</p>
      <UFormField :label="t('system.updates.interval_label')" :description="t('system.updates.interval_description')">
        <UInput v-model.number="settings.update_check_interval_hours" type="number" min="1" max="168" icon="i-lucide-timer" class="mt-2 w-full" data-testid="update-interval">
          <template #trailing><span class="font-mono text-xs text-muted">h</span></template>
        </UInput>
      </UFormField>
    </div>

    <UpdateDetailsModal v-if="status" v-model:open="detailsOpen" :offer="status.available" :kind="status.install_kind" />
  </section>
</template>
