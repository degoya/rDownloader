<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import { listOffers, releaseNotesByPlugin, type PreviewSource, type ReleaseNote } from '@/api/pluginRepositories'
import type { IncompatiblePlugin, InstalledPlugin, PluginLifecycle } from '@/api/types'
import { serverMessageFrom, translateServerMessage } from '@/i18n/server'
import DataState from '@/components/DataState.vue'
import { useConfirm } from '@/composables/useConfirm'
import { usePluginDiagnostics } from '@/composables/usePluginDiagnostics'
import { usePluginGroups } from '@/composables/usePluginGroups'
import { usePluginSupersededRemoval } from '@/composables/usePluginSupersededRemoval'
import { usePluginWithdrawals } from '@/composables/usePluginWithdrawals'
import { useFetchState } from '@/composables/useFetchState'
import { useRestartAction, useRestartStatus } from '@/composables/useRestartStatus'
import { subTabItems } from '@/composables/useSettingsSubTab'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useSettingsStore } from '@/stores/settings'
import SectionHeader from '@/components/SectionHeader.vue'
import PluginCard from './PluginCard.vue'
import PluginBundledList from './PluginBundledList.vue'
import PluginIncompatibleList from './PluginIncompatibleList.vue'
import PluginInstallPreviewModal from './PluginInstallPreviewModal.vue'
import PluginTrustedKeys from './PluginTrustedKeys.vue'
import PluginUpdatesList from './PluginUpdatesList.vue'
import PluginWithdrawDialog from './PluginWithdrawDialog.vue'
import PluginWithdrawnList from './PluginWithdrawnList.vue'
import SettingsPluginRepositories from './SettingsPluginRepositories.vue'
import { displayName, type TrustedKey } from './pluginDisplay'

/**
 * Owned by the settings view, which keeps it in the address (RD-180-15). Nine cards on one page
 * had become a long scroll to the one that was wanted, so the page is five tabs: what is
 * installed, what can be added, the updates, where packages come from, and whom this machine
 * trusts. The default is for a mount without the view, as in the tests.
 */
const activeTab = defineModel<string>('subTab', { default: 'installed' })
const { t } = useI18n()
const plugins = ref<InstalledPlugin[]>([])
const incompatible = ref<IncompatiblePlugin[]>([])
/** Per plugin id: which version runs, which is under test, how updates arrive (RD-140-02). */
const lifecycles = ref<PluginLifecycle[]>([])
/** The switch for all plugins (RD-191-10): every card's own switch then reads on and locked. */
const automaticForAll = ref(false)
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

const { typeTab, pluginGroups, typeGroups, visibleGroups, openSuperseded, toggleSuperseded } = usePluginGroups(plugins)
/** The bundle's available services, re-read when the installed set changes elsewhere. */
const bundledList = ref<InstanceType<typeof PluginBundledList> | null>(null)
/** Only for the count in the updates tab's badge; the list reads its offers itself. */
const updatesList = ref<InstanceType<typeof PluginUpdatesList> | null>(null)
/** The installed plugins and the waiting updates are counted in the badges, as on the routing page. */
const tabItems = computed(() => subTabItems('plugins', t, {
  installed: pluginGroups.value.length,
  updates: updatesList.value?.updateCount || undefined
}))
const trustedKeys = ref<TrustedKey[]>([])
const packageFile = ref<File | null>(null)
/** The package the install preview shows; the upload installs only from there (RD-140-01). */
const previewSource = ref<PreviewSource | null>(null)
const message = ref<string | null>(null)
const error = ref<string | null>(null)
const confirm = useConfirm()
/**
 * Most plugin changes run only from the next start (RD-1240-32): after any answer the restart
 * status is read again, and while one is pending the answer offers "Restart now".
 */
const { status: restartStatus, load: loadRestart } = useRestartStatus()
const { action: restartAction } = useRestartAction()
const messageActions = computed(() => (restartStatus.value?.pending ? [restartAction()] : undefined))
watch(message, (text) => { if (text) void loadRestart() })
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
const { removeSuperseded } = usePluginSupersededRemoval({ message, error, done: refresh })
/** Every plugin's superseded versions, for the tab's action that removes them all (RD-1140-04). */
const supersededCount = computed(() => pluginGroups.value.reduce((sum, group) => sum + group.superseded.length, 0))

