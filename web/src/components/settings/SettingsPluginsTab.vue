<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import { listOffers, releaseNotesByPlugin, type PreviewSource, type ReleaseNote } from '@/api/pluginRepositories'
import type { IncompatiblePlugin, InstalledPlugin, PluginLifecycle } from '@/api/types'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { usePluginDiagnostics } from '@/composables/usePluginDiagnostics'
import { usePluginWithdrawals } from '@/composables/usePluginWithdrawals'
import { useFetchState } from '@/composables/useFetchState'
import { subscribeEvents } from '@/composables/useEventStream'
import { withBase } from '@/basePath'
import SectionHeader from '@/components/SectionHeader.vue'
import PluginCard from './PluginCard.vue'
import PluginInstallPreviewModal from './PluginInstallPreviewModal.vue'
import PluginTrustedKeys from './PluginTrustedKeys.vue'
import PluginUpdatesList from './PluginUpdatesList.vue'
import PluginWithdrawDialog from './PluginWithdrawDialog.vue'
import PluginWithdrawnList from './PluginWithdrawnList.vue'
import SettingsPluginRepositories from './SettingsPluginRepositories.vue'
import { displayName, type TrustedKey } from './pluginDisplay'

const { t } = useI18n()
const plugins = ref<InstalledPlugin[]>([])
const incompatible = ref<IncompatiblePlugin[]>([])
/** Per plugin id: which version runs, which is under test, how updates arrive (RD-140-02). */
const lifecycles = ref<PluginLifecycle[]>([])
/** Per plugin id: the release notes the repository indexes delivered, newest first. */
const releaseNotes = ref<Map<string, ReleaseNote[]>>(new Map())

function lifecycleOf(id: string): PluginLifecycle | undefined {
  return lifecycles.value.find(entry => entry.plugin_id === id)
}

/** Shows what a version action answered and re-reads the inventory it changed. */
async function versionActionDone(outcome: { message: string | null, error: string | null }): Promise<void> {
  message.value = outcome.message
  error.value = outcome.error
  await refresh()
}

/**
 * Twenty-four plugins of eight kinds in one flat grid made finding a particular one a scan.
 * The groups are built from the types actually installed, so an installation without, say, a
 * storage plugin is not offered an empty group.
 *
 * This was a `UTabs` bar until RD-107-17, and it stopped being readable through growth rather
 * than through a bug: a tab bar divides one line among its entries, so the eleventh plugin
 * world — `remote-job` — turned ten of the twelve labels into "Benachrich… 3" and "Ordner-Cr… 6".
 * A wrapping chip row grows downward instead, which a settings page has room for; `design.md`
 * carries the rule and the two alternatives that were weighed and rejected.
 */
const ALL_TYPES = '__all__'
const typeTab = ref(ALL_TYPES)

/** One installed plugin: the version that is loaded, and the older ones still on disk. */
interface PluginGroup {
  plugin: InstalledPlugin
  superseded: InstalledPlugin[]
}

/**
 * The inventory as plugins rather than as version directories (RD-108-10).
 *
 * Installing never removes the older version — a job already under way keeps the one that
 * started it — so a package of 43 plugins could report 44 entries, and the difference was a
 * leftover version carrying a badge nobody counted. The list is keyed by plugin id now: the
 * loaded version is the card, every superseded version hangs underneath it, and the counts
 * say how many plugins are installed, which is what the number beside "installed" is read as.
 */
const pluginGroups = computed<PluginGroup[]>(() => {
  const groups = new Map<string, PluginGroup>()
  for (const plugin of plugins.value) {
    const group = groups.get(plugin.id)
    if (!group) groups.set(plugin.id, { plugin, superseded: [] })
    // `active` is the server's answer to which version loads. It comes first in the list, but
    // the grouping does not depend on that: whichever entry claims it becomes the card.
    else if (plugin.active && !group.plugin.active) {
      group.superseded.push(group.plugin)
      group.plugin = plugin
    } else group.superseded.push(plugin)
  }
  return [...groups.values()]
})
const installedTypes = computed(() =>
  [...new Set(pluginGroups.value.map(group => group.plugin.plugin_type))].sort((a, b) =>
    t(`plugins.type.${a}`).localeCompare(t(`plugins.type.${b}`))))
