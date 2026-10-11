<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LinkCandidate, MediaVariant } from '@/api/types'
import CollectorCandidateRequest from '@/components/CollectorCandidateRequest.vue'
import CollectorCandidateSources from '@/components/CollectorCandidateSources.vue'
import CollectorMirrorBadge from '@/components/CollectorMirrorBadge.vue'
import DragHandle from '@/components/DragHandle.vue'
import EnrichmentChips from '@/components/EnrichmentChips.vue'
import MediaFormatSelector from '@/components/MediaFormatSelector.vue'
import RemoteFileTree from '@/components/RemoteFileTree.vue'
import TorrentFileTree from '@/components/TorrentFileTree.vue'
import { downloadedBefore, queuedCount } from '@/composables/useQueuedSources'
import { useAccountProviders } from '@/composables/useAccountProviders'
import { useCandidateDetails } from '@/composables/useCandidateDetails'
import { translateServerMessage } from '@/i18n/server'
import type { MirrorGroup } from '@/utils/mirrorGroups'
import { displayName, hosterOf } from '@/utils/collectorSort'
import { formatBytes, formatDuration, formatMoment } from '@/utils/format'
import { isEnqueueable } from '@/utils/candidateState'
import { sourcePageUrl } from '@/utils/sourcePage'

const { t } = useI18n()
const props = defineProps<{
  candidate: LinkCandidate,
  selected: boolean,
  busy: boolean,
  /** Set on the row that stands for a whole mirror group; `null` on a lone link (RD-110-19). */
  mirrorGroup?: MirrorGroup | null,
  /** Whether the group's other mirrors are showing. */
  mirrorOpen?: boolean,
  /** Set on a row that is one of those other mirrors rather than a candidate of its own. */
  mirrorMember?: boolean,
  /** The view's "Show metadata" switch is off: the enricher chips stay stored, only unshown (RD-150-19). */
  hideMetadata?: boolean
}>()
const { lacksAccount, providerName } = useAccountProviders()
/** Hoster link that will be fetched without any account — surfaced instead of failing later. */
const freeDownload = computed(() => !props.candidate.route?.account_id && lacksAccount(props.candidate.provider))
const emit = defineEmits<{
  select: [id: string, selected: boolean]
  rename: [id: string]
  enqueue: [id: string]
  remove: [id: string]
  dragstart: [id: string]
  drop: [id: string]
  /** Keyboard alternative to the drag: -1 moves the link up, 1 moves it down. */
  move: [id: string, delta: -1 | 1]
  variant: [id: string, variantId: string]
  /** Show or hide the other mirrors of this group. */
  'toggle-mirror': [key: string]
  /** Make this mirror the group's chosen one, or hand the group back to the preference. */
  'choose-mirror': [id: string, chosen: boolean]
  /** Take a proposed group apart, because its links are not the same file (RD-110-34). */
  'dissolve-mirror': [id: string]
  /** Hide every link of this hoster from the LinkGrabber (RD-130-21). */
  'hide-hoster': [hoster: string]
  /** Show a link a LinkFilter rule hid, until the rules are applied again (RD-1240-09). */
  unhide: [id: string]
  /** Addresses onto the clipboard; the view copies and says so (RD-190-21). */
  'copy-links': [links: string[]]
}>()
/** What the handle announces: the drag, and the keys that do the same without a mouse. */
const dragTitle = computed(() => `${t('linkgrabber.candidate.drag_hint')} — ${t('common.a11y.reorder_keys')}`)

