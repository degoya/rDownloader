<script setup lang="ts">
/**
 * Output template for one media link, with a live preview (RD-080-05).
 *
 * The preview is not rendered here. It comes from the server, through the *same* evaluator
 * the download uses — a second client-side implementation would drift, and the whole point
 * of the field is that what you see is where the file lands. An invalid template shows the
 * reason it was refused rather than a guess at what it might have meant.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

const { t } = useI18n()
const props = defineProps<{
  template: string | null
  /** Resolves a template to a path, or to a reason it was refused. */
  resolve: (template: string) => Promise<{ relative_path: string, fields: string[] } | { error: string }>
  busy?: boolean
}>()
const emit = defineEmits<{ change: [template: string | null] }>()

const draft = ref(props.template ?? '')
const preview = ref<string | null>(null)
const problem = ref<string | null>(null)
const fields = ref<string[]>([])
/** Built here rather than in the template: a `{…}` literal inside a mustache does not parse. */
const fieldList = computed(() => fields.value.map(field => `{${field}}`).join(' '))

watch(() => props.template, value => { draft.value = value ?? '' })

async function refresh(): Promise<void> {
  const result = await props.resolve(draft.value.trim())
  if ('error' in result) {
    problem.value = result.error
    preview.value = null
    return
  }
  problem.value = null
  preview.value = result.relative_path
  fields.value = result.fields
}

/** Commits the template only once it previewed cleanly; a refused one is never stored. */
async function commit(): Promise<void> {
  await refresh()
  if (problem.value) return
  emit('change', draft.value.trim() || null)
}

void refresh()
</script>

<template>
  <div class="flex flex-col gap-2" data-testid="media-output-template">
    <UFormField :label="t('linkgrabber.media.output.title')" :description="t('linkgrabber.media.output.hint')">
      <UInput
        v-model="draft"
        size="xs"
        :placeholder="t('linkgrabber.media.output.placeholder')"
        :disabled="props.busy"
        data-testid="media-output-input"
        @blur="commit"
        @keyup.enter="commit"
      />
    </UFormField>
    <p v-if="preview" class="truncate font-mono text-xs text-muted" data-testid="media-output-preview">
      {{ preview }}
    </p>
    <p v-if="problem" class="text-xs text-error" data-testid="media-output-error">{{ problem }}</p>
    <p v-if="fields.length" class="text-xs text-muted">
      {{ t('linkgrabber.media.output.fields', { fields: fieldList }) }}
    </p>
  </div>
</template>
