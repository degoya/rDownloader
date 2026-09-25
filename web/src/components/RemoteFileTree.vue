<script setup lang="ts">
/**
 * The directory listing of one remote link, with folder tri-state selection.
 *
 * The server stores *exclusions*, not inclusions, so a re-listing that adds a file keeps
 * it selected rather than silently dropping it. This component therefore sends back the
 * excluded paths, and only ever names files: a folder checkbox expands to its files here,
 * so the ancestor rules live in exactly one place instead of being applied both here and
 * on the server, where the two could drift apart.
 */
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ResolvedRemoteListing } from '@/api/types'
import { formatBytes } from '@/utils/format'

const { t } = useI18n()
const props = defineProps<{
  listing: ResolvedRemoteListing
  busy?: boolean
  readonly?: boolean
}>()
const emit = defineEmits<{ change: [excluded: string[]] }>()

/** One node of the tree; a node is a file when it is not a directory. */
interface TreeNode {
  path: string
  name: string
  isDir: boolean
  size: bigint
  children: TreeNode[]
}

/** Excluded file paths, kept locally so the tree reacts before the round trip. */
const excluded = ref(new Set<string>())
const collapsed = ref(new Set<string>())

watch(
  () => props.listing,
  listing => {
    excluded.value = new Set(
      listing.entries
        .filter(entry => !entry.is_dir && !entry.included)
        .map(entry => entry.path)
    )
  },
  { immediate: true }
)

/** Builds the nesting from the flat, `/`-separated paths the server returns. */
const tree = computed<TreeNode[]>(() => {
  const roots: TreeNode[] = []
  const byPath = new Map<string, TreeNode>()
  // Sorted so a parent is always created before its children.
  const sorted = [...props.listing.entries].sort((a, b) => a.path.localeCompare(b.path))
  for (const entry of sorted) {
    const node: TreeNode = {
      path: entry.path,
      name: entry.path.split('/').pop() ?? entry.path,
      isDir: entry.is_dir,
      size: BigInt(entry.size ?? '0'),
      children: []
    }
    byPath.set(entry.path, node)
    const parentPath = entry.path.includes('/')
      ? entry.path.slice(0, entry.path.lastIndexOf('/'))
      : null
    const parent = parentPath ? byPath.get(parentPath) : undefined
    if (parent) parent.children.push(node)
    else roots.push(node)
  }
  return roots
})

function isExcluded(path: string): boolean {
  return excluded.value.has(path)
}

/** Tri-state for a folder: every file below it, none, or a mix. */
function folderState(node: TreeNode): 'all' | 'none' | 'some' {
  const files = filesUnder(node)
  if (!files.length) return 'all'
  const included = files.filter(file => !isExcluded(file.path)).length
  if (included === 0) return 'none'
  return included === files.length ? 'all' : 'some'
}

function filesUnder(node: TreeNode): TreeNode[] {
  return node.isDir ? node.children.flatMap(filesUnder) : [node]
}

function toggle(node: TreeNode, include: boolean): void {
  if (props.readonly) return
  const next = new Set(excluded.value)
  for (const file of filesUnder(node)) {
    if (include) next.delete(file.path)
    else next.add(file.path)
  }
  excluded.value = next
  emit('change', [...next])
}

function setAll(include: boolean): void {
  if (props.readonly) return
  const next = include
    ? new Set<string>()
    : new Set(tree.value.flatMap(filesUnder).map(file => file.path))
  excluded.value = next
  emit('change', [...next])
}

function toggleCollapse(path: string): void {
  const next = new Set(collapsed.value)
  if (!next.delete(path)) next.add(path)
  collapsed.value = next
}

const selectedBytes = computed(() => formatBytes(BigInt(props.listing.selected_bytes ?? '0')))
const totalFiles = computed(() => props.listing.entries.filter(entry => !entry.is_dir).length)

/** Flattened rows with their depth, so the template stays a single loop. */
const rows = computed(() => {
  const out: Array<{ node: TreeNode, depth: number }> = []
  const walk = (nodes: TreeNode[], depth: number): void => {
    for (const node of [...nodes].sort((a, b) => Number(b.isDir) - Number(a.isDir) || a.name.localeCompare(b.name))) {
      out.push({ node, depth })
      if (node.isDir && !collapsed.value.has(node.path)) walk(node.children, depth + 1)
    }
  }
  walk(tree.value, 0)
  return out
})
</script>

<template>
  <div class="border border-muted">
    <div class="flex flex-wrap items-center gap-3 border-b border-muted bg-elevated/50 px-3 py-2">
      <UIcon name="i-lucide-folder-tree" class="text-primary" />
      <span class="text-xs font-medium text-highlighted">{{ t('remote.listing.title') }}</span>
      <span class="font-mono text-[11px] text-muted">{{ listing.root }}</span>
      <span class="ml-auto text-[11px] text-muted">
        {{ t('remote.listing.selected', { count: listing.selected_files, total: totalFiles }) }}
        · {{ t('remote.listing.selected_bytes', { size: selectedBytes }) }}
      </span>
      <div v-if="!readonly" class="flex gap-1">
        <UButton size="xs" color="neutral" variant="ghost" :label="t('remote.listing.select_all')" :disabled="busy" @click="setAll(true)" />
        <UButton size="xs" color="neutral" variant="ghost" :label="t('remote.listing.select_none')" :disabled="busy" @click="setAll(false)" />
      </div>
    </div>

    <p v-if="!listing.supports_resume" class="border-b border-muted bg-warning/5 px-3 py-2 text-[11px] text-warning">
      {{ t('remote.listing.no_resume') }}
    </p>
    <p v-if="listing.truncated === 'entry_count'" class="border-b border-muted bg-warning/5 px-3 py-2 text-[11px] text-warning">
      {{ t('remote.listing.truncated_entries', { limit: 5000 }) }}
    </p>
    <p v-else-if="listing.truncated === 'depth'" class="border-b border-muted bg-warning/5 px-3 py-2 text-[11px] text-warning">
      {{ t('remote.listing.truncated_depth', { limit: 16 }) }}
    </p>

    <div class="max-h-80 overflow-y-auto">
      <div
        v-for="{ node, depth } in rows"
        :key="node.path"
        class="flex items-center gap-2 px-3 py-1.5 text-xs hover:bg-elevated/40"
        :style="{ paddingLeft: `${12 + depth * 16}px` }"
      >
        <UButton
          v-if="node.isDir"
          size="xs"
          color="neutral"
          variant="ghost"
          :icon="collapsed.has(node.path) ? 'i-lucide-chevron-right' : 'i-lucide-chevron-down'"
          @click="toggleCollapse(node.path)"
        />
        <span v-else class="w-6" />
        <UCheckbox
          :model-value="node.isDir ? folderState(node) === 'all' : !isExcluded(node.path)"
          :indeterminate="node.isDir && folderState(node) === 'some'"
          :disabled="readonly || busy"
          @update:model-value="toggle(node, $event === true)"
        />
        <UIcon :name="node.isDir ? 'i-lucide-folder' : 'i-lucide-file'" class="shrink-0 text-muted" />
        <span class="min-w-0 flex-1 truncate" :class="isExcluded(node.path) ? 'text-muted line-through' : 'text-highlighted'">
          {{ node.name }}
        </span>
        <span v-if="!node.isDir" class="shrink-0 font-mono text-[11px] text-muted">{{ formatBytes(node.size) }}</span>
      </div>
      <p v-if="!rows.length" class="p-5 text-center text-xs text-muted">{{ t('remote.listing.empty') }}</p>
    </div>
  </div>
</template>