const stateColor = computed<'success' | 'error' | 'warning' | 'neutral' | 'primary'>(() => {
  switch (props.candidate.state) {
    case 'online': return 'success'
    case 'offline': return 'error'
    case 'duplicate': return 'warning'
    case 'checking': return 'primary'
    case 'error': return 'error'
    case 'unsupported': return 'warning'
    // Not `warning`, which is what every "nothing good is known yet" state wears: this row
    // is a dead end, and it reads like one (RD-110-07).
    case 'unresolvable': return 'error'
    default: return 'neutral'
  }
})
const stateLabel = computed(() => t(`linkgrabber.candidate.state.${props.candidate.state}`))
// One mark per address (owner, 2026-10-10): the orange "already in the list" state wins over
// both blue badges, which would repeat it for the same address (RD-1240-28, live test 10), so
// they show only when the address is in neither the LinkGrabber nor the list.
const duplicate = computed(() => props.candidate.state === 'duplicate')
/** Queue downloads of the same source, by identity rather than by spelling (RD-150-01). */
const queuedCopies = computed(() => duplicate.value ? 0 : queuedCount(props.candidate.url))
/** The history's package of the same source, when the setting compares with it (RD-1240-14). */
const earlier = computed(() => duplicate.value ? null : downloadedBefore(props.candidate.url))
const earlierHint = computed(() => earlier.value
  ? t('linkgrabber.duplicates.history_hint', { name: earlier.value.name, date: formatMoment(earlier.value.finished_at) })
  : '')
/**
 * The values a coded candidate message interpolates.
 *
 * The host is a parameter and never a sentence the backend assembled: it is the candidate's
 * own address, which the row already has, so `collector.check_no_resolver` can name it in four
 * languages without a column to carry it (RD-120-18).
 */
const messageParams = computed(() => ({ host: hosterOf(props.candidate) }))
/**
 * What the state badge says beyond its one word.
 *
 * `duplicate` was the only state that named no reference: two links of two hosters carrying it
 * looked like a collision in the dedup key, while the key is the full address compared over
 * the whole table and both had simply been added before (RD-109-43). `unresolvable` needs the
 * same courtesy for the opposite reason: it is the one state the row offers no way out of, so
 * it has to say why rather than leave the greyed-out checkbox to be guessed at (RD-110-07).
 */
const stateHint = computed(() => {
  if (props.candidate.state === 'duplicate') return t('linkgrabber.candidate.duplicate_hint')
  if (props.candidate.state !== 'unresolvable') return undefined
  // Two reasons end in one state, and they send the reader to opposite ends (RD-120-18): the
  // address really serves no file, or no installed plugin claims its host. The backend told
  // them apart, so the hint has to repeat that rather than blame the address in both cases.
  return props.candidate.error_code === 'collector.check_no_resolver'
    ? t('linkgrabber.candidate.no_resolver_hint', messageParams.value)
    : t('linkgrabber.candidate.unresolvable_hint')
})
/**
 * The check's message. A stable code the catalogue answers, or — for a row written before the
 * codes existed, or a sentence a plugin worded itself — the English text that came with it.
 */
const candidateError = computed(() => props.candidate.error
  ? translateServerMessage({
    message: props.candidate.error,
    code: props.candidate.error_code ?? null,
    params: messageParams.value
  })
  : '')
// Unchecked links stay selectable: the user may still start them, and the download now
// reports a real error instead of silently storing the hoster's landing page.
const selectable = computed(() => isEnqueueable(props.candidate.state))
const {
  request, media, expanded, expandable, expand,
  consent, consentBusy, withdrawConsent,
  torrent, torrentDetail, torrentBusy, torrentError, savePlan,
  listing, listingDetail, listingBusy, listingError, saveListingPlan,
  mediaFormats, mediaBusy, mediaError, selectorRef, previewMedia, resolveOutput,
  authProfiles, applyAuthProfile, applyMedia, sources
} = useCandidateDetails(() => props.candidate)
/**
 * Fields an enricher plugin added. Shown with their source and their age, because a value the
 * application did not resolve itself has to be recognisable as somebody else's answer — and a
 * stale one as stale.
 */
const enrichment = computed(() => props.candidate.enrichment ?? [])
/**
 * When a provider last said it holds the file in its cache (RD-120-36). Shown as a chip with
 * that time and neutral in colour, because it is a measurement that expires without notice,
 * not a promise the download will be instant.
 */
