<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import DataState from '@/components/DataState.vue'
import BundledServicePicker from '@/components/settings/BundledServicePicker.vue'
import { useBundledServices } from '@/composables/useBundledServices'
import { useConfirm } from '@/composables/useConfirm'
import { useSessionStore } from '@/stores/session'

/**
 * "Your services" (RD-160-05): which of the bundled services this installation uses. Only those
 * are installed; the rest stay available in Settings → Plugins.
 *
 * Nothing changes while the person ticks boxes. `apply()` runs when the wizard moves on: it first
 * removes what was installed and is unticked now (RD-180-14) — after one confirmation naming it,
 * since a fresh installation already installed every service that needs no account — then
 * installs what is ticked, one service per request with a real progress bar, and resolves only
 * once every provider row is registered, so the accounts step after it never opens on a list
 * that is still filling. `restartRequired` says whether something installed runs only from the
 * next start (RD-170-12); what was removed stops at the next start, which the confirmation says.
 */
const { t } = useI18n()
const router = useRouter()
const session = useSessionStore()
const confirm = useConfirm()
const { services, loading, loadError, load, progress, installing, install, remove } = useBundledServices()
const selected = ref<string[]>([])
const error = ref<string | null>(null)
const restartRequired = ref(false)
const removing = ref(false)

const percent = computed(() => progress.value && progress.value.total
  ? Math.round((progress.value.done / progress.value.total) * 100)
  : 0)
const pending = computed(() => selected.value.filter(key =>
  services.value.some(service => service.key === key && service.state !== 'installed')))
/** Installed and unticked: what "Continue" removes. */
const unticked = computed(() => services.value.filter(service =>
  service.state === 'installed' && !selected.value.includes(service.key)))
const hasInstalled = computed(() => services.value.some(service => service.state === 'installed'))

const installedKeys = (): string[] =>
  services.value.filter(service => service.state === 'installed').map(service => service.key)

onMounted(async () => {
  await load()
  // Everything installed is ticked, and so, by default, is what works without an account.
  selected.value = [
    ...installedKeys(),
    ...services.value
      .filter(service => service.state !== 'installed' && !service.needs_account)
      .map(service => service.key)
  ]
})

/** Leaves a wizard run from the settings for the plugin manager; a first run has to finish. */
function openPlugins(): void {
  session.finishWizard(false)
  void router.push({ name: 'settings', params: { section: 'plugins' } })
}

/** Removes what was unticked; `false` when the person declined or the request was refused. */
async function removeUnticked(): Promise<boolean> {
  const chosen = unticked.value
  if (!chosen.length) return true
  const names = chosen.map(service => service.name).join(', ')
  const confirmed = await confirm({
    title: t('wizard.selection.remove_title'),
    description: t('wizard.selection.remove_confirm', { names }, chosen.length),
    confirmLabel: t('wizard.selection.remove_action'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return false
  removing.value = true
  const outcome = await remove(chosen.map(service => service.key))
  removing.value = false
  if (outcome.error) {
    error.value = outcome.error
    return false
  }
  // One message per service, however many of its versions stayed.
  const messages = new Map<string, string>()
  for (const failure of outcome.failures) {
    const name = chosen.find(service => service.key === failure.service)?.name ?? failure.name
    messages.set(failure.service, failure.code === 'plugin.version_in_use'
      ? t('wizard.selection.remove_in_use', { name })
      : t('wizard.selection.remove_failed', { name, reason: failure.message }))
  }
  if (messages.size) error.value = [...messages.values()].join(' ')
  return !messages.size
}

/** Applies the choice: removes, then installs. Resolves `false` when something did not happen. */
async function apply(): Promise<boolean> {
  error.value = null
  const toInstall = [...pending.value]
  const removed = await removeUnticked()
  if (!removed && !error.value) return false
  if (toInstall.length) {
    const outcome = await install(toInstall)
    restartRequired.value ||= outcome.restartRequired
    if (outcome.error) {
      error.value = outcome.error
    } else if (outcome.failures.length) {
      error.value = [error.value, ...outcome.failures
        .map(failure => t('plugins.bundled.failed', { name: failure.name, reason: failure.message }))]
        .filter(Boolean)
        .join(' ')
    }
  }
  // Read afresh: what stayed installed is ticked again, what failed to install is not.
  selected.value = installedKeys()
  return removed && !error.value
}

defineExpose({ apply, installing, restartRequired })
</script>

<template>
  <div class="space-y-4">
    <UAlert
      v-if="hasInstalled"
      color="info"
      icon="i-lucide-package-check"
      :title="t('wizard.selection.installed_note_title')"
      :description="t('wizard.selection.installed_note')"
      :actions="session.wizardRerun
        ? [{ label: t('wizard.selection.open_plugins'), color: 'info', variant: 'link', trailingIcon: 'i-lucide-arrow-right', onClick: openPlugins }]
        : []"
      data-testid="selection-installed-note"
    />
    <UAlert v-if="error" color="error" :description="error" data-testid="selection-error" />
    <div v-if="progress" class="space-y-2" data-testid="selection-progress">
      <UProgress :model-value="percent" />
      <p class="text-xs text-muted">{{ t('wizard.selection.progress', { done: progress.done, total: progress.total }) }}</p>
    </div>
    <BundledServicePicker
      v-if="!loading && !loadError && services.length"
      v-model:selected="selected"
      :services="services"
      mode="select"
      :busy="installing || removing"
    />
    <DataState :loading="loading" :error="loadError" :empty="!services.length">
      <UEmpty :description="t('plugins.bundled.none')" />
    </DataState>
    <p v-if="services.length" class="text-xs leading-5 text-muted">
      {{ t('wizard.selection.hint', pending.length) }}
      <span v-if="unticked.length" data-testid="selection-remove-hint">{{ t('wizard.selection.remove_hint', unticked.length) }}</span>
    </p>
  </div>
</template>