const typeGroups = computed(() => [
  { value: ALL_TYPES, label: t('plugins.type.all'), count: pluginGroups.value.length },
  ...installedTypes.value.map(type => ({
    value: type,
    label: t(`plugins.type.${type}`),
    count: pluginGroups.value.filter(group => group.plugin.plugin_type === type).length
  }))
])
const visibleGroups = computed(() => typeTab.value === ALL_TYPES
  ? pluginGroups.value
  : pluginGroups.value.filter(group => group.plugin.plugin_type === typeTab.value))
/** Plugin id whose superseded versions are unfolded; one at a time, like the diagnostics. */
const openSuperseded = ref<string | null>(null)

function toggleSuperseded(id: string): void {
  openSuperseded.value = openSuperseded.value === id ? null : id
}
const trustedKeys = ref<TrustedKey[]>([])
const packageFile = ref<File | null>(null)
/** The package the install preview shows; the upload installs only from there (RD-140-01). */
const previewSource = ref<PreviewSource | null>(null)
const message = ref<string | null>(null)
const error = ref<string | null>(null)
const confirm = useConfirm()
/** Ids the user switched off; read from the settings document, which is where they are stored. */
const disabledIds = ref<string[]>([])
const isDisabled = (plugin: InstalledPlugin): boolean => disabledIds.value.includes(plugin.id)
/** The inventory, the trust store and the withdrawals are three fetches, so three states. */
const inventoryState = useFetchState()
const keyState = useFetchState()
const revocationState = useFetchState()
const { revocations, pendingWithdrawal, withdrawalReason, withdrawing, isWithdrawn, refreshRevocations, askWithdraw, withdraw, liftWithdrawal } =
  usePluginWithdrawals({ message, error })
const { executions, openDiagnostics, diagnosticsLoading, toggleDiagnostics } = usePluginDiagnostics(error)

/** The live subscription and the timer that coalesces a burst of plugin events into one reload. */
let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

onMounted(() => {
  void inventoryState.load(refresh)
  void keyState.load(refreshKeys)
  void revocationState.load(refreshRevocations)
  releaseEvents = subscribeEvents({
    'plugin.changed': scheduleReload,
    'plugin_trust.changed': scheduleReload
  })
})

onUnmounted(() => {
  releaseEvents?.()
  releaseEvents = null
  if (reloadTimer !== null) {
    window.clearTimeout(reloadTimer)
    reloadTimer = null
  }
})

/**
 * What this tab does when the bus says the plugin state changed somewhere else.
 *
 * Until now the three lists were read once on mount and then only after this tab's own
 * writes, so a key trusted, a key revoked or a package withdrawn in a second tab — or by the
 * service itself — left this one showing the state from when it was opened. The signing keys
 * are the case that matters: a trust store that says a revoked key is still trusted is the one
 * claim it must never make wrongly, and `refreshKeys` already refuses to say "no trusted keys"
 * on a failed read for the same reason.
 *
 * Two events, one reload. What used to be a single `plugin.changed` is now split by the scope
 * its payload needs: `plugin_trust.changed` carries the four trust-store writes — a key
 * trusted or revoked, a digest withdrawn or reinstated — because those payloads name key ids
 * and digests, which is `Secrets`, while `plugin.changed` keeps the administration writes,
 * install, remove and enable/disable. A subscriber that only listened to the first name would
 * have lost exactly the trust store and the withdrawal list, which are the two lists here that
 * must never be wrong. They land on the same debounce on purpose: the three lists are read and
 * shown as one state — a withdrawal is matched against the installed versions — so a burst
 * that touches both scopes should cost one reload, not two.
 *
 * `plugin.changed` is the right name here and only here: this tab reads `/api/v1/plugins`, the
 * inventory, which costs `Admin`. An event is delivered to a subscriber only when it holds that
 * event's exact scope, and scopes widen only towards `Read`, so one name could not serve every
 * screen that a plugin install invalidates. The same payload therefore also goes out as
 * `plugin_catalog.changed` for the `Config` reads — the provider registry and the notification
 * destinations — and as `postprocess_catalog.changed` for the `Queue` ones, the post-processing
 * steps and the upload destinations. Those belong to those screens; this one must not listen to
 * them, or the split stops showing which scope each list is read at.
 *
 * Refetched rather than patched. The event says that something about the plugins changed and
 * nothing this tab could apply: a withdrawal is stored by digest while the card is keyed by
 * id and version, the service resolves which exact package a withdrawal hit, and the inventory
 * additionally carries the disabled set out of the settings document. `withdraw()` re-reads for
 * exactly that reason — guessing any of it here would be a second source for what the service
 * states. Debounced like the queue and the LinkGrabber, because installing a package emits more
 * than one event and three round trips per event is not worth a background reconciliation.
 *
 * `inventoryState.load` and friends are deliberately bypassed: they set `loading` on every
 * call, so an arriving event would trade the lists for their skeletons and back (RD-106-19).
 * The failures are recorded the way `withdraw()` records them instead. No notice is raised —
 * `design.md` has no pattern for announcing that data caught up.
 */
