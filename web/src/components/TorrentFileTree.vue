<script setup lang="ts">
/**
 * The file tree of one torrent with folder tri-state selection.
 *
 * Only files the user actually toggled are sent back as explicit decisions. That keeps the
 * server-side rule intact that a per-file decision beats an exclusion pattern, while files
 * nobody touched stay under the pattern's control.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type {
  ResolvedTorrentPlan,
  TorrentEngineCapabilities,
  TorrentFileDecision,
  TorrentFilePriority,
  TorrentPlanRequest
} from '@/api/types'
import { formatBytes } from '@/utils/format'

const { t } = useI18n()
const props = defineProps<{
  plan: ResolvedTorrentPlan
  capabilities?: TorrentEngineCapabilities | null
  busy?: boolean
  readonly?: boolean
}>()
const emit = defineEmits<{ change: [plan: TorrentPlanRequest] }>()

/** One node of the tree; a node is a file when it carries an index. */
interface TreeNode {
  key: string
  name: string
  index: number | null
  size: bigint
  children: TreeNode[]
}

/** Explicit user decisions, keyed by file index. */
const explicit = ref(new Map<number, boolean>())
/** Per-file priorities, keyed by file index; absent means normal. */
const priorities = ref(new Map<number, TorrentFilePriority>())
/** Exclusion patterns, edited as one comma-separated field. */
const patternInput = ref('')
/** Folders the user collapsed. */
const collapsed = ref(new Set<string>())

/** Rebuilds the local decisions whenever the server sends a new resolution. */
watch(
  () => props.plan,
  (plan) => {
    const next = new Map<number, boolean>()
    const tiers = new Map<number, TorrentFilePriority>()
    for (const file of plan.files) {
      if (file.explicit) next.set(file.index, file.included)
      if (file.priority !== 'normal') tiers.set(file.index, file.priority)
    }
    explicit.value = next
    priorities.value = tiers
  },
  { immediate: true }
)

/** Effective selection of one file: a local decision wins, otherwise the server's. */
function isIncluded(file: TorrentFileDecision): boolean {
  return explicit.value.get(file.index) ?? file.included
}

const filesByIndex = computed(() => new Map(props.plan.files.map(file => [file.index, file])))

const tree = computed<TreeNode>(() => {
  const root: TreeNode = { key: '', name: '', index: null, size: 0n, children: [] }
  for (const file of props.plan.files) {
    let node = root
    file.path.forEach((component, depth) => {
      const leaf = depth === file.path.length - 1
      const key = `${node.key}/${component}`
      let child = node.children.find(candidate => candidate.key === key)
      if (!child) {
        child = { key, name: component, index: leaf ? file.index : null, size: 0n, children: [] }
        node.children.push(child)
      }
      node = child
    })
    node.size = BigInt(file.length)
  }
  sortTree(root)
  return root
})

/** Folders before files, each alphabetical, so the tree reads like a file manager. */
function sortTree(node: TreeNode): void {
  node.children.sort((left, right) => {
    if ((left.index === null) !== (right.index === null)) return left.index === null ? -1 : 1
    return left.name.localeCompare(right.name)
  })
  for (const child of node.children) sortTree(child)
}

/** Every file index below a node. */
function indicesOf(node: TreeNode): number[] {
  if (node.index !== null) return [node.index]
  return node.children.flatMap(indicesOf)
}

/** Summed size of a node's files. */
function sizeOf(node: TreeNode): bigint {
  if (node.index !== null) return node.size
  return node.children.reduce((total, child) => total + sizeOf(child), 0n)
}

/** Tri-state of a folder: all, none or some of its files selected. */
function stateOf(node: TreeNode): boolean | 'indeterminate' {
  const indices = indicesOf(node)
  let selected = 0
  for (const index of indices) {
    const file = filesByIndex.value.get(index)
    if (file && isIncluded(file)) selected += 1
  }
  if (selected === 0) return false
  return selected === indices.length ? true : 'indeterminate'
}

const selectedBytes = computed(() => {
  let total = 0n
  for (const file of props.plan.files) {
    if (isIncluded(file)) total += BigInt(file.length)
  }
  return total
})
const selectedCount = computed(() => props.plan.files.filter(isIncluded).length)

/** The whole current plan, as the API expects it. */
function currentPlan(): TorrentPlanRequest {
  const included: number[] = []
  const excluded: number[] = []
  for (const [index, decision] of explicit.value) (decision ? included : excluded).push(index)
  return {
    included,
    excluded,
    priorities: [...priorities.value].map(([index, priority]) => ({ index, priority })),
    exclusion_patterns: patterns.value,
    sequential: props.plan.sequential
  }
}

/** Sets a node and everything below it, then reports the whole plan. */
function toggle(node: TreeNode, value: boolean | 'indeterminate'): void {
  if (props.readonly || props.busy) return
  const next = new Map(explicit.value)
  for (const index of indicesOf(node)) next.set(index, value === true)
  explicit.value = next
  emit('change', currentPlan())
}

