<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type {
  CreateUsenetServer,
  ProxyProfile,
  UpdateUsenetServer,
  UsenetServer
} from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import SectionHeader from '@/components/SectionHeader.vue'

/** Gap between generated priorities so manual values keep room in between. */
const PRIORITY_STEP = 10

/** The setup wizard embeds this tab under its own step heading. */
defineProps<{ hideHeader?: boolean }>()

const { t } = useI18n()
const servers = ref<UsenetServer[]>([])
const proxies = ref<ProxyProfile[]>([])
/** The server chain's own fetch; the form's own `pending` comes from the list (RD-104-07). */
const { loading, loadError, load } = useFetchState()
const testingId = ref<string | null>(null)
const deletingId = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const reorderingId = ref<string | null>(null)
const clearPassword = ref(false)
const message = ref<string | null>(null)
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
    api.GET('/api/v1/proxy-profiles')
  ])
  if (proxyResponse.data) proxies.value = proxyResponse.data
  if (!serverResponse.data) return responseError(serverResponse)
  servers.value = sortByPriority(serverResponse.data)
  return null
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
  <div class="w-full space-y-6">
    <header v-if="!hideHeader">
      <SectionHeader
        :eyebrow="t('usenet.header.eyebrow')"
        :title="t('usenet.header.title')"
        :description="t('usenet.header.description')"
        level="page"
      />
    </header>
    <UAlert v-if="error" color="error" variant="subtle" :description="error" />
    <UAlert v-if="message" color="success" variant="subtle" :description="message" />

    <FormListLayout>
      <template #form>
        <section class="border border-muted bg-default p-5">
          <SectionHeader :eyebrow="t('usenet.form.eyebrow')" :title="editingId ? t('usenet.form.title_edit') : t('usenet.form.title_add')" />
          <form ref="formElement" class="mt-4 grid gap-3" @submit.prevent="createServer">
            <UFormField :label="t('usenet.form.server_name')" name="name" required>
              <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('usenet.form.name')" />
            </UFormField>
            <UFormField :label="t('usenet.form.host')" name="host" required>
              <UInput v-model="form.host" required class="w-full font-mono" placeholder="news.provider.example" />
            </UFormField>
            <UFormField :label="t('usenet.form.port')" name="port" required>
              <UInput v-model.number="form.port" required type="number" min="1" max="65535" class="w-full" />
            </UFormField>
            <UFormField :label="t('usenet.form.connections')" name="max_connections" :description="t('usenet.form.connections_hint')">
              <UInput v-model.number="form.max_connections" type="number" min="1" max="32" class="w-full" />
            </UFormField>
            <UFormField :label="t('usenet.form.username')" name="username">
              <UInput v-model="form.username" class="w-full" autocomplete="username" />
            </UFormField>
            <UFormField :label="t('usenet.form.password')" name="password">
              <UInput v-model="form.password" type="password" class="w-full" :placeholder="editingId ? t('usenet.form.password_keep') : ''" autocomplete="new-password" />
            </UFormField>
            <UFormField :label="t('usenet.form.proxy')" name="proxy">
              <USelect v-model="proxySelection" :items="proxyItems" class="w-full" />
            </UFormField>
            <label class="flex items-center gap-3 text-sm text-muted"><USwitch v-model="form.tls" /> {{ t('usenet.form.tls') }}</label>
            <label class="flex items-center gap-3 text-sm text-muted"><USwitch v-model="form.enabled" /> {{ t('usenet.form.enabled') }}</label>
            <label v-if="editingId" class="flex items-center gap-3 text-xs text-muted"><USwitch v-model="clearPassword" /> {{ t('usenet.form.clear_password') }}</label>
            <div class="flex gap-2">
              <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-server-cog'" :label="editingId ? t('usenet.form.save_changes') : t('usenet.form.save_server')" :loading="pending" />
              <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :aria-label="t('common.actions.cancel')" @click="list.reset" />
            </div>
          </form>
        </section>
      </template>

      <template #list>
        <section class="border border-muted bg-default p-5">
          <div class="mb-4 flex items-start justify-between gap-4">
            <div>
              <SectionHeader :eyebrow="t('usenet.chain.eyebrow')" :title="t('usenet.chain.title')" />
              <p v-if="servers.length" class="mt-2 max-w-2xl text-xs leading-5 text-muted">{{ t('usenet.chain.description') }}</p>
            </div>
            <UBadge color="neutral" variant="outline">{{ servers.length }}</UBadge>
          </div>
          <div class="space-y-2">
            <article v-for="(server, index) in servers" :key="server.id" class="border p-4" :class="editingId === server.id ? 'border-primary' : 'border-muted'">
              <div class="flex items-start gap-4">
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
                  <span class="numeric grid size-9 place-items-center bg-elevated text-sm text-primary" :title="t('usenet.chain.priority', { priority: server.priority })">{{ index + 1 }}</span>
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
                <div class="min-w-0 flex-1">
                  <div class="flex items-center gap-2"><span class="size-2" :class="server.enabled ? 'bg-success' : 'bg-muted'" /><h4 class="truncate text-sm font-semibold text-highlighted">{{ server.name }}</h4></div>
                  <p class="mt-1 truncate font-mono text-xs text-muted">{{ server.host }}:{{ server.port }}</p>
                  <p class="mt-2 text-xs text-muted">{{ t('usenet.summary.connections', { count: server.max_connections }, server.max_connections) }} · {{ proxyName(server.proxy_profile_id) }} · {{ t('usenet.chain.priority', { priority: server.priority }) }}</p>
                </div>
                <div class="flex items-center gap-2">
                  <UBadge v-if="editingId === server.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                  <UBadge :color="server.tls ? 'success' : 'warning'" variant="subtle">{{ server.tls ? 'TLS' : 'PLAIN' }}</UBadge>
                  <UIcon v-if="server.has_password" name="i-lucide-key-round" class="text-primary" />
                  <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('common.actions.test')" :loading="testingId === server.id" @click="testServer(server.id)" />
                  <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('usenet.chain.edit')" @click="editServer(server)" />
                  <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('usenet.chain.delete')" :loading="deletingId === server.id" @click="deleteServer(server)" />
                </div>
              </div>
              <div class="transfer-stripe mt-4 h-1" :class="reorderingId === server.id ? 'animate-pulse opacity-80' : 'opacity-40'" />
            </article>
            <DataState :loading="loading" :error="loadError" :empty="!servers.length" :rows="2">
              <p class="signal-grid border border-dashed border-muted p-10 text-center text-sm text-muted">{{ t('usenet.chain.empty') }}</p>
            </DataState>
          </div>
        </section>
      </template>
    </FormListLayout>
  </div>
</template>
