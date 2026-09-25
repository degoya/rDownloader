<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { NotificationDestination, NotificationTarget, NotificationTargetRequest } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useEditableList } from '@/composables/useEditableList'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFormFocus } from '@/composables/useFormFocus'
import { withPluginVersion } from '@/utils/pluginVersion'

const targets = defineModel<NotificationTarget[]>({ required: true })
const props = defineProps<{
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const emit = defineEmits<{ changed: [] }>()
const { t } = useI18n()
const testing = ref<string | null>(null)
const message = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const recipients = ref('')

function emptyForm(): NotificationTargetRequest {
  return { name: '', kind: 'webhook', enabled: true, endpoint: '', config: {}, secret: null, clear_secret: false }
}

const form = reactive<NotificationTargetRequest>(emptyForm())

const list = useEditableList<NotificationTarget, NotificationTargetRequest>({
  list: targets,
  create: body => api.POST('/api/v1/notifications/targets', { body }),
  update: (id, body) => api.PUT('/api/v1/notifications/targets/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/notifications/targets/{id}', { params: { path: { id } } }),
  reset: () => {
    recipients.value = ''
    Object.assign(form, emptyForm())
  },
  confirmDelete: target => ({
    title: t('notifications.target.delete_title'),
    description: t('notifications.target.delete_description', { name: target.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

/** Installed notification-destination plugins; empty unless at least one is installed. */
const destinations = ref<NotificationDestination[]>([])

/** The live subscription and the timer that coalesces a burst of plugin events into one read. */
let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

async function loadDestinations(): Promise<void> {
  const response = await api.GET('/api/v1/notifications/destinations')
  if (response.data) destinations.value = response.data
}

onMounted(() => {
  void loadDestinations()
  releaseEvents = subscribeEvents({ 'plugin_catalog.changed': scheduleReload })
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
 * What this list does when the bus says the installed plugins changed.
 *
 * The destinations come solely from installed notifier plugins, and they decide whether the
 * `plugin` kind is offered at all. Read once on mount, the form kept the kind hidden after a
 * notifier was installed elsewhere — the feature simply appeared not to exist — and kept
 * offering it after the last one was removed, which produces a target that fails on its first
 * delivery.
 *
 * The channel is `plugin_catalog.changed`, not `plugin.changed`: the list comes from
 * `/api/v1/notifications/destinations`, which costs `Config`, and an event is delivered only to
 * a subscriber holding that event's exact scope — `plugin.changed` is the same payload at
 * `Admin` and would never arrive here.
 *
 * Refetched rather than patched: the event names no destination, and the labels shown carry the
 * plugin's own name and version, which only the service knows. The read sets no loading flag,
 * so an arriving event cannot take the kind out of the picker and put it back while the form is
 * open; the option list is simply recomputed once the answer is in hand. Debounced, because
 * installing a package emits more than one event. No notice is raised — `design.md` has no
 * pattern for announcing that data caught up.
 */
function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void loadDestinations()
  }, 300)
}

/**
 * The `plugin` kind is offered only when something can serve it. A kind that always fails
 * because nothing is installed is worse than one that is not in the list.
 */
const kinds = computed(() => {
  const values = ['webhook', 'smtp', 'apprise', ...(destinations.value.length ? ['plugin'] : [])] as const
  return values.map(value => ({ value, label: t(`notifications.kind.${value}`) }))
})

const destinationItems = computed(() =>
  destinations.value.map(destination => ({
    value: destination.plugin_id,
    label: withPluginVersion(destination.name, destination.version)
  }))
)

const tlsModes = computed(() =>
  (['starttls', 'tls', 'none'] as const).map(value => ({ value, label: t(`notifications.smtp.tls_${value}`) }))
)

/** The SMTP details live in the free-form config blob; edit them as named fields. */
const config = computed<Record<string, unknown>>({
  get: () => (form.config ?? {}) as Record<string, unknown>,
  set: (value) => { form.config = value }
})

function setConfig(key: string, value: unknown): void {
  config.value = { ...config.value, [key]: value }
}

async function submit(): Promise<void> {
  message.value = null
  if (form.kind === 'smtp') {
    setConfig('to', recipients.value.split(',').map(entry => entry.trim()).filter(Boolean))
  }
  const saved = await list.submit({ ...form, secret: form.secret?.trim() || null })
  if (saved) emit('changed')
}

function edit(target: NotificationTarget): void {
  message.value = null
  list.edit(target)
  Object.assign(form, {
    name: target.name,
    kind: target.kind,
    enabled: target.enabled,
    endpoint: target.endpoint,
    config: target.config ?? {},
    // An omitted secret keeps the stored one; the field starts empty on purpose.
    secret: null,
    clear_secret: false
  })
  const to = (target.config as Record<string, unknown> | undefined)?.['to']
  recipients.value = Array.isArray(to) ? to.join(', ') : ''
  void focusForm()
}

async function test(target: NotificationTarget): Promise<void> {
  testing.value = target.id
  error.value = null
  message.value = null
  const response = await api.POST('/api/v1/notifications/targets/{id}/test', {
    params: { path: { id: target.id } }
  })
  testing.value = null
  if (!response.data) return void (error.value = responseError(response))
  if (response.data.ok) {
    message.value = t('notifications.target.test_ok', { name: target.name })
  } else {
    error.value = t('notifications.target.test_failed', {
      name: target.name,
      detail: response.data.detail ?? String(response.data.status ?? '')
    })
  }
}

async function remove(target: NotificationTarget): Promise<void> {
  if ((await list.remove(target)).removed) emit('changed')
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('notifications.target.title')" :count="targets.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('notifications.target.eyebrow')"
          :title="editingId ? t('notifications.target.form_edit') : t('notifications.target.form_new')"
        />
        <p class="mt-2 mb-4 text-xs leading-5 text-muted">{{ t('notifications.target.description') }}</p>
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />

        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('notifications.target.name_label')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" icon="i-lucide-bell" />
          </UFormField>
          <UFormField :label="t('notifications.target.kind_label')">
            <USelect v-model="form.kind" :items="kinds" value-key="value" class="w-full" />
          </UFormField>
          <UFormField :label="t(`notifications.target.endpoint_${form.kind}`)" :description="t(`notifications.target.endpoint_${form.kind}_description`)">
            <UInput v-model="form.endpoint" required class="w-full font-mono" icon="i-lucide-link" />
          </UFormField>
          <UFormField :label="t(`notifications.target.secret_${form.kind}`)" :description="editingId ? t('notifications.target.secret_keep') : t(`notifications.target.secret_${form.kind}_description`)">
            <UInput v-model="form.secret" type="password" class="w-full font-mono" autocomplete="new-password" />
          </UFormField>

          <UFormField
            v-if="form.kind === 'plugin'"
            :label="t('notifications.target.destination_label')"
            :description="t('notifications.target.destination_description')"
          >
            <USelect
              :model-value="String(config.plugin_id ?? '')"
              :items="destinationItems"
              value-key="value"
              class="w-full"
              data-testid="notification-destination"
              @update:model-value="setConfig('plugin_id', $event)"
            />
          </UFormField>

          <template v-if="form.kind === 'smtp'">
            <UFormField :label="t('notifications.smtp.from')">
              <UInput :model-value="String(config.from ?? '')" class="w-full font-mono" @update:model-value="setConfig('from', $event)" />
            </UFormField>
            <UFormField :label="t('notifications.smtp.to')" :description="t('notifications.smtp.to_description')">
              <UInput v-model="recipients" class="w-full font-mono" />
            </UFormField>
            <UFormField :label="t('notifications.smtp.username')">
              <UInput :model-value="String(config.username ?? '')" class="w-full font-mono" @update:model-value="setConfig('username', $event)" />
            </UFormField>
            <UFormField :label="t('notifications.smtp.tls')">
              <USelect :model-value="String(config.tls ?? 'starttls')" :items="tlsModes" value-key="value" class="w-full" @update:model-value="setConfig('tls', $event)" />
            </UFormField>
            <UFormField :label="t('notifications.smtp.port')" :description="t('notifications.smtp.port_description')">
              <UInput :model-value="config.port as number | undefined" type="number" min="1" max="65535" class="w-full" @update:model-value="setConfig('port', Number($event) || null)" />
            </UFormField>
          </template>

          <div class="flex items-center justify-between gap-5">
            <p class="text-sm text-highlighted">{{ t('notifications.target.enabled') }}</p>
            <USwitch v-model="form.enabled" :aria-label="t('notifications.target.enabled')" />
          </div>
          <div class="flex gap-2">
            <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-plus'" :label="editingId ? t('common.actions.save') : t('notifications.target.create')" :loading="pending" />
            <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('routing.cancel_edit')" @click="list.reset" />
          </div>
        </form>
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="target in targets" :key="target.id" class="flex items-center gap-3 p-3" :class="editingId === target.id ? 'border-l-2 border-l-primary' : ''">
            <UIcon name="i-lucide-send" class="text-primary" />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ target.name }}</p>
              <p class="truncate font-mono text-[11px] text-muted">{{ target.endpoint }}</p>
            </div>
            <UBadge v-if="editingId === target.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge color="neutral" variant="subtle">{{ t(`notifications.kind.${target.kind}`) }}</UBadge>
            <UBadge v-if="!target.enabled" color="neutral" variant="outline">{{ t('notifications.target.disabled') }}</UBadge>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-send-horizontal" :aria-label="t('notifications.target.test')" :loading="testing === target.id" @click="test(target)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(target)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" @click="remove(target)" />
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!targets.length" variant="inline" class="p-5">
            <p class="text-center text-sm text-muted">{{ t('notifications.target.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
