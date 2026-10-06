<script setup lang="ts">
/**
 * The subscription editor beside the list (split out of `SubscriptionsView.vue`, RD-140-27).
 *
 * Two things the form insists on, because both are decisions people regret otherwise:
 * the backlog policy is chosen when the subscription is created rather than defaulted
 * silently, and review is the default mode. A subscription that starts queueing a decade of
 * uploads on its own is not something an undo button fixes.
 *
 * The view starts an edit through `edit()` and learns which row is being edited through the
 * `editing` model, which it highlights in the list.
 */
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, IndexerCaps, Subscription } from '@/api/types'
import FormActions from '@/components/FormActions.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SubscriptionGitRelease from '@/components/SubscriptionGitRelease.vue'
import SubscriptionIndexerCategories from '@/components/SubscriptionIndexerCategories.vue'
import SubscriptionIndexerSearch from '@/components/SubscriptionIndexerSearch.vue'
import { useFormBaseline } from '@/composables/useFormBaseline'
import { useFormFocus } from '@/composables/useFormFocus'
import { useRegexEditor } from '@/composables/useRegexEditor'
import { translateServerMessage } from '@/i18n/server'
import { usePostprocessStore } from '@/stores/postprocess'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import { NO_INDEXER, maxAgeDays, queryProblem } from '@/utils/indexerSearch'
import { WHOLE } from '@/utils/numberInput'
import { argumentsProblem, splitArguments } from '@/utils/scriptArguments'
import { emptyForm, fillForm, formBody, NONE, type SubscriptionFormFields } from '@/utils/subscriptionForm'
import { CARD_RATIOS, DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

const props = defineProps<{ categories: Category[] }>()

/** The id of the subscription in the form; `null` while it adds a new one. */
const editing = defineModel<string | null>('editing', { required: true })

const { t } = useI18n()
const store = useSubscriptionsStore()
const postprocess = usePostprocessStore()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
/**
 * What the server refused on the last save, shown above the form (`design.md`, *Forms Share One
 * Shape*) rather than at the top of the page. The form is long — a release subscription's ends a
 * screen below its first field — so the alert is also scrolled into view: the reader is at the
 * button they pressed, and a refusal they cannot see reads as a button that did nothing.
 */
const refusal = ref<string | null>(null)
const refusalElement = ref<HTMLElement | null>(null)
const editRegex = useRegexEditor()
const caps = ref<IndexerCaps | null>(null)
const capsError = ref<string | null>(null)
const capsBusy = ref(false)

const form = reactive<SubscriptionFormFields>(emptyForm())
/** Whether the form holds edits a leave would lose; the view asks before them (RD-1120-15). */
const baseline = useFormBaseline(() => form)

/** How the LinkGrabber draws this subscription's hits (RD-120-37). */
const viewItems = computed(() => [
  { value: 'list', label: t('subscriptions.form.views.list') },
  { value: 'cards', label: t('subscriptions.form.views.cards') }
])

/** The shape of a card's picture area (RD-120-42); the ratios read the same in every language. */
const cardRatioItems = computed(() => CARD_RATIOS.map(ratio => ({
  value: ratio,
  label: ratio === DEFAULT_CARD_RATIO ? t('subscriptions.form.card_ratio_default', { ratio }) : ratio
})))

const kindItems = computed(() => [
  { value: 'media', label: t('subscriptions.kinds.media') },
  { value: 'gallery', label: t('subscriptions.kinds.gallery') },
  { value: 'feed', label: t('subscriptions.kinds.feed') },
  { value: 'indexer', label: t('subscriptions.kinds.indexer') },
  { value: 'site_rule', label: t('subscriptions.kinds.site_rule') },
  { value: 'git_release', label: t('subscriptions.kinds.git_release') },
  { value: 'script', label: t('subscriptions.kinds.script') }
])

/**
 * The scripts to choose from: the scripts folder, the same list the automation offers (RD-150-08).
 * A saved script that is no longer there stays in the list, marked, so an edit does not quietly
 * point the subscription somewhere else.
 */
const scriptItems = computed(() => {
  const available = postprocess.scripts ?? []
  const items = available.map(name => ({ value: name, label: name }))
  if (form.script && !available.includes(form.script)) {
    items.unshift({ value: form.script, label: t('subscriptions.form.script_missing', { name: form.script }) })
  }
  return items
})

/** The arguments the parameter line becomes, or `null` while a quote is open. */
const scriptArgumentList = computed(() => splitArguments(form.scriptArguments))

/** What is wrong with the parameter line, in the words the server would use for it. */
const scriptArgumentsError = computed(() => {
  const list = scriptArgumentList.value
  if (list === null) return t('subscriptions.form.script_arguments_unclosed')
  const problem = argumentsProblem(list)
  return problem ? translateServerMessage(problem) : null
})

// Loaded when a script is first wanted, not with the page: most subscriptions are not scripts.
// A new script subscription starts on the first script there is, as an automation action does.
watch(
  () => form.kind,
  async kind => {
    if (kind !== 'script') return
    const scripts = await postprocess.loadScripts()
    if (form.kind === 'script' && !form.script) form.script = scripts[0] ?? ''
  }
)

/**
 * The floor the server enforces per kind, in minutes: a board page is not an indexer, and a
 * forge counts every request against an hourly budget (RD-190-13).
 */
const minimumMinutes = computed(() => (form.kind === 'site_rule' ? 30 : form.kind === 'git_release' ? 15 : 5))

/** Where a key or token is entered at all: an indexer's key, a private repository's token. */
const takesSecret = computed(() => form.kind === 'indexer' || form.kind === 'git_release')

const modeItems = computed(() => [
  { value: 'review', label: t('subscriptions.modes.review') },
  { value: 'auto_queue', label: t('subscriptions.modes.auto_queue') }
])

const backlogItems = computed(() => [
  { value: 'from_now', label: t('subscriptions.backlog.from_now') },
  { value: 'review_all', label: t('subscriptions.backlog.review_all') }
])

const categoryItems = computed(() => [
  { value: NONE, label: t('subscriptions.form.default_category') },
  ...props.categories.map(category => ({ value: category.id, label: category.name }))
])

/** Whether the search fields hold something the server would refuse; the fields say what. */
const searchInvalid = computed(() => form.kind === 'indexer' && (
  queryProblem(form.search.query) !== null ||
  (form.search.maxAge != null && maxAgeDays(form.search.maxAge) === null)
))

/** Taking a defined indexer over lets the address and the key fields stay empty. */
const takesOver = computed(() => form.kind === 'indexer' && form.indexerId !== NO_INDEXER)

function body() {
  return formBody(form, scriptArgumentList.value, takesOver.value)
}

function reset(): void {
  Object.assign(form, emptyForm())
  editing.value = null
  refusal.value = null
  baseline.settle()
}

async function submit(): Promise<void> {
  // The field says what is wrong; a line that would not arrive as shown is not sent.
  if (form.kind === 'script' && scriptArgumentsError.value) return
  if (searchInvalid.value) return
  refusal.value = null
  const saved = editing.value ? await store.update(editing.value, body()) : await store.create(body())
  if (saved) {
    reset()
    return
  }
  // Taken over from the store, so the page's own alert does not say it a second time.
  refusal.value = store.error
  store.error = null
  await nextTick()
  refusalElement.value?.scrollIntoView({ block: 'nearest' })
}

function edit(subscription: Subscription): void {
  editing.value = subscription.id
  refusal.value = null
  fillForm(form, subscription)
  baseline.settle()
  caps.value = null
  capsError.value = null
  void focusForm()
}

/**
 * Tests the indexer and loads its category tree (RD-080-11).
 *
 * Only possible once it is saved: the key lives in the vault, and the request is made
 * server-side so it never passes through here.
 */
async function testIndexer(id: string | null): Promise<void> {
  capsBusy.value = true
  // A saved subscription is asked through its id, so the stored key never has to be retyped.
  // Before it is saved there is no key in the vault to resolve, so the form sends the one in
  // the field — used for that one request and not stored.
  const result = id
    ? await store.loadCaps(id)
    : await store.probeCaps(form.url.trim(), form.apiKey.trim())
  capsBusy.value = false
  if ('error' in result) {
    capsError.value = result.error
    caps.value = null
    return
  }
  capsError.value = null
  caps.value = result
  // First time round, the categories already mapped are the ones being asked for. Anything
  // else would silently change what an existing subscription fetches.
  if (!form.sourceCategories.length) {
    form.sourceCategories = [...new Set(form.categoryMap.map(mapping => mapping.source_category))]
      .filter(value => value.length > 0)
  }
}

/// Whether the categories can be asked for at all: an address, and a key to ask with.
const canProbeCaps = computed(() =>
  form.kind === 'indexer' && form.url.trim().length > 0 && (Boolean(editing.value) || form.apiKey.trim().length > 0)
)

/**
 * Edits one title pattern as a regular expression.
 *
 * The field holds a comma-separated list, and only a pattern wrapped in slashes is treated as
 * an expression, so the editor works on the last entry and writes it back wrapped. Anything
 * already written as plain text keeps its meaning.
 */
async function editTitlePattern(field: 'titleContains' | 'titleExcludes'): Promise<void> {
  const entries = form[field].split(',').map(entry => entry.trim()).filter(entry => entry.length > 0)
  const last = entries.pop() ?? ''
  const bare = last.replace(/^\/(.*)\/$/, '$1')
  const result = await editRegex(bare || null)
  if (!result?.pattern) return
  form[field] = [...entries, `/${result.pattern}/`].join(', ')
}

defineExpose({ edit, reset, dirty: baseline.dirty })
</script>

<template>
  <UCard as="section">
    <SectionHeader
      class="mb-4"
      :eyebrow="t('subscriptions.title')"
      :title="editing ? t('subscriptions.form.edit') : t('subscriptions.form.form_new')"
    />
    <div v-if="refusal" ref="refusalElement" class="mb-4 scroll-mt-4" data-testid="subscription-refusal">
      <UAlert color="error" icon="i-lucide-circle-alert" :description="refusal" />
    </div>
    <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
      <!-- First: the type decides which fields follow (script, address, schedule, filters). -->
      <UFormField :label="t('subscriptions.form.kind')">
        <USelect v-model="form.kind" class="w-full" :items="kindItems" value-key="value" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.name')">
        <UInput v-model="form.name" required class="w-full" data-testid="subscription-name" />
      </UFormField>
      <UFormField
        v-if="form.kind === 'script'"
        :label="t('subscriptions.form.script')"
        :description="t('subscriptions.form.script_description')"
      >
        <USelect
          v-if="scriptItems.length"
          v-model="form.script"
          class="w-full font-mono"
          :items="scriptItems"
          value-key="value"
          data-testid="subscription-script"
        />
        <p v-else class="text-xs text-error" data-testid="subscription-no-scripts">
          {{ t('automation.action.no_scripts') }}
        </p>
      </UFormField>
      <UFormField
        v-if="form.kind === 'script'"
        :label="t('subscriptions.form.script_arguments')"
        :description="t('subscriptions.form.script_arguments_description')"
      >
        <UInput
          v-model="form.scriptArguments"
          class="w-full font-mono"
          autocomplete="off"
          spellcheck="false"
          data-testid="subscription-script-arguments"
        />
        <p v-if="scriptArgumentsError" class="mt-1 text-xs text-error" data-testid="subscription-script-arguments-error">
          {{ scriptArgumentsError }}
        </p>
        <ul
          v-else-if="scriptArgumentList?.length"
          class="mt-2 flex flex-wrap gap-1"
          :aria-label="t('subscriptions.form.script_arguments_preview')"
          data-testid="subscription-script-arguments-preview"
        >
          <li v-for="(argument, index) in scriptArgumentList" :key="index">
            <UBadge color="neutral" variant="subtle" class="whitespace-pre font-mono">
              <template v-if="argument">{{ argument }}</template>
              <em v-else>{{ t('subscriptions.form.script_arguments_empty') }}</em>
            </UBadge>
          </li>
        </ul>
      </UFormField>
      <UFormField
        v-else
        :label="t('subscriptions.form.url')"
        :description="form.kind === 'site_rule' ? t('subscriptions.form.site_rule_description') : form.kind === 'git_release' ? t('subscriptions.form.git_url_description') : takesOver ? t('subscriptions.form.url_from_indexer') : undefined"
      >
        <UInput v-model="form.url" type="url" :required="!takesOver" class="w-full" data-testid="subscription-url" />
      </UFormField>
      <SubscriptionGitRelease v-if="form.kind === 'git_release'" v-model:fields="form.gitRelease" />
      <SubscriptionIndexerSearch
        v-if="form.kind === 'indexer'"
        v-model:indexer-id="form.indexerId"
        v-model:search="form.search"
      />
      <UFormField
        v-if="form.kind === 'script'"
        :label="t('subscriptions.form.schedule')"
        :description="t('subscriptions.form.schedule_description')"
      >
        <UInput v-model="form.schedule" class="w-full font-mono" placeholder="0 6 * * *" data-testid="subscription-schedule" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.mode')" :description="t('subscriptions.form.mode_hint')">
        <USelect v-model="form.mode" class="w-full" :items="modeItems" value-key="value" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.category')">
        <USelect v-model="form.categoryId" class="w-full" :items="categoryItems" value-key="value" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.interval')">
        <UInputNumber v-model="form.intervalMinutes" required class="w-full" :min="minimumMinutes" :step="5" :format-options="WHOLE" />
      </UFormField>
      <UFormField
        v-if="form.kind === 'site_rule'"
        :label="t('subscriptions.form.every_release')"
        :description="t('subscriptions.form.every_release_description')"
      >
        <USwitch v-model="form.everyRelease" data-testid="subscription-every-release" />
      </UFormField>
      <!-- Only indexer hits reach the LinkGrabber's review drawer, so only they have a view. -->
      <UFormField
        v-if="form.kind === 'indexer'"
        :label="t('subscriptions.form.view')"
        :description="t('subscriptions.form.view_description')"
      >
        <USelect
          v-model="form.view"
          class="w-full"
          :items="viewItems"
          value-key="value"
          data-testid="subscription-view"
        />
      </UFormField>
      <UFormField
        v-if="form.kind === 'indexer' && form.view === 'cards'"
        :label="t('subscriptions.form.autoplay')"
        :description="t('subscriptions.form.autoplay_description')"
      >
        <USwitch v-model="form.autoplay" data-testid="subscription-autoplay" />
      </UFormField>
      <UFormField
        v-if="form.kind === 'indexer' && form.view === 'cards'"
        :label="t('subscriptions.form.card_ratio')"
        :description="t('subscriptions.form.card_ratio_description')"
      >
        <USelect
          v-model="form.cardRatio"
          class="w-full"
          :items="cardRatioItems"
          value-key="value"
          data-testid="subscription-card-ratio"
        />
      </UFormField>
      <!-- A script has no history to protect against: its first run is taken as it is. -->
      <UFormField
        v-if="form.kind !== 'script'"
        :label="t('subscriptions.form.backlog')"
        :description="t('subscriptions.form.backlog_hint')"
      >
        <USelect v-model="form.backlog" class="w-full" :items="backlogItems" value-key="value" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.title_contains')" :description="t('subscriptions.form.patterns_hint')">
        <UFieldGroup class="w-full">
          <UInput v-model="form.titleContains" class="w-full" />
          <UButton color="neutral" variant="outline" icon="i-lucide-regex" :aria-label="t('subscriptions.form.regex_editor')" @click="editTitlePattern('titleContains')" />
        </UFieldGroup>
      </UFormField>
      <UFormField :label="t('subscriptions.form.title_excludes')" :description="t('subscriptions.form.patterns_hint')">
        <UFieldGroup class="w-full">
          <UInput v-model="form.titleExcludes" class="w-full" />
          <UButton color="neutral" variant="outline" icon="i-lucide-regex" :aria-label="t('subscriptions.form.regex_editor')" @click="editTitlePattern('titleExcludes')" />
        </UFieldGroup>
      </UFormField>
      <UFormField
        v-if="takesSecret"
        :label="form.kind === 'git_release' ? t('subscriptions.form.git_token') : t('subscriptions.form.api_key')"
        :description="form.kind === 'git_release' ? t('subscriptions.form.git_token_hint') : t('subscriptions.form.api_key_hint')"
      >
        <UInput
          v-model="form.apiKey"
          class="w-full"
          type="password"
          autocomplete="off"
          :placeholder="editing ? (form.kind === 'git_release' ? t('subscriptions.form.git_token_keep') : t('subscriptions.form.api_key_keep')) : takesOver ? t('subscriptions.form.api_key_from_indexer') : ''"
          data-testid="subscription-api-key"
        />
      </UFormField>

      <SubscriptionIndexerCategories
        v-if="form.kind === 'indexer'"
        v-model:source-categories="form.sourceCategories"
        v-model:category-map="form.categoryMap"
        :categories="props.categories"
        :caps="caps"
        :caps-error="capsError"
        :caps-busy="capsBusy"
        :can-probe="canProbeCaps"
        @test="testIndexer(editing)"
      />

      <FormActions
        :editing="editing !== null"
        :create-label="t('subscriptions.form.create')"
        :loading="store.busy"
        data-testid="subscription-actions"
        @cancel="reset"
      />
    </form>
  </UCard>
</template>