/** A folder priority is inherited onto its files; only per-file entries are stored. */
function setPriority(node: TreeNode, priority: TorrentFilePriority): void {
  if (props.readonly || props.busy) return
  const next = new Map(priorities.value)
  for (const index of indicesOf(node)) {
    if (priority === 'normal') next.delete(index)
    else next.set(index, priority)
  }
  priorities.value = next
  emit('change', currentPlan())
}

/** The shared priority of a node, or an empty string when its files disagree. */
function priorityOf(node: TreeNode): TorrentFilePriority | '' {
  const indices = indicesOf(node)
  const first = priorities.value.get(indices[0] ?? -1) ?? 'normal'
  return indices.every(index => (priorities.value.get(index) ?? 'normal') === first) ? first : ''
}

const patterns = computed(() =>
  patternInput.value
    .split(',')
    .map(pattern => pattern.trim())
    .filter(Boolean)
)

const priorityItems = computed(() =>
  (['high', 'normal', 'low', 'skip'] as const).map(value => ({
    label: t(`torrent.priority.${value}`),
    value
  }))
)

/** Saves the pattern field; the stored plan comes back naming what each pattern dropped. */
function applyPatterns(): void {
  if (props.readonly || props.busy) return
  emit('change', currentPlan())
}

function toggleCollapse(key: string): void {
  const next = new Set(collapsed.value)
  if (!next.delete(key)) next.add(key)
  collapsed.value = next
}

/** Depth-first rows, skipping the subtree of a collapsed folder. */
const rows = computed(() => {
  const output: { node: TreeNode, depth: number }[] = []
  const walk = (node: TreeNode, depth: number): void => {
    for (const child of node.children) {
      output.push({ node: child, depth })
      if (child.index === null && !collapsed.value.has(child.key)) walk(child, depth + 1)
    }
  }
  walk(tree.value, 0)
  return output
})

/** The pattern that excluded a file, when one did. */
function patternOf(node: TreeNode): string {
  if (node.index === null) return ''
  return filesByIndex.value.get(node.index)?.excluded_by_pattern ?? ''
}
</script>

<template>
  <div class="grid gap-2">
    <div class="flex items-center justify-between gap-3 text-xs text-muted">
      <span>{{ t('torrent.tree.selected', { count: selectedCount, total: props.plan.files.length }) }}</span>
      <span class="numeric">{{ formatBytes(selectedBytes.toString()) }} / {{ formatBytes(props.plan.total_bytes) }}</span>
    </div>
    <UFormField
      v-if="!props.readonly"
      :label="t('torrent.patterns.label')"
      :description="t('torrent.patterns.description')"
      size="xs"
    >
      <UInput
        v-model="patternInput"
        :placeholder="t('torrent.patterns.placeholder')"
        :disabled="props.busy"
        size="xs"
        @change="applyPatterns"
        @keyup.enter="applyPatterns"
      />
    </UFormField>
    <p v-if="props.capabilities && !props.capabilities.sequential_download" class="text-xs text-muted">
      {{ t('torrent.priority.emulated_hint') }}
    </p>
    <ul class="max-h-80 overflow-y-auto border border-muted bg-default">
      <li
        v-for="row in rows"
        :key="row.node.key"
        class="flex items-center gap-2 px-2 py-1 text-xs hover:bg-elevated/60"
        :style="{ paddingLeft: `${row.depth * 16 + 8}px` }"
      >
        <UButton
          v-if="row.node.index === null"
          :icon="collapsed.has(row.node.key) ? 'i-lucide-chevron-right' : 'i-lucide-chevron-down'"
          size="xs"
          color="neutral"
          variant="ghost"
          class="shrink-0"
          :aria-label="t('torrent.tree.toggle_folder', { name: row.node.name })"
          @click="toggleCollapse(row.node.key)"
        />
        <span v-else class="w-6 shrink-0" />
        <UCheckbox
          :model-value="stateOf(row.node)"
          :disabled="props.readonly || props.busy"
          :aria-label="t('torrent.tree.select_entry', { name: row.node.name })"
          @update:model-value="(value: boolean | 'indeterminate') => toggle(row.node, value)"
        />
        <UIcon
          :name="row.node.index === null ? 'i-lucide-folder' : 'i-lucide-file'"
          class="size-3.5 shrink-0 text-muted"
        />
        <span class="min-w-0 flex-1 truncate text-highlighted" :title="row.node.name">{{ row.node.name }}</span>
        <USelect
          v-if="!props.readonly"
          :model-value="priorityOf(row.node)"
          :items="priorityItems"
          value-key="value"
          size="xs"
          class="hidden w-28 shrink-0 sm:flex"
          :disabled="props.busy"
          :aria-label="t('torrent.priority.select', { name: row.node.name })"
          @update:model-value="(value: TorrentFilePriority) => setPriority(row.node, value)"
        />
        <UBadge
          v-if="patternOf(row.node)"
          color="warning"
          variant="subtle"
          size="sm"
          class="shrink-0"
          :title="t('torrent.tree.excluded_by_pattern', { pattern: patternOf(row.node) })"
        >{{ patternOf(row.node) }}</UBadge>
        <span class="numeric w-20 shrink-0 text-right text-muted">{{ formatBytes(sizeOf(row.node).toString()) }}</span>
      </li>
    </ul>
  </div>
</template>
