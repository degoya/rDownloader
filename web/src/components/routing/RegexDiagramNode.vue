<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { RegexNode } from '@/api/types'
import { flagWords, nodeLabel, repeatRange } from '@/utils/regexDiagram'

/**
 * One node of the regex diagram (RD-1140-06), drawn by its kind: a row of boxes joined by lines,
 * a labelled frame for a group, a "one of" stack for alternatives and classes, the ↻ range under
 * a repeated box. Recursive; `RegexDiagram.vue` frames the root and carries the text version.
 */
const props = defineProps<{ node: RegexNode }>()
const { t } = useI18n()

const children = computed(() => props.node.children ?? [])
const label = computed(() => nodeLabel(props.node, t))
const STACKS = new Set(['alternation', 'class', 'intersection', 'difference', 'symmetric_difference'])
const ASSERTIONS = new Set(['start', 'end', 'word_boundary', 'not_word_boundary', 'word_start', 'word_end'])
</script>

<template>
  <div v-if="node.kind === 'sequence'" class="flex items-center" data-kind="sequence">
    <template v-for="(child, index) in children" :key="index">
      <span v-if="index > 0" class="w-3 shrink-0 border-t border-accented" />
      <RegexDiagramNode :node="child" />
    </template>
  </div>
  <div v-else-if="node.kind === 'repetition'" class="flex flex-col items-center gap-0.5" data-kind="repetition">
    <RegexDiagramNode v-for="(child, index) in children" :key="index" :node="child" />
    <span class="whitespace-nowrap font-mono text-2xs text-muted">↻ {{ repeatRange(node) }}<template v-if="node.lazy"> · {{ t('routing.rule.regex_editor.diagram.lazy') }}</template></span>
  </div>
  <div v-else-if="node.kind === 'group'" class="rounded-sm border border-dashed border-primary/40 px-2 pt-1 pb-2" data-kind="group">
    <p class="mb-1 whitespace-nowrap text-2xs text-primary">{{ label }}<template v-if="node.flags?.length"> · {{ flagWords(node, t) }}</template></p>
    <RegexDiagramNode v-for="(child, index) in children" :key="index" :node="child" />
  </div>
  <div v-else-if="STACKS.has(node.kind)" class="rounded-sm border border-default px-2 pt-1 pb-2" :data-kind="node.kind">
    <p class="mb-1 whitespace-nowrap text-2xs text-muted">{{ label }}</p>
    <div class="flex flex-col items-start gap-1">
      <div v-for="(child, index) in children" :key="index" class="flex items-center">
        <span class="w-2 shrink-0 border-t border-accented" />
        <RegexDiagramNode :node="child" />
      </div>
    </div>
  </div>
  <UBadge v-else-if="node.kind === 'literal'" color="neutral" variant="outline" class="whitespace-pre font-mono" data-kind="literal">{{ node.text }}</UBadge>
  <UBadge v-else-if="node.kind === 'range'" color="neutral" variant="outline" class="font-mono" data-kind="range">{{ node.from }}–{{ node.to }}</UBadge>
  <UBadge v-else-if="ASSERTIONS.has(node.kind)" color="neutral" variant="soft" class="whitespace-nowrap" :data-kind="node.kind">{{ label }}</UBadge>
  <UBadge v-else-if="node.kind === 'flags'" color="warning" variant="subtle" class="whitespace-nowrap" data-kind="flags">{{ label }}: {{ flagWords(node, t) }}</UBadge>
  <UBadge v-else-if="node.kind === 'empty'" color="neutral" variant="outline" class="text-muted" data-kind="empty">{{ label }}</UBadge>
  <UBadge v-else color="secondary" variant="subtle" class="whitespace-nowrap" :data-kind="node.kind">{{ label }}</UBadge>
</template>
