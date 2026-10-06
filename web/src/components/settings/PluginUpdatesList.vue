<script setup lang="ts">
/**
 * Updates for installed plugins, and what else the enabled repositories offer (RD-140-01).
 *
 * An update is shown and installed on a click, with the preview first — publisher, rights and
 * release notes before anything is written (owner, 2026-09-26). A plugin set to update
 * automatically is marked, because the next check installs it without asking; it still becomes
 * active only after a restart. An update that asks for new permissions is marked too: it never
 * installs itself, whatever the policy, so its new rights are seen before they are granted. The
 * mark's tooltip names them (RD-160-09).
 *
 * Above the list, one switch sets every installed plugin to automatic updates, plugins
 * installed later included (RD-191-10). It never rewrites a plugin's own policy: switched off,
 * each plugin updates the way it did before, and the safety rules above hold either way. The
 * switch stays disabled until the service has reported it, so it never shows a guessed "off"
 * that a click would turn into a write, and a refused read says so (RA-WEB-03).
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  getUpdateSettings,
  listOffers,
  setUpdateSettings,
  type PluginOffer,
  type PluginOffers,
  type PluginUpdate,
  type PreviewSource
} from '@/api/pluginRepositories'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useLatestFetch } from '@/composables/useLatestFetch'
import { translateServerMessage } from '@/i18n/server'
import PluginInstallPreviewModal from './PluginInstallPreviewModal.vue'
import { permissionLabels } from './pluginPermissions'

const emit = defineEmits<{
  /** A package was installed from here; the parent re-reads its inventory. */
  installed: [message: string]
  /** The switch for all plugins changed; the parent re-reads the cards' policies. */
  automaticChanged: [automatic: boolean]
}>()

const { t } = useI18n()
const offers = ref<PluginOffers>({ updates: [], available: [], installed: [] })
// A ticket per read, so an older answer that arrives last cannot overwrite a newer one.
const { loading, run } = useLatestFetch()
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const previewing = ref<PreviewSource | null>(null)
/** The switch for all plugins, as the service last reported it; `null` until it has. */
const automaticForAll = ref<boolean | null>(null)
/** Why the switch could not be read. */
const settingsError = ref<string | null>(null)
const switching = ref(false)

/** The plugins page counts the waiting updates in its tab's badge (RD-180-15). */
defineExpose({ updateCount: computed(() => offers.value.updates.length) })

onMounted(() => {
  void load()
})

// Repository writes and installs both announce on `plugin.changed`, the scope this list is
// read at; a check that brought a new index is one of them.
useDebouncedEventRefresh(['plugin.changed'], load)

async function load(): Promise<void> {
  await run(() => Promise.all([listOffers(), getUpdateSettings()]), ([answer, settings]) => {
    if (settings.ok) {
      automaticForAll.value = settings.data?.automatic_updates === true
      settingsError.value = null
    } else {
      // A value read earlier stays; only a switch never read stays disabled.
      settingsError.value = translateServerMessage(settings.message)
    }
    if (!answer.ok) {
      error.value = translateServerMessage(answer.message)
      return
    }
    error.value = null
    // Read defensively: an answer that is not the documented shape lists nothing rather than
    // breaking the whole settings page around it.
    offers.value = {
      updates: Array.isArray(answer.data?.updates) ? answer.data.updates : [],
      available: Array.isArray(answer.data?.available) ? answer.data.available : [],
      installed: Array.isArray(answer.data?.installed) ? answer.data.installed : []
    }
  })
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
  await load()
}

/** Why the update waits for a click, and what exactly it would add. */
function addedPermissionsHint(update: PluginUpdate): string {
  const added = permissionLabels(t, update.added_permissions)
  const hint = t('plugins.updates.adds_permissions_hint')
  return added.length ? `${hint}\n${t('plugins.updates.added_permissions', { permissions: added.join(', ') })}` : hint
}

