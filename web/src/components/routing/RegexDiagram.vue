<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TestRegexResponse } from '@/api/types'
import RegexDiagramNode from '@/components/routing/RegexDiagramNode.vue'
import { translateServerMessage } from '@/i18n/server'
import { diagramSteps } from '@/utils/regexDiagram'

/**
 * The regex editor's diagram of its pattern (RD-1140-06), drawn from the structure the service's
 * tester answers with, so it shows the pattern as the rules run it. An invalid pattern shows its
 * error instead; a pattern too large to draw says so. The drawing is hidden from screen readers,
 * which get the same structure as a list of steps; the frame scrolls sideways, never the page.
 */
const props = defineProps<{
  /** The tester's last answer, `null` before the first or without a pattern. */
  response: TestRegexResponse | null
  /** The answer is for an older pattern: the diagram dims and an error waits for the next one. */
  stale?: boolean
}>()
const { t } = useI18n()

const steps = computed(() => props.response?.structure ? diagramSteps(props.response.structure, t) : [])
</script>

<template>
  <div v-if="response" data-testid="regex-diagram">
    <UAlert v-if="!response.valid && !stale" color="error" :title="t('routing.rule.regex_editor.invalid_pattern')" :description="response.error ?? undefined" :ui="{ description: 'font-mono text-xs break-all' }" />
    <template v-else-if="response.valid">
      <p class="eyebrow">{{ t('routing.rule.regex_editor.diagram.title') }}</p>
      <p v-if="response.structure_error" class="mt-1 text-xs text-muted" data-testid="regex-diagram-limits">{{ translateServerMessage({ code: response.structure_error }) }}</p>
      <div
        v-else-if="response.structure"
        role="group"
        tabindex="0"
        :aria-label="t('routing.rule.regex_editor.diagram.label')"
        class="mt-2 max-w-full overflow-x-auto rounded-sm border border-muted p-3 transition-opacity focus-visible:outline-2 focus-visible:outline-primary"
        :class="{ 'opacity-60': stale }"
      >
        <div aria-hidden="true" class="flex w-max items-center">
          <span class="size-2 shrink-0 rounded-full border border-accented" />
          <span class="w-3 shrink-0 border-t border-accented" />
          <RegexDiagramNode :node="response.structure" />
          <span class="w-3 shrink-0 border-t border-accented" />
          <span class="size-2 shrink-0 rounded-full border border-accented" />
        </div>
        <ol class="sr-only" data-testid="regex-diagram-steps">
          <li v-for="(step, index) in steps" :key="index">{{ step }}</li>
        </ol>
      </div>
    </template>
  </div>
</template>
