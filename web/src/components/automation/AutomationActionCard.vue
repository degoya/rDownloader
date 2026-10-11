<script setup lang="ts">
/**
 * One action of the automation editor (`AutomationView.vue`): its kind and the fields that kind
 * needs — a script, a category, a notification target, a priority, a message, links
 * (RD-090-05, RD-1240-10).
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, NotificationTarget } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { type DraftAction, MAX_NOTIFY_MESSAGE } from '@/composables/useAutomationDraft'

const props = defineProps<{
  kindOptions: { value: string, label: string }[]
  scriptItems: { value: string, label: string }[]
  categories: Category[]
  targets: NotificationTarget[]
}>()
const action = defineModel<DraftAction>({ required: true })
const emit = defineEmits<{ kind: [kind: string], remove: [] }>()
const { t } = useI18n()

const priorityOptions = computed(() =>
  (['low', 'normal', 'high'] as const).map(value => ({ value, label: t(`automation.action.priority_value.${value}`) }))
)
const destinationOptions = computed(() =>
  (['link_grabber', 'downloads'] as const).map(value => ({ value, label: t(`automation.action.destination_value.${value}`) }))
)
/** The actions that point at a notification target. */
const needsTarget = computed(() => action.value.kind === 'webhook' || action.value.kind === 'notify')

function patch(change: Partial<DraftAction>): void {
  action.value = { ...action.value, ...change }
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2 border border-muted p-3" data-testid="automation-action-card">
    <USelectMenu
      :model-value="action.kind"
      :items="props.kindOptions"
      value-key="value"
      :aria-label="t('automation.action.kind')"
      class="w-52"
      @update:model-value="(value: string) => emit('kind', value)"
    />
    <SearchableSelect
      v-if="action.kind === 'script' && props.scriptItems.length"
      :model-value="action.name"
      :items="props.scriptItems"
      :aria-label="t('automation.action.script_name')"
      class="w-56 font-mono"
      @update:model-value="(name: string) => patch({ name })"
    />
    <p v-else-if="action.kind === 'script'" class="self-center text-xs text-error">
      {{ t('automation.action.no_scripts') }}
    </p>
    <USelectMenu
      v-if="action.kind === 'set_category'"
      :model-value="action.category_id"
      :items="props.categories"
      value-key="id"
      label-key="name"
      :aria-label="t('automation.action.category')"
      :placeholder="t('automation.action.category')"
      class="w-56"
      @update:model-value="(id: string) => patch({ category_id: id })"
    />
    <USelect
      v-if="action.kind === 'set_priority'"
      :model-value="action.priority"
      :items="priorityOptions"
      :aria-label="t('automation.action.priority')"
      class="w-40"
      @update:model-value="(priority: 'low' | 'normal' | 'high') => patch({ priority })"
    />
    <USelectMenu
      v-if="needsTarget"
      :model-value="action.target_id"
      :items="props.targets"
      value-key="id"
      label-key="name"
      :filter-fields="['name', 'endpoint']"
      :aria-label="t('automation.action.target')"
      :placeholder="t('automation.action.target')"
      class="w-56"
      @update:model-value="(id: string) => patch({ target_id: id })"
    />
    <p v-if="needsTarget && !props.targets.length" class="self-center text-xs text-error">
      {{ t('automation.action.no_targets') }}
    </p>
    <UButton
      icon="i-lucide-x"
      size="xs"
      color="error"
      variant="ghost"
      class="ms-auto"
      :aria-label="t('automation.action.remove')"
      @click="emit('remove')"
    />
    <UFormField v-if="action.kind === 'notify'" :label="t('automation.action.message')" class="w-full">
      <UTextarea
        :model-value="action.message"
        :maxlength="MAX_NOTIFY_MESSAGE"
        :rows="2"
        autoresize
        class="w-full"
        @update:model-value="(message: string) => patch({ message })"
      />
    </UFormField>
    <template v-if="action.kind === 'add_links'">
      <UFormField :label="t('automation.action.destination')">
        <USelect
          :model-value="action.destination"
          :items="destinationOptions"
          class="w-48"
          @update:model-value="(destination: 'link_grabber' | 'downloads') => patch({ destination })"
        />
      </UFormField>
      <UFormField :label="t('automation.action.links')" class="w-full">
        <UTextarea
          :model-value="action.links_text"
          :rows="3"
          autoresize
          class="w-full font-mono"
          placeholder="https://"
          @update:model-value="(links_text: string) => patch({ links_text })"
        />
      </UFormField>
    </template>
  </div>
</template>
