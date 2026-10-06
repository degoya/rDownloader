<script setup lang="ts">
/**
 * The diagnostic bundle, the second tab of the log page (RD-110-02, RD-1120-01).
 *
 * It sat under the log list until the tab: at the end of a long list nobody found it but by
 * searching the page. The state stays in `useLogsStore`; the bundle is offered only after its
 * preview was shown, and the preview's digest goes back with what was ticked.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { BundleNote } from '@/api/types'
import { BASE_PATH } from '@/basePath'
import SectionHeader from '@/components/SectionHeader.vue'
import { useRangeSelection } from '@/composables/useRangeSelection'
import { useLogsStore } from '@/stores/logs'

const { t, te } = useI18n()
const store = useLogsStore()

/**
 * A line the server sent about the bundle, in the reader's language (RD-120-15).
 *
 * The server writes no prose: an entry's description, its redaction notes and the
 * never-included lines travel as a stable code plus the data the sentence names. A code no
 * catalogue knows falls back to the English text that came with it, never to the raw key --
 * an untranslated sentence is still an answer, a dotted identifier is not.
 */
function noteText(note: BundleNote): string {
  const key = `logs.${note.code}`
  return te(key) ? t(key, { ...(note.params ?? {}) }) : note.text
}

function downloadHref(fileName: string): string {
  return `${BASE_PATH}/api/v1/diagnostics/bundles/${encodeURIComponent(fileName)}`
}

/** Bundle entries: a click toggles one, Shift+click the rows from the last click (RD-170-13). */
const entryRange = useRangeSelection(
  computed(() => store.preview?.entries.map(entry => entry.id) ?? []),
  (ids, checked) => {
    const rest = store.selected.filter(entry => !ids.includes(entry))
    store.selected = checked ? [...rest, ...ids] : rest
  }
)
</script>

<template>
  <UCard as="section" data-testid="diagnostic-bundle">
    <!-- The tab's own header: under the page's h1 with no list heading before it, an h3 would skip a level. -->
    <SectionHeader level="page" :eyebrow="t('logs.bundle.eyebrow')" :title="t('logs.bundle.title')" :description="t('logs.bundle.intro')" />
    <div class="mt-4 flex flex-wrap gap-2">
      <UButton
        color="neutral"
        variant="outline"
        icon="i-lucide-list-checks"
        :label="t('logs.bundle.preview')"
        :loading="store.bundleBusy"
        @click="store.loadPreview()"
      />
      <UButton
        icon="i-lucide-package"
        :label="t('logs.bundle.create')"
        :disabled="!store.canCreate"
        :loading="store.bundleBusy && store.preview !== null"
        @click="store.create()"
      />
    </div>
    <UAlert v-if="store.bundleError" class="mt-4" color="error" :description="store.bundleError" />

    <div v-if="store.preview" class="mt-4 space-y-4" data-testid="bundle-preview">
      <p class="text-xs text-muted">{{ t('logs.bundle.directory', { directory: store.preview.directory }) }}</p>
      <div>
        <h3 class="text-sm font-semibold text-highlighted">{{ t('logs.bundle.entries') }}</h3>
        <p class="mb-2 text-xs text-muted">{{ t('logs.bundle.select_hint') }}</p>
        <ul class="divide-y divide-muted border border-muted" @click.capture="entryRange.noteModifier" @keydown.capture="entryRange.noteModifier">
          <li v-for="entry in store.preview.entries" :key="entry.id" class="px-3 py-2">
            <UCheckbox
              :model-value="store.selected.includes(entry.id)"
              :label="entry.path"
              @update:model-value="(value: boolean | 'indeterminate') => entryRange.pick(entry.id, value === true)"
            />
            <p class="mt-1 text-xs text-muted">
              {{ noteText(entry.description) }} · {{ t('logs.bundle.items', { count: entry.items }) }}
            </p>
            <p v-if="entry.redactions.length" class="mt-1 text-xs text-muted">
              <span class="font-medium">{{ t('logs.bundle.redactions') }}:</span> {{ entry.redactions.map(noteText).join('; ') }}
            </p>
          </li>
        </ul>
      </div>
      <div>
        <h3 class="text-sm font-semibold text-highlighted">{{ t('logs.bundle.excluded') }}</h3>
        <ul class="mt-1 list-disc pl-5 text-xs text-muted">
          <li v-for="line in store.preview.excluded" :key="line.code">{{ noteText(line) }}</li>
        </ul>
      </div>
    </div>

    <UAlert
      v-if="store.created"
      class="mt-4"
      color="neutral"
      icon="i-lucide-package-check"
      orientation="vertical"
      :title="t('logs.bundle.created', { file: store.created.file_name })"
      :description="`${store.created.path} · ${t('logs.bundle.bytes', { bytes: store.created.bytes })}`"
      :ui="{ description: 'numeric text-xs text-muted' }"
      data-testid="bundle-created"
    >
      <template #actions>
        <UButton
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-download"
          :label="t('logs.bundle.download')"
          :to="downloadHref(store.created.file_name)"
          external
          download
        />
      </template>
    </UAlert>
  </UCard>
</template>
