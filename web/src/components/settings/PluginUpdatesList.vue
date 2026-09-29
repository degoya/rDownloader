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
 */
import { onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import {
  listOffers,
  type PluginOffer,
  type PluginOffers,
  type PluginUpdate,
  type PreviewSource
} from '@/api/pluginRepositories'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { subscribeEvents } from '@/composables/useEventStream'
import { translateServerMessage } from '@/i18n/server'
import PluginInstallPreviewModal from './PluginInstallPreviewModal.vue'
import { permissionLabels } from './pluginPermissions'

const emit = defineEmits<{
  /** A package was installed from here; the parent re-reads its inventory. */
  installed: [message: string]
}>()

const { t } = useI18n()
const offers = ref<PluginOffers>({ updates: [], available: [] })
const loading = ref(true)
const error = ref<string | null>(null)
const message = ref<string | null>(null)
const previewing = ref<PreviewSource | null>(null)

let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

onMounted(() => {
  void load()
  // Repository writes and installs both announce on `plugin.changed`, the scope this list is
  // read at; a check that brought a new index is one of them.
  releaseEvents = subscribeEvents({ 'plugin.changed': scheduleReload })
})

onUnmounted(() => {
  releaseEvents?.()
  releaseEvents = null
  if (reloadTimer !== null) {
    window.clearTimeout(reloadTimer)
    reloadTimer = null
  }
})

function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void load()
  }, 300)
}

async function load(): Promise<void> {
  const answer = await listOffers()
  loading.value = false
  if (!answer.ok) {
    error.value = translateServerMessage(answer.message)
    return
  }
  error.value = null
  // Read defensively: an answer that is not the documented shape lists nothing rather than
  // breaking the whole settings page around it.
  offers.value = {
    updates: Array.isArray(answer.data?.updates) ? answer.data.updates : [],
    available: Array.isArray(answer.data?.available) ? answer.data.available : []
  }
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
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader :eyebrow="t('plugins.updates.eyebrow')" :title="t('plugins.updates.title')" level="sub" />
      <UBadge color="neutral" variant="outline">{{ offers.updates.length }}</UBadge>
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.updates.description') }}</p>
    <UAlert v-if="message" class="mb-4" color="success" variant="subtle" :description="message" />
    <UAlert v-if="error" class="mb-4" color="error" variant="subtle" :description="error" />

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
        <p class="border border-dashed border-muted p-6 text-center text-sm text-muted">{{ t('plugins.updates.empty') }}</p>
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
  </section>
</template>