const cachedAt = computed(() => props.candidate.cached_at ?? null)
/**
 * Which provider gave that cache answer, by slug (RD-130-11); named in the chip's tooltip.
 * `null` for an answer stamped before the server recorded it — then nobody is named.
 */
const cachedBy = computed(() => props.candidate.cached_by ?? null)
const cacheHint = computed(() => {
  if (!cachedAt.value) return ''
  const at = formatMoment(cachedAt.value)
  return cachedBy.value
    ? t('linkgrabber.cache.hint_by', { at, provider: providerName(cachedBy.value) })
    : t('linkgrabber.cache.hint', { at })
})
const selectedVariant = computed(() => media.value?.variants.find(variant => variant.id === media.value?.selected) ?? null)
const mediaIcon = computed(() => selectedVariant.value?.kind === 'audio' ? 'i-lucide-music' : 'i-lucide-clapperboard')
const variantItems = computed(() => (media.value?.variants ?? []).map(variant => ({ label: variantLabel(variant), value: variant.id })))
const variantModel = computed({
  get: () => media.value?.selected ?? '',
  set: (value: string) => { if (value && value !== media.value?.selected) emit('variant', props.candidate.id, value) }
})
const hasMp3 = computed(() => media.value?.variants.some(variant => variant.id === 'audio_mp3') ?? false)
const isMp3 = computed(() => media.value?.selected === 'audio_mp3')
/** The name the rename dialog writes and the downloader uses — never the yt-dlp page title. */
const rowName = computed(() => displayName(props.candidate))
const rowTitle = computed(() => media.value?.title ? `${media.value.title}\n${props.candidate.url}` : props.candidate.url)
/** The row shows the file name, so the page title moves here — dropped when it is the file name. */
const mediaMeta = computed(() => {
  if (!media.value) return ''
  const title = media.value.title !== rowName.value ? media.value.title : ''
  return [title, formatDuration(media.value.duration_seconds), media.value.uploader].filter(Boolean).join(' · ')
})

/**
 * The page the link came from: the referrer of a captured browser download, else the page a
 * media link was extracted from. A link nobody saw on a page offers nothing (RD-190-21).
 */
const sourcePage = computed(() => sourcePageUrl(request.value?.referrer, media.value?.page_url))
/** A group row stands for all its mirrors, so it copies all of them; any other row its own. */
const rowLinks = computed(() => props.mirrorGroup ? props.mirrorGroup.members.map(member => member.url) : [props.candidate.url])

function variantLabel(variant: MediaVariant): string {
  const parts = [variant.label]
  if (variant.fps && variant.fps > 30) parts.push(`${variant.fps}fps`)
  if (variant.dynamic_range && variant.dynamic_range !== 'sdr' && variant.dynamic_range !== 'unknown') {
    parts.push(t(`linkgrabber.media.ranges.${variant.dynamic_range}`))
  }
  if (variant.video_codec && variant.video_codec !== 'other') parts.push(t(`linkgrabber.media.codecs.${variant.video_codec}`))
  if (variant.filesize_approx) parts.push(`≈${formatBytes(BigInt(variant.filesize_approx))}`)
  return parts.join(' · ')
}
/**
 * Everything but the enqueue button, which is the one action worth a row's width: queueing the
 * link is what the review is for, and it is what somebody reaches for while working down the
 * list. Rename, the MP3 switch and delete are deliberate acts that can afford a menu, and they
 * keep the labels they carried as `aria-label`s (RD-110-27, the rule of RD-109-30). A right-click
 * on the row opens the same entries (`UContextMenu`, RD-1240-14).
 */
