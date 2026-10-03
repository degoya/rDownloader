<script setup lang="ts">
/**
 * The Newznab indexers, defined once (RD-180-19): a name, the API base address and the key.
 *
 * The LinkGrabber searches them, and an indexer subscription can take one over. The key goes
 * into the vault the moment it is saved and is never read back: a row only says whether one is
 * stored, and an edit that leaves the field empty keeps it. The test is `t=caps` with the stored
 * key — the cheapest request that proves address and key — and it is also where the category
 * list for the defaults comes from.
 *
 * The list style (RD-190-16) is chosen here, where the indexer is defined, and not in the result
 * list: whether an indexer's hits are worth a cover and a line of metadata depends on what that
 * indexer sends, which does not change from one search to the next.
 */
import { storeToRefs } from 'pinia'
import { computed, onMounted, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { Indexer, IndexerCaps, IndexerCategory, IndexerRequest } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { useIndexersStore } from '@/stores/indexers'

/** `MAX_NAME` in `crates/rd-api-intake/src/indexer_handlers.rs`. */
const MAX_NAME = 200

type ListStyle = NonNullable<Indexer['list_style']>

const { t } = useI18n()
const store = useIndexersStore()
const { indexers } = storeToRefs(store)
const { loading, loadError, load } = useFetchState()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const message = ref<string | null>(null)
const testingId = ref<string | null>(null)
const deletingId = ref<string | null>(null)
/** The category tree of the indexer in the form, once it was asked for. */
const caps = ref<IndexerCaps | null>(null)
const capsBusy = ref(false)

const form = reactive({
  name: '',
  url: '',
  apiKey: '',
  categories: [] as string[],
  enabled: true,
  listStyle: 'compact' as ListStyle
})

const list = useEditableList<Indexer, IndexerRequest>({
  list: indexers,
  create: body => api.POST('/api/v1/indexers', { body }),
  update: (id, body) => api.PUT('/api/v1/indexers/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/indexers/{id}', { params: { path: { id } } }),
  reset: () => {
    form.name = ''
    form.url = ''
    form.apiKey = ''
    form.categories = []
    form.enabled = true
    form.listStyle = 'compact'
    caps.value = null
  },
  confirmDelete: indexer => ({
    title: t('usenet.indexers.delete.title'),
    description: t('usenet.indexers.delete.description', { name: indexer.name }),
    confirmLabel: t('usenet.indexers.delete.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

onMounted(() => void load(() => store.refresh()))

/** Asking needs an address and a key: the stored one when editing, the typed one otherwise. */
const canLoadCategories = computed(() =>
  form.url.trim().length > 0 && (editingId.value !== null || form.apiKey.trim().length > 0)
)

function categoryLabel(category: IndexerCategory): string {
  const parent = caps.value?.categories.find(entry => entry.id === category.parent_id)
  return `${category.id} · ${parent ? `${parent.name} / ${category.name}` : category.name}`
}

const listStyleItems = computed(() => (['compact', 'detailed'] as const).map(value => ({
  value,
  label: t(`usenet.indexers.list_styles.${value}`),
  description: t(`usenet.indexers.list_styles.${value}_hint`)
})))

const categoryItems = computed(() => (caps.value?.categories ?? []).map(category => ({ value: category.id, label: categoryLabel(category) })))

function body(): IndexerRequest {
  return {
    name: form.name.trim(),
    url: form.url.trim(),
    // Omitted rather than cleared when left blank: an edit that does not retype the key keeps it.
    api_key: form.apiKey.trim() || null,
    categories: form.categories.map(entry => entry.trim()).filter(Boolean),
    enabled: form.enabled,
    list_style: form.listStyle
  }
}

async function save(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit(body())
  if (!saved) return
  message.value = updating ? t('usenet.indexers.messages.updated') : t('usenet.indexers.messages.created')
}

function edit(indexer: Indexer): void {
  message.value = null
  list.edit(indexer)
  form.name = indexer.name
  form.url = indexer.url
  // Never prefilled: the key is not readable, and a blank field means "keep it".
  form.apiKey = ''
  form.categories = [...(indexer.categories ?? [])]
  form.enabled = indexer.enabled
  form.listStyle = indexer.list_style ?? 'compact'
  caps.value = null
  void focusForm()
}

function capsSummary(answer: IndexerCaps): string {
  return t('usenet.indexers.messages.tested', { server: answer.server || form.name || '—', count: answer.categories.length })
}

/** The stored indexer's own test, from its row. */
async function test(indexer: Indexer): Promise<void> {
  testingId.value = indexer.id
  message.value = null
  error.value = null
  const response = await api.POST('/api/v1/indexers/{id}/caps', { params: { path: { id: indexer.id } } })
  testingId.value = null
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  message.value = t('usenet.indexers.messages.tested', { server: response.data.server || indexer.name, count: response.data.categories.length })
}

/**
 * Fills the category choice for the form. A saved indexer is asked with its stored key; a new
 * one with the key in the field, used for that one request and not stored.
 */
async function loadCategories(): Promise<void> {
  capsBusy.value = true
  error.value = null
  const response = editingId.value && !form.apiKey.trim()
    ? await api.POST('/api/v1/indexers/{id}/caps', { params: { path: { id: editingId.value } } })
    : await api.POST('/api/v1/subscriptions/caps', { body: { url: form.url.trim(), api_key: form.apiKey.trim() } })
  capsBusy.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  caps.value = response.data
  message.value = capsSummary(response.data)
}

async function remove(indexer: Indexer): Promise<void> {
  deletingId.value = indexer.id
  message.value = null
  const { removed } = await list.remove(indexer)
  deletingId.value = null
  if (removed) message.value = t('usenet.indexers.messages.deleted', { name: indexer.name })
}
</script>

<template>
  <FormListLayout>
    <template #form>
      <section data-settings-anchor="usenet.indexer" class="border border-muted bg-default p-5">
        <SectionHeader
          :eyebrow="t('usenet.indexers.eyebrow')"
          :title="editingId ? t('usenet.indexers.title_edit') : t('usenet.indexers.title_add')"
          :description="t('usenet.indexers.description')"
        />
        <UAlert v-if="error" class="mt-4" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mt-4" color="success" variant="subtle" :description="message" data-testid="indexer-message" />
        <form ref="formElement" class="mt-4 grid gap-3" @submit.prevent="save">
          <UFormField :label="t('usenet.indexers.name')" name="name" required>
            <UInput v-model="form.name" required :maxlength="MAX_NAME" class="w-full" data-testid="indexer-name" />
          </UFormField>
          <UFormField :label="t('usenet.indexers.url')" name="url" :description="t('usenet.indexers.url_hint')" required>
            <UInput v-model="form.url" type="url" required class="w-full font-mono" placeholder="https://api.example.org/api" data-testid="indexer-url" />
          </UFormField>
          <UFormField :label="t('usenet.indexers.api_key')" name="api_key" :required="!editingId">
            <UInput
              v-model="form.apiKey"
              type="password"
              class="w-full"
              :required="!editingId"
              autocomplete="off"
              :placeholder="editingId ? t('usenet.indexers.api_key_keep') : ''"
              data-testid="indexer-api-key"
            />
          </UFormField>
          <UFormField :label="t('usenet.indexers.categories')" name="categories" :description="t('usenet.indexers.categories_hint')">
            <div class="flex flex-col gap-2">
              <USelectMenu
                v-if="categoryItems.length"
                v-model="form.categories"
                multiple
                class="w-full"
                value-key="value"
                :items="categoryItems"
                :placeholder="t('usenet.indexers.categories_all')"
              />
              <UInputTags v-else v-model="form.categories" class="w-full" :placeholder="t('usenet.indexers.categories_all')" data-testid="indexer-categories" />
              <UButton
                class="self-start"
                size="xs"
                color="neutral"
                variant="outline"
                icon="i-lucide-list-tree"
                :label="t('usenet.indexers.load_categories')"
                :loading="capsBusy"
                :disabled="!canLoadCategories"
                data-testid="indexer-load-categories"
                @click="loadCategories"
              />
            </div>
          </UFormField>
          <UFormField :label="t('usenet.indexers.list_style')" name="list_style" :description="t('usenet.indexers.list_style_hint')">
            <URadioGroup v-model="form.listStyle" :items="listStyleItems" data-testid="indexer-list-style" />
          </UFormField>
          <USwitch v-model="form.enabled" :label="t('usenet.indexers.enabled')" />
          <FormActions
            :editing="editingId !== null"
            :create-label="t('usenet.indexers.create')"
            create-icon="i-lucide-search-check"
            :save-label="t('usenet.indexers.save')"
            :loading="pending"
            @cancel="list.reset"
          />
        </form>
      </section>
    </template>

    <template #list>
      <section data-settings-anchor="usenet.indexers" class="border border-muted bg-default p-5">
        <div class="mb-4 flex items-start justify-between gap-4">
          <div>
            <SectionHeader :eyebrow="t('usenet.indexers.list_eyebrow')" :title="t('usenet.indexers.list_title')" />
            <p v-if="indexers.length" class="mt-2 max-w-2xl text-xs leading-5 text-muted">{{ t('usenet.indexers.list_description') }}</p>
          </div>
          <UBadge color="neutral" variant="outline">{{ indexers.length }}</UBadge>
        </div>
        <div class="space-y-2">
          <article v-for="indexer in indexers" :key="indexer.id" class="min-w-0 border p-4" :class="editingId === indexer.id ? 'border-primary' : 'border-muted'" data-testid="indexer-row">
            <div class="flex flex-wrap items-start gap-4">
              <div class="min-w-0 flex-1 basis-40">
                <div class="flex items-center gap-2"><span class="size-2" :class="indexer.enabled ? 'bg-success' : 'bg-muted'" /><h4 class="truncate text-sm font-semibold text-highlighted">{{ indexer.name }}</h4></div>
                <p class="mt-1 truncate font-mono text-xs text-muted">{{ indexer.url }}</p>
                <p class="mt-2 text-xs text-muted">{{ indexer.categories?.length ? indexer.categories.join(', ') : t('usenet.indexers.categories_all') }}</p>
              </div>
              <div class="flex flex-wrap items-center gap-2">
                <UBadge v-if="editingId === indexer.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                <UBadge v-if="!indexer.enabled" color="neutral" variant="subtle">{{ t('usenet.indexers.disabled') }}</UBadge>
                <UBadge v-if="indexer.list_style === 'detailed'" color="neutral" variant="outline" icon="i-lucide-image" data-testid="indexer-detailed">{{ t('usenet.indexers.list_styles.detailed') }}</UBadge>
                <UIcon v-if="indexer.has_secret" name="i-lucide-key-round" class="text-primary" :aria-label="t('usenet.indexers.has_key')" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-plug-zap" :label="t('common.actions.test')" :loading="testingId === indexer.id" :data-testid="`indexer-test-${indexer.id}`" @click="test(indexer)" />
                <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('usenet.indexers.edit')" :title="t('usenet.indexers.edit')" @click="edit(indexer)" />
                <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('usenet.indexers.delete.action')" :title="t('usenet.indexers.delete.action')" :loading="deletingId === indexer.id" @click="remove(indexer)" />
              </div>
            </div>
          </article>
          <DataState :loading="loading" :error="loadError" :empty="!indexers.length" :rows="2">
            <p class="signal-grid border border-dashed border-muted p-10 text-center text-sm text-muted">{{ t('usenet.indexers.empty') }}</p>
          </DataState>
        </div>
      </section>
    </template>
  </FormListLayout>
</template>
