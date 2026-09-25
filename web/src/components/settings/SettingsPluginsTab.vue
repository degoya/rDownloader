<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type { IncompatiblePlugin, InstalledPlugin, PluginExecution, PluginRevocation } from '@/api/types'
import { currentLocale } from '@/i18n'
import { loadPluginMessages, providerText, resetPluginMessages } from '@/i18n/plugins'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { subscribeEvents } from '@/composables/useEventStream'
import { withBase } from '@/basePath'
import { formatMoment } from '@/utils/format'
import SectionHeader from '@/components/SectionHeader.vue'

/** A signing key the user has not confirmed yet, as reported by a 409 install response. */
interface PendingKey {
  keyId: string
  fingerprint: string
  name: string
  version: string
}

/**
 * One build the operator is about to withdraw, as the dialog names it.
 *
 * The identity the service stores is the package digest, but nobody recognises a plugin by 64
 * hex characters — so the request goes out as the id and the version the card already shows,
 * and the service resolves which exact package that was.
 */
interface PendingWithdrawal {
  id: string
  version: string
  name: string
}

interface TrustedKey {
  key_id: string
  fingerprint: string
  plugin_name: string | null
  confirmed_at: string
}

const { t } = useI18n()
const plugins = ref<InstalledPlugin[]>([])
const incompatible = ref<IncompatiblePlugin[]>([])

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
/** Loaded on demand per plugin: diagnostics nobody opened cost nothing. */
const executions = ref<Record<string, PluginExecution[]>>({})
const openDiagnostics = ref<string | null>(null)
/**
 * The plugin whose entries are in flight, so the open panel says "loading" rather than looking
 * like a plugin that recorded nothing. It is never both: a fetch that fails clears this and
 * raises `error`, which the card already shows in an alert of its own.
 */
const diagnosticsLoading = ref<string | null>(null)
const trustedKeys = ref<TrustedKey[]>([])
const packageFile = ref<File | null>(null)
const pending = ref(false)
const message = ref<string | null>(null)
const error = ref<string | null>(null)
const pendingKey = ref<PendingKey | null>(null)
const confirm = useConfirm()
/** Ids the user switched off; read from the settings document, which is where they are stored. */
const disabledIds = ref<string[]>([])
const isDisabled = (plugin: InstalledPlugin): boolean => disabledIds.value.includes(plugin.id)
/**
 * Whether this plugin has anything behind its diagnostics accordion (RD-120-28).
 *
 * The inventory carries the number of recorded invocations, never the invocations themselves,
 * so the card can decide whether to offer the control at all without fetching a single entry.
 * That is what lets the rule in `design.md` — a control that opens onto nothing is not
 * rendered — hold without undoing the decision to load the entries on demand.
 */
const hasDiagnostics = (plugin: InstalledPlugin): boolean => (plugin.execution_count ?? 0) > 0
/** The inventory, the trust store and the withdrawals are three fetches, so three states. */
const inventoryState = useFetchState()
const keyState = useFetchState()
const revocationState = useFetchState()
/** Withdrawn packages, newest first as the service lists them. */
const revocations = ref<PluginRevocation[]>([])
const pendingWithdrawal = ref<PendingWithdrawal | null>(null)
const withdrawalReason = ref('')
const withdrawing = ref(false)

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

/**
 * The withdrawals that name an installed version, as `<id>@<version>`.
 *
 * A withdrawal is stored by digest, and a digest says nothing to a reader — so the card that
 * carries the name and the version is where it has to be visible. An entry whose package is
 * not installed here matches nothing and is named in the list below instead, where its digest
 * is the only honest answer.
 */
const withdrawnVersions = computed(() => new Set(
  revocations.value
    .filter(entry => entry.plugin_id && entry.version)
    .map(entry => `${entry.plugin_id}@${entry.version}`)
))

function isWithdrawn(plugin: { id: string, version: string }): boolean {
  return withdrawnVersions.value.has(`${plugin.id}@${plugin.version}`)
}

/** Whether a withdrawn package is one of the versions this machine actually has on disk. */
function isInstalledHere(entry: PluginRevocation): boolean {
  return plugins.value.some(plugin => plugin.id === entry.plugin_id && plugin.version === entry.version)
}

/**
 * What to call a withdrawal: its plugin's name, failing that its id, and failing both a stated
 * "unnamed package" — a digest entered by hand carries no context, and an empty line in its
 * place would read as a row that failed to load.
 */