function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void reloadFromEvent()
  }, 300)
}

async function reloadFromEvent(): Promise<void> {
  // In parallel and without short-circuiting: a failed key read must not stop the withdrawals
  // from being re-read, or one stale list would keep the other one stale too.
  const [, keyFailure, revocationFailure] = await Promise.all([
    refresh(),
    refreshKeys(),
    refreshRevocations()
  ])
  const failure = keyFailure ?? revocationFailure
  if (failure) error.value = failure
}

async function refresh(): Promise<string | null> {
  const [inventory, settings] = await Promise.all([
    api.GET('/api/v1/plugins'),
    api.GET('/api/v1/settings')
  ])
  if (inventory.data) {
    plugins.value = inventory.data.installed
    incompatible.value = inventory.data.incompatible
    lifecycles.value = inventory.data.lifecycle ?? []
  } else error.value = responseError(inventory)
  // Release notes come from the repository indexes; without a loaded index there are none,
  // and the version panel shows no notes section rather than an empty one.
  void listOffers().then((answer) => {
    releaseNotes.value = answer.ok ? releaseNotesByPlugin(answer.data) : new Map()
  })
  // The switched-off set lives in the settings document; the inventory lists every installed
  // plugin regardless, so that a disabled one can be switched back on.
  if (settings.data) disabledIds.value = settings.data.disabled_plugins ?? []
  return inventory.data ? null : responseError(inventory)
}

/** Removes a package this build refuses, once the user asks for it. */
async function removeVersion(plugin: { id: string, version: string }): Promise<void> {
  error.value = null
  message.value = null
  try {
    // Through `withBase` like every other request here: served under a path prefix, a
    // hand-built absolute path leaves the application.
    const path = withBase(`/api/v1/plugins/${encodeURIComponent(plugin.id)}/${encodeURIComponent(plugin.version)}`)
    const response = await fetch(path, { method: 'DELETE', credentials: 'same-origin' })
    const payload: unknown = await response.json()
    const serverMessage = serverMessageFrom(payload)
    if (response.ok) message.value = serverMessage ? translateServerMessage(serverMessage) : null
    else error.value = serverMessage ? translateServerMessage(serverMessage) : null
    await refresh()
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : t('plugins.install.network_error')
  }
}

