<script setup lang="ts">
/**
 * The plugin manager's one view of the plugins (RD-1260-02): every service the release ships and
 * every plugin the repositories offer — installed, available or with an update waiting — in the
 * design of the bundle's list, with each one's origin as an attribute. It replaces the
 * "Available services" and "Updates" cards (owner, 2026-10-12).
 *
 * A bundled service installs on a click, each plugin from the repository when it answers and from
 * the bundle otherwise; nothing is installed behind the person's back. What only a repository
 * offers, and every update, starts at the install preview: publisher, rights and release notes
 * before anything is written (owner, 2026-09-26). An update set to install itself is marked, and
 * so is one that asks for new permissions, which never installs itself (RD-160-09).
 *
 * Above the list, one switch sets every installed plugin to automatic updates, as the updates card
 * had it (RD-191-10): disabled until the service reported it, so it never shows a guessed "off"
 * that a click would turn into a write, and a refused read says so (RA-WEB-03).
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BundledPlugin, BundledService } from '@/api/bundledPlugins'
import { getUpdateSettings, setUpdateSettings, type PreviewSource } from '@/api/pluginRepositories'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useBundledServices } from '@/composables/useBundledServices'
import { translateServerMessage } from '@/i18n/server'
import BundledServicePicker from './BundledServicePicker.vue'
import PluginInstallPreviewModal from './PluginInstallPreviewModal.vue'

const emit = defineEmits<{
  /** A package was installed from here; the parent re-reads its inventory. */
  installed: [message: string]
  /** The switch for all plugins changed; the parent re-reads the cards' policies. */
  automaticChanged: [automatic: boolean]
}>()

const { t } = useI18n()
const { services, repositoryServices, loading, loadError, load, refresh, install } = useBundledServices()
const installingKey = ref<string | null>(null)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const previewing = ref<PreviewSource | null>(null)
/** The switch for all plugins, as the service last reported it; `null` until it has. */
const automaticForAll = ref<boolean | null>(null)
/** Why the switch could not be read. */
const settingsError = ref<string | null>(null)
const switching = ref(false)

type Show = 'all' | 'installed' | 'available' | 'updates'
const show = ref<Show>('all')

const everything = computed<BundledService[]>(() => [...services.value, ...repositoryServices.value])
const previewOnly = computed(() => repositoryServices.value.map(service => service.key))
const updateCount = computed(() =>
  everything.value.reduce((sum, service) => sum + service.plugins.filter(plugin => plugin.update).length, 0))

function matches(service: BundledService, wanted: Show): boolean {
  if (wanted === 'installed') return service.state !== 'available'
  if (wanted === 'available') return service.state !== 'installed'
  if (wanted === 'updates') return service.update_available === true
  return true
}

const showItems = computed(() => (['all', 'installed', 'available', 'updates'] as const).map(value => ({
  value,
  label: t(`plugins.catalogue.show.${value}`),
  count: everything.value.filter(service => matches(service, value)).length
})))
const shown = computed(() => everything.value.filter(service => matches(service, show.value)))

/** The plugins page counts the waiting updates in its tab's badge (RD-180-15). */
defineExpose({ reload: reloadAll, updateCount })

onMounted(() => {
  void load()
  void loadSettings()
})

/** Re-read without the skeleton; the plugin manager calls it on its `plugin.changed` reload. */
async function reloadAll(): Promise<void> {
  await Promise.all([refresh(), loadSettings()])
}

async function loadSettings(): Promise<void> {
  const settings = await getUpdateSettings()
  if (settings.ok) {
    automaticForAll.value = settings.data?.automatic_updates === true
    settingsError.value = null
  } else {
    // A value read earlier stays; only a switch never read stays disabled.
    settingsError.value = translateServerMessage(settings.message)
  }
}

async function setAutomaticForAll(value: boolean): Promise<void> {
  // Disabled until read; a click that still arrives must not write a value never shown.
  if (automaticForAll.value === null) return
  switching.value = true
  error.value = null
  message.value = null
  const answer = await setUpdateSettings(value)
  switching.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  automaticForAll.value = answer.data?.automatic_updates === true
  message.value = t(automaticForAll.value ? 'plugins.updates.automatic_all_on' : 'plugins.updates.automatic_all_off')
  emit('automaticChanged', automaticForAll.value)
  // Every update's mark follows the switch.
  await refresh()
}

async function installService(key: string): Promise<void> {
  error.value = null
  message.value = null
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

/** The preview of a waiting update, or of a package only a repository offers. */
function review(plugin: BundledPlugin): void {
  message.value = null
  const update = plugin.update
  if (update) {
    previewing.value = {
      kind: 'repository',
      repositoryId: update.offer.repository_id,
      pluginId: update.offer.package.plugin_id,
      version: update.offer.package.version
    }
  } else if (plugin.repository_id) {
    previewing.value = {
      kind: 'repository',
      repositoryId: plugin.repository_id,
      pluginId: String(plugin.id),
      version: plugin.repository_version ?? plugin.version
    }
  }
}

async function onInstalled(text: string): Promise<void> {
  previewing.value = null
  message.value = text
  emit('installed', text)
  await refresh()
}
</script>

<template>
  <UCard as="section" data-settings-anchor="plugins.catalogue" data-testid="plugin-catalogue">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader
        :eyebrow="t('plugins.catalogue.eyebrow')"
        :title="t('plugins.catalogue.title')"
        :description="t('plugins.catalogue.description')"
        level="sub"
      />
      <UBadge color="neutral" variant="outline">{{ everything.length }}</UBadge>
    </div>
    <USwitch
      class="mb-4"
      data-automatic-all
      :model-value="automaticForAll === true"
      :disabled="switching || automaticForAll === null"
      :label="t('plugins.updates.automatic_all')"
      :description="t('plugins.updates.automatic_all_hint')"
      @update:model-value="setAutomaticForAll"
    />
    <UAlert v-if="settingsError" class="mb-4" color="error" data-automatic-all-error :description="settingsError" />
    <UAlert v-if="message" class="mb-4" color="success" :description="message" />
    <UAlert v-if="error" class="mb-4" color="error" :description="error" />
    <URadioGroup
      v-if="everything.length"
      v-model="show"
      class="mb-4"
      :items="showItems"
      variant="card"
      indicator="hidden"
      orientation="horizontal"
      size="xs"
      :aria-label="t('plugins.catalogue.show_label')"
    >
      <template #label="{ item }">
        {{ item.label }}<UBadge size="xs" color="neutral" variant="subtle" class="ms-1.5 font-mono">{{ item.count }}</UBadge>
      </template>
    </URadioGroup>
    <BundledServicePicker
      v-if="!loading && !loadError && shown.length"
      :services="shown"
      mode="catalogue"
      :preview-only="previewOnly"
      :busy="installingKey ?? false"
      @install="installService"
      @review="review"
    />
    <DataState :loading="loading" :error="loadError" :empty="!shown.length">
      <UEmpty :description="everything.length ? t('plugins.catalogue.none_shown') : t('plugins.catalogue.none')" />
    </DataState>

    <PluginInstallPreviewModal :source="previewing" @close="previewing = null" @installed="onInstalled" />
  </UCard>
</template>