function revocationName(entry: PluginRevocation): string {
  return entry.plugin_name ?? entry.plugin_id ?? t('plugins.withdrawn.unknown_package')
}

async function refreshRevocations(): Promise<string | null> {
  const response = await api.GET('/api/v1/plugins/revocations')
  if (!response.data) return responseError(response)
  revocations.value = response.data
  return null
}

const withdrawalOpen = computed({
  get: () => pendingWithdrawal.value !== null,
  set: (value: boolean) => {
    if (!value) pendingWithdrawal.value = null
  }
})

/** Opens the dialog for one exact build; the reason starts empty for every one of them. */
function askWithdraw(name: string, id: string, version: string): void {
  withdrawalReason.value = ''
  pendingWithdrawal.value = { id, version, name }
}

/**
 * Withdraws the build the dialog names.
 *
 * The list is fetched again rather than patched: the service answers with the digest it
 * resolved, the context columns and the moment, and guessing any of those here would be a
 * second source for what the service already states.
 */
async function withdraw(): Promise<void> {
  const target = pendingWithdrawal.value
  if (!target) return
  pendingWithdrawal.value = null
  error.value = null
  message.value = null
  withdrawing.value = true
  const reason = withdrawalReason.value.trim()
  const response = await api.POST('/api/v1/plugins/revocations', {
    body: { plugin_id: target.id, version: target.version, ...(reason ? { reason } : {}) }
  })
  if (response.data) message.value = resultMessage(response.data)
  else error.value = responseError(response)
  const failure = await refreshRevocations()
  if (failure) error.value = failure
  withdrawing.value = false
}

/** Takes a withdrawal back. Reversible in both directions, which is why neither asks twice. */
async function liftWithdrawal(digest: string): Promise<void> {
  error.value = null
  message.value = null
  const response = await api.DELETE('/api/v1/plugins/revocations/{digest}', {
    params: { path: { digest } }
  })
  if (response.data) message.value = resultMessage(response.data)
  else error.value = responseError(response)
  const failure = await refreshRevocations()
  if (failure) error.value = failure
}

/** Splits a hex fingerprint into 8-character blocks so it can be compared by eye. */
function groupFingerprint(fingerprint: string): string {
  return (fingerprint.match(/.{1,8}/g) ?? [fingerprint]).join(' ')
}

/** Localised plugin name, falling back to the manifest's own value. */
function displayName(plugin: InstalledPlugin): string {
  return providerText(plugin.provider_slug, 'name') ?? plugin.name
}

function description(plugin: InstalledPlugin): string {
  return providerText(plugin.provider_slug, 'description') ?? plugin.description
}

const modalOpen = computed({
  get: () => pendingKey.value !== null,
  set: (value: boolean) => {
    if (!value) pendingKey.value = null
  }
})

async function refresh(): Promise<string | null> {
  const [inventory, settings] = await Promise.all([
    api.GET('/api/v1/plugins'),
    api.GET('/api/v1/settings')
  ])
  if (inventory.data) {
    plugins.value = inventory.data.installed
    incompatible.value = inventory.data.incompatible
  } else error.value = responseError(inventory)
  // The switched-off set lives in the settings document; the inventory lists every installed
  // plugin regardless, so that a disabled one can be switched back on.
  if (settings.data) disabledIds.value = settings.data.disabled_plugins ?? []
  return inventory.data ? null : responseError(inventory)
}

/**
 * A grant as the manifest declares it. Two of them carry a detail worth showing in full:
 * `secrets:<reference>` names the one credential the plugin may expand, and
 * `net_stream:<ports>` the ports it may dial. The rest are fixed capability names.
 */
function capabilityLabel(capability: string): string {
  const [name, detail] = capability.split(/:(.*)/s)
  if (name === 'secrets') return t('plugins.capability.secret', { reference: detail })
  if (name === 'net_stream') return t('plugins.capability.net_stream', { ports: detail })
  return t(`plugins.capability.${name}`)
}