/** Asks first: removing a plugin deletes it from disk, and re-adding means re-installing it. */
async function confirmRemove(plugin: InstalledPlugin): Promise<void> {
  const confirmed = await confirm({
    title: t('plugins.remove.title'),
    description: t('plugins.remove.description', { name: displayName(plugin) }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (confirmed) await removeVersion(plugin)
}

/**
 * Removes one superseded version, once the user asks for it.
 *
 * Also destructive, and asked for separately: the version that is loaded stays, so the
 * sentence has to name which of the two is going. The service refuses with
 * `plugin.version_in_use` when a job is still bound to it, and that answer is what the alert
 * above the list then shows.
 */
async function confirmRemoveSuperseded(active: InstalledPlugin, old: InstalledPlugin): Promise<void> {
  const confirmed = await confirm({
    title: t('plugins.remove.superseded_title'),
    description: t('plugins.remove.superseded_description', {
      name: displayName(active),
      version: old.version,
      active: active.version
    }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (confirmed) await removeVersion(old)
}

/**
 * Switches one plugin off or back on.
 *
 * It stays installed and listed, so it can be switched back on. Like installing one, this takes
 * full effect on the next start — the message the server returns says so.
 */
async function setEnabled(plugin: InstalledPlugin, enabled: boolean): Promise<void> {
  error.value = null
  message.value = null
  const response = await api.PATCH('/api/v1/plugins/{id}', {
    params: { path: { id: plugin.id } },
    body: { enabled }
  })
  if (response.data) {
    message.value = resultMessage(response.data)
    disabledIds.value = enabled
      ? disabledIds.value.filter(id => id !== plugin.id)
      : [...disabledIds.value, plugin.id]
  } else {
    error.value = responseError(response)
  }
}

async function refreshKeys(): Promise<string | null> {
  try {
    const response = await fetch(withBase('/api/v1/plugins/keys'), { credentials: 'same-origin' })
    // A failed key fetch used to leave "no trusted keys" standing, which is the one claim a
    // trust store must never make wrongly.
    if (!response.ok) return t('common.data.load_failed')
    trustedKeys.value = (await response.json()) as TrustedKey[]
    return null
  } catch {
    return t('common.data.load_failed')
  }
}

function selectPackage(event: Event): void {
  const target = event.target
  packageFile.value = target instanceof HTMLInputElement ? target.files?.item(0) ?? null : null
}

/**
 * Opens the install preview for the picked package. Nothing is installed until the person has
 * seen its publisher and permissions there and confirmed — for a trusted key as much as for an
 * unknown one, which the preview confirms in the same step.
 */
function previewUpload(): void {
  if (!packageFile.value) return
  message.value = null
  error.value = null
  previewSource.value = { kind: 'upload', file: packageFile.value }
}

/** After an install from the preview, an update or a repository offer. */
async function onInstalled(text: string): Promise<void> {
  previewSource.value = null
  packageFile.value = null
  message.value = text
  await Promise.all([refresh(), refreshKeys()])
}

async function revokeKey(keyId: string): Promise<void> {
  error.value = null
  try {
    const response = await fetch(withBase(`/api/v1/plugins/keys/${encodeURIComponent(keyId)}`), {
      method: 'DELETE',
      credentials: 'same-origin'
    })
    const payload: unknown = await response.json()
    const serverMessage = serverMessageFrom(payload)
    if (response.ok) message.value = serverMessage ? translateServerMessage(serverMessage) : null
    else error.value = serverMessage ? translateServerMessage(serverMessage) : null
    await refreshKeys()
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : t('plugins.install.network_error')
  }
}
</script>

<template>
  <div class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('plugins.header.eyebrow')"
        :title="t('plugins.header.title')"
        :description="t('plugins.header.description')"
        level="page"
      />
    </header>

    <section class="border border-muted bg-default p-5">
      <form class="flex flex-col gap-3 sm:flex-row sm:items-end" @submit.prevent="previewUpload()">
        <UFormField class="flex-1" :label="t('plugins.install.label')" :description="t('plugins.install.hint')">
          <input class="mt-2 block w-full border border-muted bg-elevated px-3 py-2 text-sm text-toned file:mr-3 file:border-0 file:bg-primary/10 file:px-3 file:py-1 file:text-primary" type="file" accept=".rdplug,application/octet-stream" @change="selectPackage">
        </UFormField>
        <UButton type="submit" icon="i-lucide-package-plus" :label="t('plugins.install.submit')" :disabled="!packageFile" />
      </form>
      <UAlert v-if="message" class="mt-4" color="success" variant="subtle" :description="message" />
      <UAlert v-if="error" class="mt-4" color="error" variant="subtle" :description="error" />
    </section>

    <PluginUpdatesList @installed="onInstalled" />

    <section class="border border-muted bg-default p-5">
      <div class="mb-4 flex items-center justify-between">
        <SectionHeader :eyebrow="t('plugins.installed.eyebrow')" :title="t('plugins.installed.title')" level="sub" />
        <UBadge color="neutral" variant="outline">{{ pluginGroups.length }}</UBadge>
      </div>
      <div
        v-if="pluginGroups.length"
        class="mb-4 flex flex-wrap items-center gap-2"
        role="group"
        :aria-label="t('plugins.installed.filter_label')"
      >
        <UButton
          v-for="group in typeGroups"
          :key="group.value"
          class="max-w-full"
          size="xs"
          :color="typeTab === group.value ? 'primary' : 'neutral'"
          :variant="typeTab === group.value ? 'solid' : 'outline'"
          :aria-pressed="typeTab === group.value"
          @click="typeTab = group.value"
        >
          <span class="whitespace-normal text-left">{{ group.label }}</span>
          <UBadge size="xs" color="neutral" variant="subtle" class="font-mono">{{ group.count }}</UBadge>
        </UButton>
      </div>
      <div class="grid gap-3 md:grid-cols-2">
        <PluginCard
          v-for="{ plugin, superseded } in visibleGroups"
          :key="plugin.id"
          :plugin="plugin"
          :superseded="superseded"
          :disabled="isDisabled(plugin)"
          :is-withdrawn="isWithdrawn"
          :lifecycle="lifecycleOf(plugin.id)"
          :release-notes="releaseNotes.get(String(plugin.id)) ?? []"
          :superseded-open="openSuperseded === plugin.id"
          :diagnostics-open="openDiagnostics === plugin.id"
          :diagnostics-loading="diagnosticsLoading === plugin.id"
          :executions="executions[plugin.id] ?? []"
          @toggle-superseded="toggleSuperseded(plugin.id)"
          @toggle-diagnostics="toggleDiagnostics(plugin)"
          @set-enabled="enabled => setEnabled(plugin, enabled)"
          @withdraw="build => askWithdraw(displayName(plugin), build.id, build.version)"
          @remove="confirmRemove(plugin)"
          @remove-superseded="old => confirmRemoveSuperseded(plugin, old)"
          @version-done="versionActionDone"
        />
        <DataState :loading="inventoryState.loading.value" :error="inventoryState.loadError.value" :empty="!visibleGroups.length" class="md:col-span-2">
          <p class="border border-dashed border-muted p-8 text-center text-sm text-muted">{{ t('plugins.installed.empty') }}</p>
        </DataState>
      </div>
    </section>

    <section v-if="incompatible.length" class="border border-error/40 bg-default p-5">
      <div class="mb-4 flex items-center justify-between">
        <SectionHeader :eyebrow="t('plugins.incompatible.eyebrow')" :title="t('plugins.incompatible.title')" level="sub" />
        <UBadge color="error" variant="outline">{{ incompatible.length }}</UBadge>
      </div>
      <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.incompatible.description') }}</p>
      <div class="space-y-2">
        <div v-for="plugin in incompatible" :key="`${plugin.id}:${plugin.version}`" class="flex items-start justify-between gap-4 border border-muted p-3">
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <h4 class="font-medium text-highlighted">{{ plugin.name }}</h4>
              <UBadge color="neutral" variant="subtle">v{{ plugin.version }}</UBadge>
            </div>
            <p class="mt-1 text-sm leading-5 text-toned">{{ t(`plugins.incompatible.reason.${plugin.code}`) }}</p>
            <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ plugin.id }}</p>
          </div>
          <UButton color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('plugins.incompatible.remove')" @click="removeVersion(plugin)" />
        </div>
      </div>
    </section>

    <PluginWithdrawnList
      :revocations="revocations"
      :installed="plugins"
      :loading="revocationState.loading.value"
      :load-error="revocationState.loadError.value"
      @lift="liftWithdrawal"
    />

    <SettingsPluginRepositories />

    <PluginTrustedKeys
      :keys="trustedKeys"
      :loading="keyState.loading.value"
      :load-error="keyState.loadError.value"
      @revoke="revokeKey"
    />

    <PluginInstallPreviewModal :source="previewSource" @close="previewSource = null" @installed="onInstalled" />

    <PluginWithdrawDialog
      v-model:reason="withdrawalReason"
      :pending="pendingWithdrawal"
      :withdrawing="withdrawing"
      @cancel="pendingWithdrawal = null"
      @confirm="withdraw"
    />
  </div>
</template>
