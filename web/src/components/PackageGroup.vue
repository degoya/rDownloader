<script setup lang="ts">
import { computed, inject, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, Download, DownloadPackage, DownloadPriority, PostprocessStep } from '@/api/types'
import NzbFileList from '@/components/NzbFileList.vue'
import PostprocessSteps from '@/components/PostprocessSteps.vue'
import { formatByteProgress, formatDuration, formatMoment, formatRate, hasExtractable, isRecoveryVolume, postprocessStageLabel, priorityItems } from '@/utils/format'
import { NO_SELECTION } from '@/utils/select'

const PRIORITY_ITEMS = computed(() => priorityItems())
const { t } = useI18n()

/**
 * The header row of one package.
 *
 * Until RD-106-12 this component also rendered the package's files. It does not any more: the
 * queue flattens itself into one stream of rows — header, then the files while the package is
 * open — so that the list can be virtualized, and a row that owns its children cannot be part
 * of such a stream. `open` therefore arrives as a prop and the toggle is reported upwards.
 */
const props = defineProps<{
  package: DownloadPackage
  /** The package's files as the active filter shows them; read for the counters and the totals. */
  downloads: Download[]
  categories: Category[]
  selection: 'none' | 'some' | 'all'
  /** Whether the view is currently rendering this package's file rows below the header. */
  open: boolean
  /** Every file of the package is finished (store getter, unaffected by the view filter). */
  complete: boolean
  /** Combined live rate of all files in the package, in bytes per second. */
  packageRate: number
  /** Seconds left for the whole package, or `null` when no figure can be measured. */
  packageEta: number | null
  dragging: boolean
  canPause: boolean
  canResume: boolean
  controlBusy: 'pause' | 'resume' | null
}>()
const emit = defineEmits<{
  select: [id: string, selected: boolean]
  category: [id: string, categoryId: string | null]
  priority: [id: string, priority: DownloadPriority]
  rename: [id: string]
  extract: [id: string]
  forceExtract: [id: string]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the package up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
  /** The chevron was used; the view decides whether the file rows are in the stream. */
  toggle: [id: string]
  deletePackage: [id: string]
  copyPath: [path: string]
  pausePackage: [id: string]
  resumePackage: [id: string]
}>()
/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('downloads.package.drag_title')} — ${t('common.a11y.reorder_keys')}`)

const hasArchive = computed(() => hasExtractable(props.downloads))
/**
 * Fields enrichers contributed to the links this package was built from (RD-107-02).
 *
 * The same chips the LinkGrabber row shows, with the same tooltip naming the plugin and the
 * time it answered: a value the application did not resolve itself has to stay recognisable
 * as somebody else's answer. Until now they lived on the candidate alone, so an auto-queueing
 * subscription showed them for the few seconds before the link moved on.
 */
const enrichment = computed(() => props.package.enrichment ?? [])
/** Empty whenever the estimate would be invented; the row then simply shows the rate. */
const etaLabel = computed(() => formatDuration(props.packageEta))
const showSteps = ref(false)
const showSegments = ref(false)
const usenet = computed(() => props.package.kind === 'usenet')
const steps = ref<PostprocessStep[]>([])
const stepsLoading = ref(false)
const loadSteps = inject<(id: string) => Promise<PostprocessStep[]>>('loadPostprocess', async () => [])

async function toggleSteps(): Promise<void> {
  showSteps.value = !showSteps.value
  if (!showSteps.value) return
  stepsLoading.value = true
  steps.value = await loadSteps(props.package.id)
  stepsLoading.value = false
}
/** Every file that is not repair data has arrived, so no repair block can still be wanted. */
const payloadComplete = computed(() => props.downloads.every(
  item => isRecoveryVolume(item) || item.state === 'completed' || item.state === 'skipped'))
/**
 * Recovery volumes that failed while the payload is complete — nobody ever asked for them.
 *
 * An expired `vol…par2` is routine on Usenet. Counting it made a package that verified and
 * unpacked cleanly report "47/48 · 1 error" at 99% (RD-107-10). While the payload is still
 * short this set is empty, so a package that genuinely needs its repair blocks keeps every
 * failure it has.
 */
const dismissed = computed(() => payloadComplete.value
  ? props.downloads.filter(item => isRecoveryVolume(item) && ['failed', 'blocked'].includes(item.state))
  : [])
const dismissedIds = computed(() => new Set(dismissed.value.map(item => item.id)))
const finished = computed(() =>
  props.downloads.filter(item => item.state === 'completed').length + dismissed.value.length)
const activeCount = computed(() => props.downloads.filter(item => ['resolving', 'downloading', 'verifying', 'repairing', 'extracting'].includes(item.state)).length)
const errorCount = computed(() => props.downloads.filter(
  item => ['failed', 'blocked'].includes(item.state) && !dismissedIds.value.has(item.id)).length)
/**
 * A mirror standing down is not work left to do: its bytes arrive through the link that is
 * running, so counting them would keep the package short of 100% for good. A recovery volume
 * nobody needed is the same case — its bytes were never owed, so they must not weigh on the
 * total either.
 */
const counted = computed(() => props.downloads.filter(
  item => item.state !== 'skipped' && !dismissedIds.value.has(item.id)))
const committed = computed(() => counted.value.reduce((sum, item) => sum + BigInt(item.committed_bytes), 0n))
const total = computed(() => counted.value.reduce((sum, item) => sum + BigInt(item.total_bytes ?? '0'), 0n))
const downloadProgress = computed(() => total.value > 0n ? Number(committed.value * 100n / total.value) : 0)
const postprocessing = computed(() => props.package.state === 'postprocessing')
const postprocessFailed = computed(() => props.package.state === 'failed')
/** Persisted unpack outcome of the last pipeline run; survives completion. */
const extraction = computed(() => props.package.extraction_result ?? null)
/** While the pipeline runs the header bar follows the stage progress instead of the download aggregate. */
const progress = computed(() => postprocessing.value ? props.package.postprocess?.percent ?? 0 : downloadProgress.value)
const stageBadge = computed(() => {
  if (!postprocessing.value) return ''
  const stage = postprocessStageLabel(props.package.postprocess?.stage)
  const percent = props.package.postprocess?.percent
  return percent === null || percent === undefined ? stage : t('downloads.package.postprocess_progress', { stage, percent })
})
const categoryItems = computed(() => [
  { label: t('downloads.bulk.default_category'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const categoryModel = computed({
  get: () => props.package.category_id ?? NO_SELECTION,
  set: (value: string) => emit('category', props.package.id, value === NO_SELECTION ? null : value)
})
/**
 * The priority as an arrow rather than a spelled-out select (RD-109-30).
 *
 * Three levels whose order is the whole meaning do not need 96px of text beside a file name
 * that has none left. The glyph is the level; the name of the level stays on the control, and
 * every level stays reachable and named inside the menu.
 */
const PRIORITY_ICONS: Record<DownloadPriority, string> = {
  high: 'i-lucide-arrow-up',
  normal: 'i-lucide-minus',
  low: 'i-lucide-arrow-down'
}
const priorityLabel = computed(() => t('downloads.package.priority_value', {
  priority: t(`common.priority.${props.package.priority}`)
}))
const priorityActions = computed(() => [PRIORITY_ITEMS.value.map(item => ({
  label: item.label,
  icon: PRIORITY_ICONS[item.value],
  onSelect: () => emit('priority', props.package.id, item.value)
}))])
/**
 * The password is worth a row's width exactly while it still opens something (RD-109-30).
 *
 * Once the archives are out, it opens nothing: the 30 characters it took were taken from the
 * name beside it. While the unpack is outstanding or has failed it stays, because that is when
 * somebody needs to read it.
 *
 * Both halves are required, and the second one is not redundant. `extraction_result` is the
 * outcome of the *last* pipeline run and survives completion, so a package that was unpacked
 * once still reports `success` while files added to it afterwards are downloading — and that
 * is precisely when the password is needed again. Hiding it on the unpack alone would take it
 * away mid-download.
 */
const downloadsDone = computed(() =>
  counted.value.length > 0 && counted.value.every(item => item.state === 'completed'))
const showPassword = computed(() =>
  props.package.has_password && !(downloadsDone.value && extraction.value === 'success'))
/**
 * Everything but the package's own start/stop control, which is the one action that is worth a
 * row's width: it acts on every file at once and it is the one somebody reaches for while the
 * package is running. The rest are deliberate acts that can afford a menu, and they keep the
 * labels they carried as `aria-label`s. The file rows below have worked this way since RD-106-12
 * — one dropdown, no loose icons — so the header now reads like its own children.
 */
const actions = computed(() => [[
  {
    label: t('downloads.package.copy_path_aria'),
    icon: 'i-lucide-folder',
    description: props.package.destination,
    onSelect: () => emit('copyPath', props.package.destination)
  },
  ...(hasArchive.value && props.complete
    ? [{
        label: t('common.actions.extract'),
        icon: 'i-lucide-package-open',
        description: t('downloads.package.extract_title'),
        onSelect: () => emit('extract', props.package.id)
      }]
    : []),
  // Only where it is the answer to something: a package whose verification failed and is
  // sitting on archives nobody is allowed to touch (RD-104-04).
  ...(hasArchive.value && postprocessFailed.value
    ? [{
        label: t('downloads.package.force_extract_aria'),
        icon: 'i-lucide-shield-alert',
        description: t('downloads.package.force_extract_title'),
        onSelect: () => emit('forceExtract', props.package.id)
      }]
    : [])
], [
  ...(usenet.value && props.package.nzb_import_id
    ? [{ label: t('downloads.package.segments_aria'), icon: 'i-lucide-layers', onSelect: () => { showSegments.value = !showSegments.value } }]
    : []),
  { label: t('downloads.package.postprocess_aria'), icon: 'i-lucide-list-checks', onSelect: () => { void toggleSteps() } },
  { label: t('downloads.package.edit_aria'), icon: 'i-lucide-pencil', onSelect: () => emit('rename', props.package.id) }
], [
  {
    label: t('downloads.package.delete_aria'),
    icon: 'i-lucide-trash-2',
    color: 'error' as const,
    description: t('downloads.package.delete_title'),
    onSelect: () => emit('deletePackage', props.package.id)
  }
]])
const packageControl = computed<'pause' | 'resume' | null>(() => {
  if (props.controlBusy) return props.controlBusy
  if (props.canPause) return 'pause'
  if (props.canResume) return 'resume'
  return null
})

function controlPackage(): void {
  if (packageControl.value === 'pause') emit('pausePackage', props.package.id)
  if (packageControl.value === 'resume') emit('resumePackage', props.package.id)
}
</script>

<template>
  <!--
    The bottom border is dropped while the package is open: its file rows are siblings in the
    flattened stream now, not children, and they carry the frame onward themselves.
  -->
  <section
    class="border bg-elevated transition"
    :class="[props.selection !== 'none' ? 'border-primary' : 'border-muted', props.dragging ? 'opacity-50' : '', props.open ? 'border-b-0' : '']"
    @dragover.prevent
    @drop.prevent="emit('drop', props.package.id)"
  >
    <header class="queue-row px-2 py-1.5" :class="props.open ? 'border-b border-muted' : ''">
      <button
        type="button"
        class="queue-cell-handle grid cursor-grab select-none place-items-center text-muted"
        data-row-handle
        draggable="true"
        :title="dragTitle"
        :aria-label="dragTitle"
        @dragstart.stop="emit('dragstart', props.package.id)"
        @keydown.up.prevent="emit('move', props.package.id, -1)"
        @keydown.down.prevent="emit('move', props.package.id, 1)"
      >
        <UIcon name="i-lucide-grip-vertical" class="size-4" />
      </button>
      <UCheckbox class="queue-cell-select justify-self-center" :model-value="props.selection === 'all' ? true : props.selection === 'some' ? 'indeterminate' : false" :aria-label="t('downloads.package.select_aria')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', props.package.id, value === true)" />
      <UButton class="queue-cell-expand" :icon="props.open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'" size="xs" color="neutral" variant="ghost" :aria-expanded="props.open" :aria-label="props.open ? t('downloads.package.hide_files') : t('downloads.package.show_files')" @click="emit('toggle', props.package.id)" />
      <div class="queue-cell-name flex min-w-0 items-center gap-2">
        <h3 class="min-w-0 truncate text-sm font-semibold text-highlighted" :title="props.package.name">{{ props.package.name }}</h3>
        <UBadge v-if="usenet" color="neutral" variant="outline" size="sm" class="shrink-0">{{ t('downloads.package.usenet') }}</UBadge>
        <span v-if="showPassword" class="flex shrink-0 items-center gap-1 text-warning" :title="t('downloads.package.password_stored')">
          <UIcon name="i-lucide-key-round" class="size-4" />
          <span v-if="props.package.password" class="max-w-32 truncate font-mono text-xs">{{ props.package.password }}</span>
        </span>
        <UBadge v-if="postprocessing" color="primary" variant="subtle" size="sm" class="numeric shrink-0" :title="props.package.postprocess?.current ?? undefined">{{ stageBadge || t('downloads.postprocess.queue.pending') }}</UBadge>
        <UBadge v-else-if="postprocessFailed" color="error" variant="subtle" size="sm" class="shrink-0">{{ extraction === 'failed' ? t('downloads.package.extract_failed') : t('downloads.package.postprocess_failed') }}</UBadge>
        <!-- Finished and unpacked are unambiguous enough to be glyphs; the word each dropped
             stays on the badge as its accessible name (RD-109-30). -->
        <UBadge v-else-if="props.complete" color="success" variant="subtle" size="sm" icon="i-lucide-circle-check" class="shrink-0" :aria-label="t('downloads.package.complete')" :title="t('downloads.package.complete_title')" />
        <UBadge v-if="!postprocessing && extraction === 'success'" color="success" variant="outline" size="sm" icon="i-lucide-package-open" class="shrink-0" :aria-label="t('downloads.package.extracted')" :title="t('downloads.package.extracted_title')" />
      </div>
      <span class="queue-cell-state numeric truncate text-xs text-muted" :title="t('downloads.package.finished_title', { finished, total: props.downloads.length })">
        {{ finished }}/{{ props.downloads.length }}
        <span v-if="activeCount" class="text-primary"> · {{ t('downloads.package.active_count', { count: activeCount }) }}</span>
        <span v-if="errorCount" class="text-error"> · {{ t('downloads.package.error_count', { count: errorCount }, errorCount) }}</span>
      </span>
      <!-- A full bar already says 100%; the number beside it is the same statement twice. -->
      <div class="queue-cell-progress items-center gap-2">
        <UProgress :model-value="progress" size="xs" class="flex-1" :color="postprocessFailed ? 'error' : 'primary'" />
        <span v-if="progress < 100" class="numeric w-9 text-right text-[11px] text-toned">{{ progress }}%</span>
      </div>
      <span class="queue-cell-size min-w-0 text-right">
        <span class="numeric block truncate text-xs text-muted">{{ formatByteProgress(committed, total) }}</span>
        <span v-if="props.packageRate > 0" class="numeric block truncate text-[10px] font-medium text-primary" :aria-label="t('downloads.package.rate_aria', { rate: formatRate(props.packageRate) })">{{ formatRate(props.packageRate) }}<span v-if="etaLabel" class="text-toned" :aria-label="t('downloads.package.eta_aria', { duration: etaLabel })"> · {{ etaLabel }}</span></span>
      </span>
      <div class="queue-cell-meta min-w-0 items-center gap-1">
        <!-- The category stays editable after the download: changing it moves the package's
             data into the new folder. The priority is history once everything is here. -->
        <USelect v-model="categoryModel" :items="categoryItems" value-key="value" size="xs" class="w-36" :aria-label="t('downloads.package.category_aria')" />
        <UDropdownMenu v-if="!props.complete" :items="priorityActions" :content="{ align: 'end' }">
          <UButton :icon="PRIORITY_ICONS[props.package.priority]" size="xs" color="neutral" variant="ghost" :aria-label="priorityLabel" :title="priorityLabel" />
        </UDropdownMenu>
      </div>
      <div class="queue-cell-actions flex items-center justify-end">
        <UButton v-if="packageControl" :icon="packageControl === 'pause' ? 'i-lucide-pause' : 'i-lucide-play'" size="xs" :color="packageControl === 'resume' ? 'primary' : 'neutral'" variant="ghost" :aria-label="packageControl === 'pause' ? t('downloads.package.pause_all_aria') : t('downloads.package.resume_all_aria')" :title="packageControl === 'pause' ? t('downloads.package.pause_title') : t('downloads.package.resume_title')" :disabled="props.controlBusy !== null" :loading="props.controlBusy !== null" @click="controlPackage" />
        <UDropdownMenu :items="actions" :content="{ align: 'end' }">
          <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('downloads.package.actions_aria')" :title="t('downloads.package.actions_aria')" />
        </UDropdownMenu>
      </div>
    </header>
    <div v-if="enrichment.length" class="flex flex-wrap items-center gap-2 border-b border-muted px-3 py-1.5">
      <UBadge
        v-for="field in enrichment"
        :key="`${field.plugin_id}:${field.name}`"
        color="neutral"
        variant="subtle"
        size="sm"
        class="font-mono"
        :title="t('downloads.package.enrichment_source', { name: field.name, at: formatMoment(field.fetched_at) })"
      >{{ field.name.split('.').pop() }}: {{ field.value }}</UBadge>
    </div>
    <div v-if="showSteps" class="border-b border-muted px-3 py-2">
      <PostprocessSteps :steps="steps" :loading="stepsLoading" />
    </div>
    <div v-if="showSegments && props.package.nzb_import_id" class="border-b border-muted px-3 py-1">
      <NzbFileList :import-id="props.package.nzb_import_id" />
    </div>
  </section>
</template>