function review(offer: PluginOffer): void {
  message.value = null
  previewing.value = {
    kind: 'repository',
    repositoryId: offer.repository_id,
    pluginId: offer.package.plugin_id,
    version: offer.package.version
  }
}

async function onInstalled(text: string): Promise<void> {
  previewing.value = null
  message.value = text
  emit('installed', text)
  await load()
}
</script>

<template>
  <UCard as="section" data-settings-anchor="plugins.updates">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader :eyebrow="t('plugins.updates.eyebrow')" :title="t('plugins.updates.title')" level="sub" />
      <UBadge color="neutral" variant="outline">{{ offers.updates.length }}</UBadge>
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.updates.description') }}</p>
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

    <div class="space-y-2" data-plugin-updates>
      <div
        v-for="update in offers.updates"
        :key="`${update.offer.repository_id}:${update.offer.package.plugin_id}`"
        class="flex flex-wrap items-start justify-between gap-3 border border-muted p-3"
      >
        <div class="min-w-0">
          <div class="flex flex-wrap items-center gap-2">
            <p class="font-medium text-highlighted">{{ update.offer.package.name }}</p>
            <UBadge color="primary" variant="subtle" class="font-mono">
              {{ t('plugins.updates.version_change', { from: update.installed_version, to: update.offer.package.version }) }}
            </UBadge>
            <UBadge v-if="update.policy === 'automatic'" color="warning" variant="subtle" :title="t('plugins.updates.automatic_hint')">
              {{ t('plugins.updates.automatic') }}
            </UBadge>
            <UBadge v-if="update.adds_permissions" color="error" variant="subtle" data-adds-permissions :title="addedPermissionsHint(update)">
              {{ t('plugins.updates.adds_permissions') }}
            </UBadge>
          </div>
          <p class="mt-1 text-xs text-muted">{{ t('plugins.updates.from', { repository: update.offer.repository_name }) }}</p>
        </div>
        <UButton size="xs" color="primary" variant="outline" icon="i-lucide-scan-search" :label="t('plugins.updates.review')" @click="review(update.offer)" />
      </div>
      <DataState :loading="loading" :error="null" :empty="!offers.updates.length">
        <UEmpty :description="t('plugins.updates.empty')" />
      </DataState>
    </div>

    <div v-if="offers.available.length" class="mt-6" data-plugin-offers>
      <p class="mb-2 text-xs font-medium text-highlighted">{{ t('plugins.updates.available_title') }}</p>
      <div class="space-y-2">
        <div
          v-for="offer in offers.available"
          :key="`${offer.repository_id}:${offer.package.plugin_id}:${offer.package.version}`"
          class="flex flex-wrap items-start justify-between gap-3 border border-muted p-3"
        >
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
              <p class="font-medium text-highlighted">{{ offer.package.name }}</p>
              <UBadge color="neutral" variant="subtle">v{{ offer.package.version }}</UBadge>
              <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${offer.package.plugin_type}`) }}</UBadge>
              <UBadge v-if="offer.compatibility !== 'compatible'" color="error" variant="subtle">
                {{ t(`plugins.updates.compatibility.${offer.compatibility}`) }}
              </UBadge>
            </div>
            <p class="mt-1 text-xs text-muted">{{ t('plugins.updates.from', { repository: offer.repository_name }) }}</p>
          </div>
          <UButton
            size="xs"
            color="neutral"
            variant="outline"
            icon="i-lucide-scan-search"
            :label="t('plugins.updates.review_package')"
            :disabled="offer.compatibility !== 'compatible'"
            @click="review(offer)"
          />
        </div>
      </div>
    </div>
    <p v-else-if="!loading && !error" class="mt-4 text-xs text-muted">{{ t('plugins.updates.available_empty') }}</p>

    <PluginInstallPreviewModal :source="previewing" @close="previewing = null" @installed="onInstalled" />
  </UCard>
</template>