onMounted(() => {
  void inventoryState.load(refresh)
  void keyState.load(refreshKeys)
  void revocationState.load(refreshRevocations)
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
useDebouncedEventRefresh(['plugin.changed', 'plugin_trust.changed'], reloadFromEvent)

async function reloadFromEvent(): Promise<void> {
  // In parallel and without short-circuiting: a failed key read must not stop the withdrawals
  // from being re-read, or one stale list would keep the other one stale too.
  const [, keyFailure, revocationFailure] = await Promise.all([
    refresh(),
    refreshKeys(),
    refreshRevocations(),
    bundledList.value?.reload()
  ])
  const failure = keyFailure ?? revocationFailure
  if (failure) error.value = failure
}

async function refresh(): Promise<string | null> {
  const [inventory, settings] = await Promise.all([
    api.GET('/api/v1/plugins'),
    useSettingsStore().fetchSettings()
  ])
  if (inventory.data) {
    plugins.value = inventory.data.installed
    incompatible.value = inventory.data.incompatible
    lifecycles.value = inventory.data.lifecycle ?? []
    automaticForAll.value = inventory.data.automatic_updates_global === true
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
  const response = await api.DELETE('/api/v1/plugins/{id}/{version}', {
    params: { path: { id: plugin.id, version: plugin.version } }
  })
  showAnswer(response)
  await refresh()
}

/**
 * The coded message of a removal's answer, in the success or the error line. Through the client
 * like every request (WEB-03): a raw `fetch` here noticed neither a lapsed session nor a dropped
 * connection.
 */
function showAnswer(response: { data?: unknown, error?: unknown }): void {
  const serverMessage = serverMessageFrom(response.data ?? response.error)
  const text = serverMessage ? translateServerMessage(serverMessage) : null
  if (response.data) message.value = text
  else error.value = text
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
  const response = await api.GET('/api/v1/plugins/keys')
  // A failed key fetch used to leave "no trusted keys" standing, which is the one claim a
  // trust store must never make wrongly.
  if (!response.data) return t('common.data.load_failed')
  trustedKeys.value = response.data
  return null
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
  const response = await api.DELETE('/api/v1/plugins/keys/{key_id}', { params: { path: { key_id: keyId } } })
  showAnswer(response)
  await refreshKeys()
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

    <!-- Above the tabs: removing, switching off and withdrawing answer here, whichever tab they came from. -->
    <UAlert v-if="message" color="success" :description="message" :actions="messageActions" data-testid="plugins-message" />
    <UAlert v-if="error" color="error" :description="error" />

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #installed>
        <div class="space-y-6">
          <UCard as="section" data-settings-anchor="plugins.installed">
            <div class="mb-4 flex items-center gap-2">
              <SectionHeader :eyebrow="t('plugins.installed.eyebrow')" :title="t('plugins.installed.title')" level="sub" />
              <!-- Only while there is something to remove; icon-only on a phone, the name in aria-label. -->
              <UButton
                v-if="supersededCount"
                size="sm"
                color="error"
                variant="ghost"
                icon="i-lucide-trash-2"
                class="ml-auto"
                :aria-label="t('plugins.actions.remove_all_superseded', { count: supersededCount })"
                :title="t('plugins.actions.remove_all_superseded', { count: supersededCount })"
                @click="removeSuperseded({ count: supersededCount })"
              >
                <span class="hidden sm:inline">{{ t('plugins.actions.remove_all_superseded', { count: supersededCount }) }}</span>
              </UButton>
              <UBadge color="neutral" variant="outline" :class="supersededCount ? '' : 'ml-auto'">{{ pluginGroups.length }}</UBadge>
            </div>
            <URadioGroup
              v-if="pluginGroups.length"
              v-model="typeTab"
              class="mb-4"
              :items="typeGroups"
              variant="card"
              indicator="hidden"
              orientation="horizontal"
              size="xs"
              :aria-label="t('plugins.installed.filter_label')"
            >
              <template #label="{ item }">
                {{ item.label }}<UBadge size="xs" color="neutral" variant="subtle" class="ms-1.5 font-mono">{{ item.count }}</UBadge>
              </template>
            </URadioGroup>
            <div class="grid gap-3 md:grid-cols-2">
              <PluginCard
                v-for="{ plugin, superseded } in visibleGroups"
                :key="plugin.id"
                :plugin="plugin"
                :superseded="superseded"
                :disabled="isDisabled(plugin)"
                :is-withdrawn="isWithdrawn"
                :lifecycle="lifecycleOf(plugin.id)"
                :automatic-for-all="automaticForAll"
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
                @remove-all-superseded="removeSuperseded({ count: superseded.length, plugin: { id: plugin.id, name: displayName(plugin) } })"
                @version-done="versionActionDone"
              />
              <DataState :loading="inventoryState.loading.value" :error="inventoryState.loadError.value" :empty="!visibleGroups.length" class="md:col-span-2">
                <UEmpty :description="t('plugins.installed.empty')" />
              </DataState>
            </div>
          </UCard>

          <PluginIncompatibleList v-if="incompatible.length" :incompatible="incompatible" @remove="removeVersion" />
        </div>
      </template>

      <template #add>
        <div class="space-y-6">
          <UCard as="section">
            <form class="flex flex-col gap-3 sm:flex-row sm:items-end" @submit.prevent="previewUpload()">
              <UFormField data-settings-anchor="plugins.install" class="flex-1" :label="t('plugins.install.label')" :description="t('plugins.install.hint')">
                <!--
                  The extension alone: a MIME type here becomes the drop zone's only allowed type,
                  and a browser reports a .rdplug with none, so a dropped package was refused. The
                  service checks what it is sent (RA-WEB-01).
                -->
                <UFileUpload
                  v-model="packageFile"
                  accept=".rdplug"
                  icon="i-lucide-package"
                  :label="t('plugins.install.drop')"
                  layout="list"
                  class="mt-2 w-full"
                  data-testid="plugin-package"
                />
              </UFormField>
              <UButton type="submit" icon="i-lucide-package-plus" :label="t('plugins.install.submit')" :disabled="!packageFile" />
            </form>
          </UCard>

          <PluginBundledList ref="bundledList" @installed="onInstalled" />
        </div>
      </template>

      <template #updates>
        <PluginUpdatesList ref="updatesList" @installed="onInstalled" @automatic-changed="value => (automaticForAll = value)" />
      </template>

      <template #repositories>
        <SettingsPluginRepositories />
      </template>

      <template #trust>
        <div class="space-y-6">
          <PluginWithdrawnList
            :revocations="revocations"
            :installed="plugins"
            :loading="revocationState.loading.value"
            :load-error="revocationState.loadError.value"
            @lift="liftWithdrawal"
          />

          <PluginTrustedKeys
            :keys="trustedKeys"
            :loading="keyState.loading.value"
            :load-error="keyState.loadError.value"
            @revoke="revokeKey"
          />
        </div>
      </template>
    </UTabs>

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