/** Shows or hides one plugin's recorded invocations, fetching them the first time. */
async function toggleDiagnostics(plugin: InstalledPlugin): Promise<void> {
  if (openDiagnostics.value === plugin.id) {
    openDiagnostics.value = null
    return
  }
  openDiagnostics.value = plugin.id
  diagnosticsLoading.value = plugin.id
  const response = await api.GET('/api/v1/plugins/{id}/executions', { params: { path: { id: plugin.id } } })
  if (response.data) executions.value = { ...executions.value, [plugin.id]: response.data }
  else error.value = responseError(response)
  // Only if nothing else moved on in the meantime: closing the panel, or opening another
  // plugin's, takes the flag from that moment, and a late answer must not clear it for them.
  if (diagnosticsLoading.value === plugin.id) diagnosticsLoading.value = null
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
 * Uploads the package. An unknown signing key comes back as 409 with its fingerprint; the
 * user confirms it, and the same bytes are sent again with that fingerprint attached.
 */
async function install(trustFingerprint?: string): Promise<void> {
  if (!packageFile.value) return
  pending.value = true
  message.value = null
  error.value = null
  try {
    const query = trustFingerprint ? `?trust_fingerprint=${encodeURIComponent(trustFingerprint)}` : ''
    const response = await fetch(withBase(`/api/v1/plugins/install${query}`), {
      method: 'POST',
      credentials: 'same-origin',
      headers: { 'Content-Type': 'application/octet-stream' },
      body: packageFile.value
    })
    const payload: unknown = await response.json()
    const serverMessage = serverMessageFrom(payload)
    if (response.status === 409 && serverMessage?.code === 'plugin.key_untrusted') {
      const params = serverMessage.params ?? {}
      pendingKey.value = {
        keyId: params.key_id ?? '',
        fingerprint: params.fingerprint ?? '',
        name: params.name ?? '',
        version: params.version ?? ''
      }
      return
    }
    if (!response.ok) {
      error.value = serverMessage
        ? translateServerMessage(serverMessage)
        : t('plugins.install.failed', { status: response.status })
      return
    }
    message.value = serverMessage ? translateServerMessage(serverMessage) : t('plugins.install.success')
    packageFile.value = null
    pendingKey.value = null
    // A new plugin brings its own translations along.
    resetPluginMessages()
    await loadPluginMessages(currentLocale())
    await Promise.all([refresh(), refreshKeys()])
  } catch (reason: unknown) {
    error.value = reason instanceof Error ? reason.message : t('plugins.install.network_error')
  } finally {
    pending.value = false
  }
}

async function confirmKey(): Promise<void> {
  const fingerprint = pendingKey.value?.fingerprint
  if (!fingerprint) return
  pendingKey.value = null
  await install(fingerprint)
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
      <form class="flex flex-col gap-3 sm:flex-row sm:items-end" @submit.prevent="install()">
        <UFormField class="flex-1" :label="t('plugins.install.label')" :description="t('plugins.install.hint')">
          <input class="mt-2 block w-full border border-muted bg-elevated px-3 py-2 text-sm text-toned file:mr-3 file:border-0 file:bg-primary/10 file:px-3 file:py-1 file:text-primary" type="file" accept=".rdplug,application/octet-stream" @change="selectPackage">
        </UFormField>
        <UButton type="submit" icon="i-lucide-package-plus" :label="t('plugins.install.submit')" :disabled="!packageFile" :loading="pending" />
      </form>
      <UAlert v-if="message" class="mt-4" color="success" variant="subtle" :description="message" />
      <UAlert v-if="error" class="mt-4" color="error" variant="subtle" :description="error" />
    </section>

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
        <article v-for="{ plugin, superseded } in visibleGroups" :key="plugin.id" class="border border-muted p-4">
          <div class="flex items-start gap-3">
            <span class="grid size-9 place-items-center bg-primary/10 text-primary"><UIcon name="i-lucide-box" /></span>
            <div class="min-w-0 flex-1">
              <div class="flex flex-wrap items-center gap-2"><h4 class="font-medium text-highlighted">{{ displayName(plugin) }}</h4><UBadge :color="plugin.active ? 'primary' : 'neutral'" variant="subtle" :title="plugin.active ? t('plugins.card.active_version_hint') : t('plugins.card.superseded_hint')">v{{ plugin.version }}</UBadge><UBadge v-if="isDisabled(plugin)" color="warning" variant="subtle">{{ t('plugins.actions.disabled_badge') }}</UBadge><UBadge v-if="isWithdrawn(plugin)" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge></div>
              <p class="mt-1 text-sm leading-5 text-toned">{{ description(plugin) }}</p>
              <p class="mt-1 text-xs text-muted">
                {{ t('plugins.card.author', { author: plugin.author }) }}
                <span v-if="plugin.license"> · {{ plugin.license }}</span>
              </p>
              <p class="mt-1 flex flex-wrap gap-3 text-xs">
                <a v-if="plugin.homepage" class="text-primary hover:underline" :href="plugin.homepage" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.homepage') }}</a>
                <a v-if="plugin.support_url" class="text-primary hover:underline" :href="plugin.support_url" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.support') }}</a>
              </p>
              <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ plugin.id }}</p>
            </div>
            <UTooltip :text="t('plugins.card.concurrency_hint', { count: plugin.max_concurrent_downloads })">
              <div class="shrink-0 text-right">
                <p class="font-mono text-sm leading-none text-toned">{{ plugin.max_concurrent_downloads }}</p>
                <p class="mt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.concurrency') }}</p>
              </div>
            </UTooltip>
          </div>
          <div class="mt-3 flex flex-wrap gap-1">
            <UBadge color="primary" variant="subtle">{{ plugin.provider_slug }}</UBadge>
            <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${plugin.plugin_type}`) }}</UBadge>
            <UBadge color="neutral" variant="subtle">{{ t('plugins.card.api_version', { version: plugin.api_version }) }}</UBadge>
          </div>
          <div class="mt-2">
            <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.capabilities') }}</p>
            <div class="mt-1 flex flex-wrap gap-1">
              <UBadge v-for="capability in plugin.capabilities" :key="capability" color="warning" variant="outline">{{ capabilityLabel(capability) }}</UBadge>
            </div>
          </div>
          <div class="mt-2 flex flex-wrap gap-1">
            <UBadge v-for="domain in plugin.domains" :key="domain" color="neutral" variant="outline">{{ domain }}</UBadge>
          </div>
          <div v-if="superseded.length" class="mt-3 border-t border-muted pt-2">
            <UButton
              size="xs"
              color="neutral"
              variant="ghost"
              :icon="openSuperseded === plugin.id ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
              :aria-expanded="openSuperseded === plugin.id"
              :label="t('plugins.card.superseded_versions', { count: superseded.length })"
              @click="toggleSuperseded(plugin.id)"
            />
            <div v-if="openSuperseded === plugin.id" class="mt-2 space-y-1">
              <p class="text-xs leading-5 text-muted">{{ t('plugins.card.superseded_hint') }}</p>
              <div
                v-for="old in superseded"
                :key="old.version"
                class="flex items-center justify-between gap-3 border border-muted px-2 py-1 text-xs"
              >
                <div class="flex flex-wrap items-center gap-2">
                  <span class="font-mono text-toned">v{{ old.version }}</span>
                  <UBadge size="xs" color="warning" variant="subtle">{{ t('plugins.card.superseded') }}</UBadge>
                  <UBadge v-if="isWithdrawn(old)" size="xs" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge>
                </div>
                <div class="flex shrink-0 items-center gap-1">
                  <UButton
                    v-if="!isWithdrawn(old)"
                    size="xs"
                    color="neutral"
                    variant="ghost"
                    icon="i-lucide-shield-off"
                    :aria-label="t('plugins.card.withdraw_version', { version: old.version })"
                    :title="t('plugins.card.withdraw_version', { version: old.version })"
                    @click="askWithdraw(displayName(plugin), old.id, old.version)"
                  />
                  <UButton
                    size="xs"
                    color="error"
                    variant="ghost"
                    icon="i-lucide-trash-2"
                    :aria-label="t('plugins.card.remove_superseded', { version: old.version })"
                    :title="t('plugins.card.remove_superseded', { version: old.version })"
                    @click="confirmRemoveSuperseded(plugin, old)"
                  />
                </div>
              </div>
            </div>
          </div>
          <div class="mt-3 flex flex-wrap items-center gap-2 border-t border-muted pt-2">
            <UButton
              v-if="hasDiagnostics(plugin)"
              size="xs"
              color="neutral"
              variant="ghost"
              :icon="openDiagnostics === plugin.id ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
              :label="t('plugins.diagnostics.title')"
              @click="toggleDiagnostics(plugin)"
            />
            <div class="ml-auto flex items-center gap-1">
              <UButton
                size="xs"
                color="neutral"
                variant="outline"
                :icon="isDisabled(plugin) ? 'i-lucide-play' : 'i-lucide-power-off'"
                :label="isDisabled(plugin) ? t('plugins.actions.enable') : t('plugins.actions.disable')"
                @click="setEnabled(plugin, isDisabled(plugin))"
              />
              <UButton
                v-if="!isWithdrawn(plugin)"
                size="xs"
                color="neutral"
                variant="ghost"
                icon="i-lucide-shield-off"
                :label="t('plugins.actions.withdraw')"
                @click="askWithdraw(displayName(plugin), plugin.id, plugin.version)"
              />
              <UButton
                size="xs"
                color="error"
                variant="ghost"
                icon="i-lucide-trash-2"
                :label="t('common.actions.delete')"
                @click="confirmRemove(plugin)"
              />
            </div>
            <div v-if="hasDiagnostics(plugin) && openDiagnostics === plugin.id" class="mt-2 space-y-1">
              <p v-if="diagnosticsLoading === plugin.id" class="text-xs text-muted">{{ t('common.data.loading') }}</p>
              <div
                v-for="entry in executions[plugin.id] ?? []"
                :key="entry.correlation_id"
                class="flex items-start justify-between gap-3 border border-muted px-2 py-1 text-xs"
              >
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <UBadge size="xs" :color="entry.outcome === 'ok' ? 'success' : entry.outcome === 'failed' ? 'warning' : 'error'" variant="subtle">
                      {{ t(`plugins.diagnostics.outcome.${entry.outcome}`) }}
                    </UBadge>
                    <span class="font-mono text-toned">{{ entry.operation }}</span>
                    <span class="text-muted">v{{ entry.plugin_version }}</span>
                  </div>
                  <p v-if="entry.message" class="mt-1 break-words text-muted">{{ entry.message }}</p>
                  <p class="mt-1 font-mono text-[10px] text-muted">{{ entry.correlation_id }}</p>
                </div>
                <div class="shrink-0 text-right text-muted">
                  <p>{{ formatMoment(entry.started_at) }}</p>
                  <p>{{ t('plugins.diagnostics.duration', { ms: entry.duration_ms }) }}</p>
                </div>
              </div>
            </div>
          </div>
        </article>
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

    <section class="border border-muted bg-default p-5">
      <div class="mb-4 flex items-center justify-between">
        <SectionHeader :eyebrow="t('plugins.withdrawn.eyebrow')" :title="t('plugins.withdrawn.title')" level="sub" />
        <UBadge color="neutral" variant="outline">{{ revocations.length }}</UBadge>
      </div>
      <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.withdrawn.description') }}</p>
      <div class="space-y-2">
        <div v-for="entry in revocations" :key="entry.digest" class="flex items-start justify-between gap-4 border border-muted p-3">
          <div class="min-w-0">
            <div class="flex flex-wrap items-center gap-2">
              <p class="font-medium text-highlighted">{{ revocationName(entry) }}</p>
              <UBadge v-if="entry.version" color="neutral" variant="subtle">v{{ entry.version }}</UBadge>
              <UBadge v-if="isInstalledHere(entry)" color="warning" variant="subtle">{{ t('plugins.withdrawn.installed') }}</UBadge>
            </div>
            <!-- The digest stands where it is the only honest answer: a build this machine no
                 longer has cannot be pointed at by name and version alone. -->
            <template v-if="!isInstalledHere(entry)">
              <p class="mt-1 text-xs leading-5 text-muted">{{ t('plugins.withdrawn.not_installed') }}</p>
              <p class="mt-2 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.withdrawn.digest') }}</p>
              <p class="break-all font-mono text-[11px] text-muted">{{ groupFingerprint(entry.digest) }}</p>
            </template>
            <p v-if="entry.reason" class="mt-1 text-xs leading-5 text-toned">{{ t('plugins.withdrawn.reason', { reason: entry.reason }) }}</p>
            <p class="mt-1 text-xs text-muted">{{ t('plugins.withdrawn.since', { when: formatMoment(entry.revoked_at) }) }}</p>
          </div>
          <UButton
            size="xs"
            color="neutral"
            variant="ghost"
            icon="i-lucide-undo-2"
            :label="t('plugins.withdrawn.lift')"
            @click="liftWithdrawal(entry.digest)"
          />
        </div>
        <DataState :loading="revocationState.loading.value" :error="revocationState.loadError.value" :empty="!revocations.length">
          <p class="border border-dashed border-muted p-8 text-center text-sm text-muted">{{ t('plugins.withdrawn.empty') }}</p>
        </DataState>
      </div>
    </section>

    <section class="border border-muted bg-default p-5">
      <div class="mb-4 flex items-center justify-between">
        <SectionHeader :eyebrow="t('plugins.keys.eyebrow')" :title="t('plugins.keys.title')" level="sub" />
        <UBadge color="neutral" variant="outline">{{ trustedKeys.length }}</UBadge>
      </div>
      <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.keys.description') }}</p>
      <div class="space-y-2">
        <div v-for="key in trustedKeys" :key="key.key_id" class="flex items-start justify-between gap-4 border border-muted p-3">
          <div class="min-w-0">
            <p class="font-medium text-highlighted">{{ key.key_id }}</p>
            <p class="mt-1 break-all font-mono text-[11px] text-muted">{{ groupFingerprint(key.fingerprint) }}</p>
            <p v-if="key.plugin_name" class="mt-1 text-xs text-muted">{{ t('plugins.keys.first_seen', { plugin: key.plugin_name }) }}</p>
          </div>
          <UButton color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('plugins.keys.revoke')" @click="revokeKey(key.key_id)" />
        </div>
        <DataState :loading="keyState.loading.value" :error="keyState.loadError.value" :empty="!trustedKeys.length">
          <p class="border border-dashed border-muted p-8 text-center text-sm text-muted">{{ t('plugins.keys.empty') }}</p>
        </DataState>
      </div>
    </section>

    <UModal v-model:open="modalOpen" :title="t('plugins.trust.title')">
      <template #body>
        <div v-if="pendingKey" class="space-y-4">
          <p class="text-sm leading-6 text-toned">{{ t('plugins.trust.intro', { name: pendingKey.name, version: pendingKey.version }) }}</p>
          <div class="border border-muted bg-elevated p-3">
            <p class="text-xs text-muted">{{ t('plugins.trust.key_id') }}</p>
            <p class="font-mono text-sm text-highlighted">{{ pendingKey.keyId }}</p>
            <p class="mt-3 text-xs text-muted">{{ t('plugins.trust.fingerprint') }}</p>
            <p class="break-all font-mono text-sm text-highlighted">{{ groupFingerprint(pendingKey.fingerprint) }}</p>
          </div>
          <UAlert color="warning" variant="subtle" :description="t('plugins.trust.warning')" />
        </div>
      </template>
      <template #footer>
        <div class="flex w-full justify-end gap-2">
          <UButton color="neutral" variant="ghost" :label="t('plugins.trust.cancel')" @click="pendingKey = null" />
          <UButton color="primary" icon="i-lucide-shield-check" :label="t('plugins.trust.confirm')" :loading="pending" @click="confirmKey" />
        </div>
      </template>
    </UModal>

    <UModal v-model:open="withdrawalOpen" :title="t('plugins.withdraw.title')">
      <template #body>
        <div v-if="pendingWithdrawal" class="space-y-4">
          <p class="text-sm leading-6 text-toned">{{ t('plugins.withdraw.intro', { name: pendingWithdrawal.name, version: pendingWithdrawal.version }) }}</p>
          <!-- The one sentence somebody has to read before they conclude the feature is broken:
               the package they just withdrew keeps running until the service restarts. -->
          <UAlert color="warning" variant="subtle" :description="t('plugins.withdraw.restart')" />
          <p class="text-sm leading-6 text-toned">{{ t('plugins.withdraw.key_untouched') }}</p>
          <UFormField :label="t('plugins.withdraw.reason_label')" :description="t('plugins.withdraw.reason_hint')">
            <UInput v-model="withdrawalReason" class="mt-2 w-full" :maxlength="200" />
          </UFormField>
        </div>
      </template>
      <template #footer>
        <div v-if="pendingWithdrawal" class="flex w-full justify-end gap-2">
          <UButton color="neutral" variant="ghost" :label="t('common.actions.cancel')" @click="pendingWithdrawal = null" />
          <UButton color="error" icon="i-lucide-shield-off" :label="t('plugins.withdraw.confirm')" :loading="withdrawing" @click="withdraw" />
        </div>
      </template>
    </UModal>
  </div>
</template>