const actions = computed(() => [[
  // A mirror group takes the row's chevron for its members, so the detail panel — which no
  // hoster mirror has anyway — moves in here rather than losing its way in.
  ...(props.mirrorGroup && expandable.value
    ? [{ label: t('linkgrabber.actions.details'), icon: 'i-lucide-list-tree', onSelect: () => void expand() }]
    : []),
  ...(props.mirrorGroup?.pinned
    ? [{
        label: t('linkgrabber.mirror.release'),
        icon: 'i-lucide-pin-off',
        onSelect: () => emit('choose-mirror', props.candidate.id, false)
      }]
    : []),
  // Only a proposal offers the way out of itself. A declared group and one two sizes agree on
  // are not wrong in a way one package can fix, so the entry is absent rather than disabled:
  // a dead control reads as a broken feature, and the server refuses those two anyway.
  ...(props.mirrorGroup?.source === 'name'
    ? [{
        label: t('linkgrabber.mirror.dissolve'),
        icon: 'i-lucide-ungroup',
        onSelect: () => emit('dissolve-mirror', props.candidate.id)
      }]
    : []),
  ...(media.value && hasMp3.value
    ? [{
        label: t('linkgrabber.media.mp3_hint'),
        icon: 'i-lucide-music',
        disabled: props.busy || isMp3.value,
        onSelect: () => emit('variant', props.candidate.id, 'audio_mp3')
      }]
    : []),
  { label: t('common.actions.rename'), icon: 'i-lucide-pencil', onSelect: () => emit('rename', props.candidate.id) },
  {
    label: rowLinks.value.length > 1 ? t('common.actions.copy_links') : t('common.actions.copy_link'),
    icon: 'i-lucide-link',
    onSelect: () => emit('copy-links', rowLinks.value)
  },
  ...(sourcePage.value
    ? [{ label: t('common.actions.open_source_page'), icon: 'i-lucide-external-link', to: sourcePage.value, target: '_blank' }]
    : []),
  // JDownloader's "hide links of this hoster": the row is where somebody notices a hoster they
  // do not want, so the way to hide it is where the noticing happens (RD-130-21).
  ...(messageParams.value.host
    ? [{
        label: t('linkgrabber.hosters.hide', { host: messageParams.value.host }),
        icon: 'i-lucide-eye-off',
        onSelect: () => emit('hide-hoster', messageParams.value.host)
      }]
    : []),
  ...(props.candidate.hidden_by_filter
    ? [{ label: t('linkgrabber.link_filter.unhide'), icon: 'i-lucide-eye', onSelect: () => emit('unhide', props.candidate.id) }]
    : [])
], [
  {
    label: t('linkgrabber.actions.delete_link'),
    icon: 'i-lucide-trash-2',
    color: 'error' as const,
    disabled: props.candidate.state === 'resolving',
    onSelect: () => emit('remove', props.candidate.id)
  }
]])
const mirrorToggleLabel = computed(() => props.mirrorOpen
  ? t('linkgrabber.mirror.collapse')
  : t('linkgrabber.mirror.expand'))
</script>

