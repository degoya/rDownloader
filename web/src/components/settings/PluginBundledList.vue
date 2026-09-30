<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useBundledServices } from '@/composables/useBundledServices'
import BundledServicePicker from './BundledServicePicker.vue'

/**
 * What the release ships and is not installed (RD-160-05): a service the person did not choose
 * in the wizard, removed, or that is new in this release. One click installs it; nothing here
 * is installed behind the person's back.
 */
const emit = defineEmits<{ installed: [message: string] }>()

const { t } = useI18n()
const { services, loading, loadError, load, refresh, install } = useBundledServices()
const installingKey = ref<string | null>(null)
const error = ref<string | null>(null)
const available = computed(() => services.value.filter(service => service.state !== 'installed'))

onMounted(() => void load())

async function installService(key: string): Promise<void> {
  error.value = null
  installingKey.value = key
  const outcome = await install([key])
  installingKey.value = null
  if (outcome.error) error.value = outcome.error
  else if (outcome.failures.length) {
    error.value = outcome.failures
      .map(failure => t('plugins.bundled.failed', { name: failure.name, reason: failure.message }))
      .join(' ')
  } else {
    emit('installed', t(outcome.restartRequired ? 'plugins.bundled.installed' : 'plugins.bundled.installed_live', outcome.installed))
  }
}

/** Re-read without the skeleton, for the plugin manager's event reload. */
defineExpose({ reload: refresh })
</script>

<template>
  <section data-settings-anchor="plugins.bundled" class="border border-muted bg-default p-5" data-testid="plugin-bundled-list">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader
        :eyebrow="t('plugins.bundled.eyebrow')"
        :title="t('plugins.bundled.title')"
        :description="t('plugins.bundled.description')"
        level="sub"
      />
      <UBadge color="neutral" variant="outline">{{ available.length }}</UBadge>
    </div>
    <UAlert v-if="error" class="mb-4" color="error" variant="subtle" :description="error" />
    <BundledServicePicker
      v-if="!loading && !loadError && available.length"
      :services="available"
      mode="install"
      :busy="installingKey ?? false"
      @install="installService"
    />
    <DataState :loading="loading" :error="loadError" :empty="!available.length">
      <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">
        {{ services.length ? t('plugins.bundled.all_installed') : t('plugins.bundled.none') }}
      </p>
    </DataState>
  </section>
</template>
