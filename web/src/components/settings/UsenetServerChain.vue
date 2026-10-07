<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch, watchEffect } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type {
  CreateUsenetServer,
  ProxyProfile,
  UpdateUsenetServer,
  UsenetServer,
  UsenetServerTraffic
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { PLAIN, WHOLE } from '@/utils/numberInput'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import SectionHeader from '@/components/SectionHeader.vue'
import UsenetQuotaEditor from '@/components/settings/UsenetQuotaEditor.vue'
import { editingRowClass } from '@/utils/editingRow'
import FormFeedback from '@/components/FormFeedback.vue'

/*
 * The Usenet server chain: the form, the servers in priority order and their quotas. The
 * *Servers* tab of the Usenet page and the setup wizard both show it (RD-1120-23).
 */

/** Gap between generated priorities so manual values keep room in between. */
const PRIORITY_STEP = 10
/** Matches `validate_name` in `crates/rd-api-queue/src/usenet_handlers.rs`. */
const MAX_SERVER_NAME = 100

/**
 * The server chain as loaded, for the NNTP limits measured against it (RD-1120-21); null while it
 * loads or when it could not be loaded. The setup wizard does not read it.
 */
const loadedChain = defineModel<UsenetServer[] | null>('loaded', { default: null })

const { t } = useI18n()
const servers = ref<UsenetServer[]>([])
const proxies = ref<ProxyProfile[]>([])
/** What each server delivered, by id (RD-1100-05); a server without figures shows none. */
const traffic = ref<Record<string, UsenetServerTraffic>>({})
/** The server chain's own fetch; the form's own `pending` comes from the list (RD-104-07). */
const { loading, loadError, load } = useFetchState()
const testingId = ref<string | null>(null)
const deletingId = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const reorderingId = ref<string | null>(null)
const clearPassword = ref(false)
const message = ref<string | null>(null)
const copyName = useCopyName()
/**
 * The server the form was filled from while that copy is still unsaved (RD-150-12). The copy
 * carries no password — the browser never holds one — so the form asks for it again.
 */
const copiedFrom = ref<string | null>(null)
const form = reactive<CreateUsenetServer>({
  name: '',
  host: '',
  port: 563,
  tls: true,
  username: null,
  password: null,
  proxy_profile_id: null,
  priority: PRIORITY_STEP,
  max_connections: 8,
  enabled: true
})

const proxyItems = computed(() => [
  { label: t('usenet.form.proxy_direct'), value: NO_SELECTION },
  ...proxies.value
    .filter(proxy => proxy.kind === 'socks5')
    .map(proxy => ({ label: proxy.name, value: proxy.id }))
])
const proxySelection = computed({
  get: () => optionalSelection(form.proxy_profile_id),
  set: (value: string) => { form.proxy_profile_id = selectionValue(value) }
})

watch(() => form.tls, tls => {
  if (form.port === 563 || form.port === 119) form.port = tls ? 563 : 119
})

onMounted(() => void load(refresh))
watchEffect(() => {
  loadedChain.value = loading.value || loadError.value ? null : servers.value
})

function sortByPriority(rows: UsenetServer[]): UsenetServer[] {
  return [...rows].sort((left, right) => left.priority - right.priority)
}

/** Priority appended after the current chain, so a new server lands last. */
const list = useEditableList<UsenetServer, CreateUsenetServer>({
  list: servers,
  create: body => api.POST('/api/v1/usenet/servers', { body }),
  // Only an update can clear the stored password; a create has none to clear.
  update: (id, body) => api.PUT('/api/v1/usenet/servers/{id}', {
    params: { path: { id } },
    body: { ...body, clear_password: clearPassword.value } satisfies UpdateUsenetServer
  }),
  destroy: id => api.DELETE('/api/v1/usenet/servers/{id}', { params: { path: { id } } }),
  reset: () => {
    form.name = ''
    form.host = ''
    form.tls = true
    form.port = 563
    form.username = null
    form.password = null
    form.proxy_profile_id = null
    form.priority = PRIORITY_STEP
    form.max_connections = 8
    form.enabled = true
    clearPassword.value = false
    copiedFrom.value = null
  },
  confirmDelete: server => ({
    title: t('usenet.delete.title'),
    description: t('usenet.delete.description', { name: server.name }),
    confirmLabel: t('usenet.delete.confirm'),
    confirmIcon: 'i-lucide-server-off',
    destructive: true
  })
})
const { editingId, pending, error } = list

function nextPriority(): number {
  return servers.value.reduce((highest, server) => Math.max(highest, server.priority), 0) + PRIORITY_STEP
}

async function refresh(): Promise<string | null> {
  const [serverResponse, proxyResponse] = await Promise.all([
    api.GET('/api/v1/usenet/servers'),
    api.GET('/api/v1/proxy-profiles'),
    refreshTraffic()
  ])
  if (proxyResponse.data) proxies.value = proxyResponse.data
  if (!serverResponse.data) return responseError(serverResponse)
  servers.value = sortByPriority(serverResponse.data)
  return null
}

/** The usage figures are an addition to the chain; failing to read them leaves the chain alone. */
async function refreshTraffic(): Promise<void> {
  const response = await api.GET('/api/v1/stats/usenet-servers')
  if (response.data) traffic.value = Object.fromEntries(response.data.servers.map(entry => [entry.server_id, entry]))
}

function quotaSaved(saved: UsenetServer): void {
  servers.value = servers.value.map(server => (server.id === saved.id ? saved : server))
  error.value = null
  message.value = t('usenet.quota.saved')
  void refreshTraffic()
}

async function createServer(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({
    ...form,
    username: form.username || null,
    password: form.password || null,
    proxy_profile_id: form.proxy_profile_id || null,
    priority: updating ? form.priority : nextPriority()
  })
  if (!saved) return
  // The chain is read in priority order, and a new server joins at the end of it.
  servers.value = sortByPriority(servers.value)
  message.value = updating ? t('usenet.messages.updated') : t('usenet.messages.created')
}

function editServer(server: UsenetServer): void {
  message.value = null
  list.edit(server)
  form.name = server.name
  form.host = server.host
  form.port = server.port
  form.tls = server.tls
  form.username = server.username ?? null
  form.password = null
  form.proxy_profile_id = server.proxy_profile_id ?? null
  form.priority = server.priority
  form.max_connections = server.max_connections
  form.enabled = server.enabled
  clearPassword.value = false
  void focusForm()
}

/**
 * Fills the form with a copy of `server`, unsaved (RD-150-12).
 *
 * A duplicate is otherwise a create through the existing route, but this one cannot be: the
 * server takes a username only together with its password, and the password is never sent to
 * the browser, so a copy of a server that signs in would be refused. The form therefore holds
 * the copy — every setting, a free copy name, the username, an empty password with a sentence
 * that says so — and the reader's own save creates it. Priority and position in the chain are
 * the original's and stay with it; the copy joins at the end like any new server.
 */
function duplicateServer(server: UsenetServer): void {
  message.value = null
  list.reset()
  form.name = copyName(server.name, servers.value.map(entry => entry.name), MAX_SERVER_NAME)
  form.host = server.host
  form.port = server.port
  form.tls = server.tls
  form.username = server.username ?? null
  form.password = null
  form.proxy_profile_id = server.proxy_profile_id ?? null
  form.max_connections = server.max_connections
  form.enabled = server.enabled
  copiedFrom.value = server.name
  void focusForm()
}

/** Full update payload that keeps the stored password (no `password` field sent). */
function reorderBody(server: UsenetServer, priority: number): UpdateUsenetServer {
  return {
    name: server.name,
    host: server.host,
    port: server.port,
    tls: server.tls,
    username: server.username ?? null,
    proxy_profile_id: server.proxy_profile_id ?? null,
    priority,
    max_connections: server.max_connections,
    enabled: server.enabled,
    clear_password: false
  }
}

/** Moves a server one slot up or down the fallback chain by rewriting priorities. */
async function moveServer(index: number, delta: number): Promise<void> {
  const target = index + delta
  const moved = servers.value[index]
  if (!moved || target < 0 || target >= servers.value.length) return
  const ordered = [...servers.value]
  ordered.splice(index, 1)
  ordered.splice(target, 0, moved)
  reorderingId.value = moved.id
  error.value = null
  message.value = null
  for (const [position, server] of ordered.entries()) {
    const priority = (position + 1) * PRIORITY_STEP
    if (server.priority === priority) continue
    const response = await api.PUT('/api/v1/usenet/servers/{id}', {
      params: { path: { id: server.id } },
      body: reorderBody(server, priority)
    })
    if (!response.data) {
      reorderingId.value = null
      error.value = responseError(response)
      await refresh()
      return
    }
  }
  reorderingId.value = null
  await refresh()
  if (editingId.value === moved.id) form.priority = servers.value.find(server => server.id === moved.id)?.priority ?? form.priority
  message.value = t('usenet.messages.reordered')
}

async function testServer(id: string): Promise<void> {
  testingId.value = id
  message.value = null
  error.value = null
  const response = await api.POST('/api/v1/usenet/servers/{id}/test', {
    params: { path: { id } }
  })
  testingId.value = null
  if (response.data) message.value = resultMessage(response.data)
  else error.value = responseError(response)
}

async function deleteServer(server: UsenetServer): Promise<void> {
  deletingId.value = server.id
  message.value = null
  const { removed, body } = await list.remove(server)
  deletingId.value = null
  if (removed) message.value = resultMessage(body)
}

function proxyName(id: string | null | undefined): string {
  return proxies.value.find(proxy => proxy.id === id)?.name ?? t('usenet.chain.direct')
}
</script>

<template>
  <FormListLayout>
    <template #form>
      <UCard as="section" data-settings-anchor="usenet.server">
        <SectionHeader :eyebrow="t('usenet.form.eyebrow')" :title="editingId ? t('usenet.form.title_edit') : t('usenet.form.title_add')" />
        <FormFeedback class="mt-4" :error="error" :message="message" />
        <form ref="formElement" class="mt-4 grid gap-3" @submit.prevent="createServer">
          <UFormField :label="t('usenet.form.server_name')" name="name" required>
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('usenet.form.name')" />
          </UFormField>
          <UFormField :label="t('usenet.form.host')" name="host" required>
            <UInput v-model="form.host" required class="w-full font-mono" placeholder="news.provider.example" />
          </UFormField>
          <!-- TLS moves the port between 563 and 119, so it stands before it (RD-150-11). -->
          <USwitch v-model="form.tls" :label="t('usenet.form.tls')" />
          <UFormField :label="t('usenet.form.port')" name="port" required>
            <UInputNumber v-model="form.port" required :min="1" :max="65535" :format-options="PLAIN" class="w-full" />
          </UFormField>
          <UFormField data-settings-anchor="usenet.connections" :label="t('usenet.form.connections')" name="max_connections" :description="t('usenet.form.connections_hint')" required>
            <UInputNumber v-model="form.max_connections" required :min="1" :max="32" :format-options="WHOLE" class="w-full" />
          </UFormField>
          <UFormField :label="t('usenet.form.username')" name="username">
            <UInput v-model="form.username" class="w-full" autocomplete="username" />
          </UFormField>
          <UFormField
            :label="t('usenet.form.password')"
            name="password"
            :description="copiedFrom && form.username ? t('usenet.form.password_copy', { name: copiedFrom }) : undefined"
            :required="Boolean(copiedFrom && form.username)"
          >
            <UInput v-model="form.password" type="password" class="w-full" :placeholder="editingId ? t('usenet.form.password_keep') : ''" autocomplete="new-password" />
          </UFormField>
          <USwitch v-if="editingId" v-model="clearPassword" size="sm" :label="t('usenet.form.clear_password')" />
          <UFormField :label="t('usenet.form.proxy')" name="proxy">
            <USelect v-model="proxySelection" :items="proxyItems" class="w-full" />
          </UFormField>
          <USwitch v-model="form.enabled" :label="t('usenet.form.enabled')" />
          <FormActions
            :editing="editingId !== null"
            :create-label="t('usenet.form.create_server')"
            create-icon="i-lucide-server-cog"
            :save-label="t('usenet.form.save_changes')"
            :loading="pending"
            @cancel="list.reset"
          />
        </form>
      </UCard>
    </template>

    <template #list>
      <UCard as="section" data-settings-anchor="usenet.chain">
        <div class="mb-4 flex items-start justify-between gap-4">
          <div>
            <SectionHeader :eyebrow="t('usenet.chain.eyebrow')" :title="t('usenet.chain.title')" />
            <p v-if="servers.length" class="mt-2 max-w-2xl text-xs leading-5 text-muted">{{ t('usenet.chain.description') }}</p>
          </div>
          <UBadge color="neutral" variant="outline">{{ servers.length }}</UBadge>
        </div>
        <div class="space-y-2">
          <article v-for="(server, index) in servers" :key="server.id" class="min-w-0 p-4" :class="editingRowClass(editingId === server.id)">
            <!-- Wraps in a narrow column (the setup wizard), like the account rows. -->
            <div class="flex flex-wrap items-start gap-4">
              <div class="flex shrink-0 flex-col items-center gap-1">
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-chevron-up"
                  :disabled="index === 0 || reorderingId !== null"
                  :aria-label="t('usenet.chain.move_up')"
                  :title="t('usenet.chain.move_up')"
                  @click="moveServer(index, -1)"
                />
                <UAvatar :text="String(index + 1)" color="primary" size="lg" class="numeric text-sm" :title="t('usenet.chain.priority', { priority: server.priority })" />
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-chevron-down"
                  :disabled="index === servers.length - 1 || reorderingId !== null"
                  :aria-label="t('usenet.chain.move_down')"
                  :title="t('usenet.chain.move_down')"
                  @click="moveServer(index, 1)"
                />
              </div>
              <div class="min-w-0 flex-1 basis-40">
                <div class="flex items-center gap-2"><UChip standalone color="success" :show="server.enabled" class="w-2" /><h4 class="truncate text-sm font-semibold text-highlighted">{{ server.name }}</h4></div>
                <p class="mt-1 truncate font-mono text-xs text-muted">{{ server.host }}:{{ server.port }}</p>
                <p class="mt-2 text-xs text-muted">{{ t('usenet.summary.connections', { count: server.max_connections }, server.max_connections) }} · {{ proxyName(server.proxy_profile_id) }} · {{ t('usenet.chain.priority', { priority: server.priority }) }}</p>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <UBadge v-if="editingId === server.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                <UBadge v-if="!server.enabled" color="neutral" variant="subtle">{{ t('usenet.chain.disabled') }}</UBadge>
                <UBadge :color="server.tls ? 'success' : 'warning'" variant="subtle">{{ server.tls ? 'TLS' : 'PLAIN' }}</UBadge>
                <UIcon v-if="server.has_password" name="i-lucide-key-round" class="text-primary" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('common.actions.test')" :loading="testingId === server.id" @click="testServer(server.id)" />
                <UButton
                  size="xs"
                  color="neutral"
                  variant="ghost"
                  icon="i-lucide-copy-plus"
                  :label="t('common.actions.duplicate')"
                  :title="t('common.duplicate_hint')"
                  @click="duplicateServer(server)"
                />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('usenet.chain.edit')" :title="t('usenet.chain.edit')" @click="editServer(server)" />
                <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('usenet.chain.delete')" :title="t('usenet.chain.delete')" :loading="deletingId === server.id" @click="deleteServer(server)" />
              </div>
            </div>
            <UsenetQuotaEditor :server="server" :traffic="traffic[server.id]" @saved="quotaSaved" />
            <div class="transfer-stripe mt-4 h-1" :class="reorderingId === server.id ? 'animate-pulse opacity-80' : 'opacity-40'" />
          </article>
          <DataState :loading="loading" :error="loadError" :empty="!servers.length" :rows="2">
            <UEmpty class="signal-grid" :description="t('usenet.chain.empty')" />
          </DataState>
        </div>
      </UCard>
    </template>
  </FormListLayout>
</template>