<template>
  <!--
    One root, so the frame the view hands down (`linkFrame`) has somewhere to land; the row
    is the shared queue grid, so it wraps where the queue rows wrap (RD-110-27).
  -->
  <div
    class="group transition hover:bg-elevated/60"
    :class="props.selected ? 'bg-primary/5' : ''"
    @dragover.prevent
    @drop.prevent.stop="emit('drop', props.candidate.id)"
  >
    <UContextMenu :items="actions">
      <div class="queue-row px-2 py-1.5" :class="props.mirrorMember ? 'pl-8' : ''">
        <!--
          A member row is not a second candidate: queueing, selecting and reordering belong to
          the group, which is the one thing that will be downloaded. So it carries no handle and
          no checkbox, and the cells stay empty rather than disappearing, or the grid loses its
          shape (RD-110-19).
        -->
        <DragHandle
          v-if="!props.mirrorMember"
          class="queue-cell-handle grid place-items-center"
          :label="dragTitle"
          @dragstart="emit('dragstart', props.candidate.id)"
          @move="(delta: -1 | 1) => emit('move', props.candidate.id, delta)"
        />
        <div v-else class="queue-cell-handle" />
        <div v-if="props.mirrorMember" class="queue-cell-select" />
        <UCheckbox v-else-if="selectable" class="queue-cell-select justify-self-center" :model-value="props.selected" :aria-label="t('linkgrabber.candidate.select')" @update:model-value="(value: boolean | 'indeterminate') => emit('select', props.candidate.id, value === true)" />
        <UIcon v-else name="i-lucide-link-2" class="queue-cell-select size-4 justify-self-center text-muted" />
        <UButton
          v-if="props.mirrorGroup"
          class="queue-cell-expand"
          :icon="props.mirrorOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
          size="xs"
          color="neutral"
          variant="ghost"
          :aria-expanded="props.mirrorOpen"
          :aria-label="mirrorToggleLabel"
          :title="mirrorToggleLabel"
          @click="emit('toggle-mirror', props.mirrorGroup.key)"
        />
        <UButton
          v-else-if="expandable && !props.mirrorMember"
          class="queue-cell-expand"
          :icon="expanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
          size="xs"
          color="neutral"
          variant="ghost"
          :aria-expanded="expanded"
          :aria-label="t('linkgrabber.actions.details')"
          :title="t('linkgrabber.actions.details')"
          @click="expand"
        />
        <div class="queue-cell-name flex min-w-0 items-center gap-2">
          <img v-if="media?.thumbnail" :src="media.thumbnail" alt="" loading="lazy" class="size-12 shrink-0 bg-elevated object-cover">
          <div class="min-w-0 flex-1">
            <UButton variant="link" color="neutral" class="flex w-full p-0 text-left text-sm font-normal text-highlighted hover:text-highlighted hover:underline" :label="rowName" :title="rowTitle" @click="emit('rename', props.candidate.id)" />
            <p v-if="mediaMeta" class="truncate text-xs text-muted">{{ mediaMeta }}</p>
          </div>
          <!-- Each of these is one idea with one icon; the word it dropped is its name. -->
          <UBadge v-if="props.candidate.provider === 'media'" color="primary" variant="outline" size="sm" class="shrink-0" role="img" :icon="mediaIcon" :title="t('linkgrabber.media.badge')" :aria-label="t('linkgrabber.media.badge')" />
          <UBadge v-if="torrent" color="primary" variant="outline" size="sm" class="shrink-0" :title="t('torrent.tree.files_badge', { selected: torrent.selected_count, total: torrent.file_count })" :aria-label="t('torrent.tree.files_badge', { selected: torrent.selected_count, total: torrent.file_count })">
            <UIcon name="i-lucide-list-tree" class="mr-1 size-3.5" />{{ torrent.selected_count }}/{{ torrent.file_count }}
          </UBadge>
          <UBadge v-if="consent" color="warning" variant="subtle" size="sm" class="shrink-0" role="img" icon="i-lucide-shield-check" :title="t('linkgrabber.replay.consent.granted')" :aria-label="t('linkgrabber.replay.consent.granted')" />
          <UBadge v-if="queuedCopies" color="info" variant="subtle" size="sm" class="shrink-0" role="img" icon="i-lucide-list-checks" :title="t('linkgrabber.duplicates.queued_hint', { count: queuedCopies })" :aria-label="t('linkgrabber.duplicates.queued', { count: queuedCopies })" data-testid="queued-badge" />
          <UBadge v-if="earlier" color="info" variant="subtle" size="sm" class="shrink-0" role="img" icon="i-lucide-history" :title="earlierHint" :aria-label="t('linkgrabber.duplicates.history')" data-testid="history-badge" />
          <UBadge v-if="freeDownload" color="warning" variant="subtle" size="sm" class="shrink-0" role="img" icon="i-lucide-user-x" :title="t('linkgrabber.candidate.no_account')" :aria-label="t('linkgrabber.candidate.no_account')" />
          <UBadge v-if="props.candidate.hidden_by_filter" color="neutral" variant="outline" size="sm" class="shrink-0" role="img" icon="i-lucide-filter" :title="t('linkgrabber.link_filter.hidden_hint')" :aria-label="t('linkgrabber.link_filter.hidden')" data-testid="filter-hidden-badge" />
          <UBadge v-if="sources.length" color="neutral" variant="subtle" size="sm" class="shrink-0" role="img" icon="i-lucide-layers" :title="t('linkgrabber.sources.badge', { count: sources.length })" :aria-label="t('linkgrabber.sources.badge', { count: sources.length })" data-testid="candidate-sources-badge" />
          <!-- The group, and how sure it is. The word changes with the evidence, not only the
               colour: a proposal that merely looked different would read as a fact to anybody
               who does not see the difference (RD-110-19). -->
          <CollectorMirrorBadge v-if="props.mirrorGroup" :group="props.mirrorGroup" />
          <UBadge v-if="props.mirrorGroup && props.mirrorGroup.onlineCount === 0" color="error" variant="soft" size="sm" class="shrink-0" role="img" icon="i-lucide-cloud-off" :title="t('linkgrabber.mirror.all_offline_hint', { count: props.mirrorGroup.members.length })" :aria-label="t('linkgrabber.mirror.all_offline')" />
          <UBadge v-if="props.mirrorGroup?.pinned" color="neutral" variant="outline" size="sm" class="shrink-0" role="img" icon="i-lucide-pin" :title="t('linkgrabber.mirror.pinned_hint')" :aria-label="t('linkgrabber.mirror.pinned')" />
        </div>
        <!-- Kept as a word: `check failed` and `not checked` need their qualifier, and a list that
             mixes glyph states with worded ones reads as two systems. The cell is sized for it. -->
        <span class="queue-cell-state min-w-0">
          <UBadge :color="stateColor" variant="subtle" size="sm" class="max-w-full truncate" :title="stateHint">
            <UIcon v-if="props.candidate.state === 'checking'" name="i-lucide-loader-circle" class="mr-1 size-3 animate-spin" />{{ stateLabel }}
          </UBadge>
        </span>
        <!-- A link under review has no progress; the cell stays so the grid keeps its shape. -->
        <div class="queue-cell-progress" />
        <!-- A size nobody measured prints nothing, not a dash. -->
        <span class="queue-cell-size numeric min-w-0 truncate text-right text-xs text-muted">{{ props.candidate.size ? formatBytes(props.candidate.size) : '' }}</span>
        <div class="queue-cell-meta min-w-0 items-center">
          <USelect v-if="media" v-model="variantModel" :items="variantItems" value-key="value" size="xs" class="w-full" :aria-label="t('linkgrabber.media.variant')" :disabled="props.busy" />
          <UBadge v-else color="neutral" variant="outline" size="sm" class="max-w-full truncate font-mono">{{ hosterOf(props.candidate) }}</UBadge>
        </div>
        <div class="queue-cell-actions flex items-center justify-end opacity-70 transition group-hover:opacity-100">
          <!-- The one action the expansion exists for, labelled because it is not self-evident. -->
          <UButton
            v-if="props.mirrorMember"
            icon="i-lucide-check"
            :label="t('linkgrabber.mirror.use')"
            size="xs"
            color="neutral"
            variant="outline"
            :aria-label="t('linkgrabber.mirror.use')"
            :title="t('linkgrabber.mirror.use_hint')"
            @click="emit('choose-mirror', props.candidate.id, true)"
          />
          <UButton v-else icon="i-lucide-arrow-down-to-line" size="xs" color="primary" variant="ghost" :aria-label="t('linkgrabber.actions.enqueue_link')" :title="t('linkgrabber.actions.enqueue_link')" :disabled="!selectable || props.busy" :loading="props.busy" @click="emit('enqueue', props.candidate.id)" />
          <UDropdownMenu :items="actions" :content="{ align: 'end' }">
            <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('linkgrabber.actions.link_actions')" :title="t('linkgrabber.actions.link_actions')" />
          </UDropdownMenu>
        </div>
      </div>
    </UContextMenu>
    <p v-if="candidateError" class="truncate px-12 pb-1 text-xs text-error" :title="candidateError">{{ candidateError }}</p>
    <div v-if="(enrichment.length && !props.hideMetadata) || cachedAt" class="flex flex-wrap items-center gap-2 px-12 pb-1">
      <UBadge
        v-if="cachedAt"
        color="neutral"
        variant="subtle"
        size="sm"
        class="font-mono"
        data-testid="candidate-cached"
        :title="cacheHint"
      >{{ t('linkgrabber.cache.label') }}: {{ formatMoment(cachedAt) }}</UBadge>
      <EnrichmentChips v-if="!props.hideMetadata" :fields="enrichment" />
    </div>
    <div v-if="sources.length && expanded" class="border-t border-muted px-12 py-2">
      <CollectorCandidateSources :sources="sources" />
    </div>
    <div v-if="torrent && expanded" class="border-t border-muted px-12 py-2">
      <p v-if="torrentBusy && !torrentDetail" class="flex items-center gap-2 text-xs text-muted">
        <UIcon name="i-lucide-loader-circle" class="size-3.5 animate-spin" />{{ t('torrent.tree.loading') }}
      </p>
      <p v-else-if="torrentError" class="text-xs text-error">{{ torrentError }}</p>
      <TorrentFileTree
        v-else-if="torrentDetail?.plan"
        :plan="torrentDetail.plan"
        :capabilities="torrentDetail.capabilities"
        :busy="torrentBusy"
        @change="savePlan"
      />
      <p v-else class="text-xs text-muted">{{ t('torrent.tree.empty') }}</p>
    </div>
    <div v-if="media && expanded" class="border-t border-muted px-12 py-3">
      <p v-if="mediaBusy && !mediaFormats" class="flex items-center gap-2 text-xs text-muted">
        <UIcon name="i-lucide-loader-circle" class="size-3.5 animate-spin" />{{ t('linkgrabber.media.loading') }}
      </p>
      <p v-else-if="mediaError" class="text-xs text-muted">{{ mediaError }}</p>
      <MediaFormatSelector
        v-else-if="mediaFormats"
        ref="selectorRef"
        :formats="mediaFormats"
        :busy="mediaBusy || props.busy"
        :resolve-output="resolveOutput"
        :auth-profiles="authProfiles"
        :auth-selection="props.candidate.auth_profile ?? null"
        :url="props.candidate.url"
        @preview="previewMedia"
        @apply="applyMedia"
        @auth-profile="applyAuthProfile"
      />
    </div>
    <div v-if="listing && expanded" class="border-t border-muted px-12 py-2">
      <p v-if="listingBusy && !listingDetail" class="flex items-center gap-2 text-xs text-muted">
        <UIcon name="i-lucide-loader-circle" class="size-3.5 animate-spin" />{{ t('remote.listing.loading') }}
      </p>
      <p v-else-if="listingError" class="text-xs text-error">{{ listingError }}</p>
      <p v-else-if="listingDetail?.single_file" class="text-xs text-muted">{{ t('remote.listing.single_file') }}</p>
      <RemoteFileTree
        v-else-if="listingDetail"
        :listing="listingDetail"
        :busy="listingBusy"
        @change="saveListingPlan"
      />
    </div>
    <CollectorCandidateRequest
      v-if="request && expanded"
      :request="request"
      :consent="consent"
      :consent-busy="consentBusy"
      @withdraw="withdrawConsent"
    />
  </div>
</template>
