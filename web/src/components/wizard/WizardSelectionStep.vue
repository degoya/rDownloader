<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import DataState from '@/components/DataState.vue'
import BundledServicePicker from '@/components/settings/BundledServicePicker.vue'
import { useBundledServices } from '@/composables/useBundledServices'

/**
 * "Your services" (RD-160-05): which of the bundled services this installation uses. Only those
 * are installed; the rest stay available in Settings → Plugins.
 *
 * Nothing installs while the person ticks boxes. `install()` runs when the wizard moves on, one
 * service per request with a real progress bar, and resolves only once every provider row is
 * registered — so the accounts step after it never opens on a list that is still filling.
 * `restartRequired` says whether something installed runs only from the next start (RD-170-12).
 */
const { t } = useI18n()
const { services, loading, loadError, load, progress, installing, install: installServices } = useBundledServices()
const selected = ref<string[]>([])
const error = ref<string | null>(null)
const restartRequired = ref(false)

const percent = computed(() => progress.value && progress.value.total
  ? Math.round((progress.value.done / progress.value.total) * 100)
  : 0)
const pending = computed(() => selected.value.filter(key =>
  services.value.some(service => service.key === key && service.state !== 'installed')))

onMounted(async () => {
  await load()
  // The default is what works without an account; everything installed is ticked already.
  selected.value = services.value
    .filter(service => service.state !== 'installed' && !service.needs_account)
    .map(service => service.key)
})

/** Installs what is ticked and not installed. Resolves `false` when something failed. */
async function install(): Promise<boolean> {
  error.value = null
  if (!pending.value.length) return true
  const outcome = await installServices(pending.value)
  selected.value = []
  restartRequired.value ||= outcome.restartRequired
  if (outcome.error) {
    error.value = outcome.error
    return false
  }
  if (outcome.failures.length) {
    error.value = outcome.failures
      .map(failure => t('plugins.bundled.failed', { name: failure.name, reason: failure.message }))
      .join(' ')
    return false
  }
  return true
}

defineExpose({ install, installing, restartRequired })
</script>

<template>
  <div class="space-y-4">
    <UAlert v-if="error" color="error" variant="subtle" :description="error" data-testid="selection-error" />
    <div v-if="progress" class="space-y-2" data-testid="selection-progress">
      <UProgress :model-value="percent" />
      <p class="text-xs text-muted">{{ t('wizard.selection.progress', { done: progress.done, total: progress.total }) }}</p>
    </div>
    <BundledServicePicker
      v-if="!loading && !loadError && services.length"
      v-model:selected="selected"
      :services="services"
      mode="select"
      :busy="installing"
    />
    <DataState :loading="loading" :error="loadError" :empty="!services.length">
      <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('plugins.bundled.none') }}</p>
    </DataState>
    <p v-if="services.length" class="text-xs leading-5 text-muted">
      {{ t('wizard.selection.hint', pending.length) }}
    </p>
  </div>
</template>
