<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SubscriptionItem } from '@/api/types'
import SubscriptionItemDetails from '@/components/SubscriptionItemDetails.vue'
import { useCoverPreview } from '@/composables/useCoverPreview'
import { hitCover, hitLocked, hitSize, hitTitle, promotedFacts as promotedFactsOf } from '@/utils/subscriptionHit'

/**
 * One indexer hit, in both places hits are shown (RD-101-17).
 *
 * The LinkGrabber's review list and the archive under an expanded subscription used to draw
 * their own row, and drifted apart: the same hit was a bare title in one and a bare title
 * with different buttons in the other. One row, actions supplied by the caller through the
 * slot, is what keeps them the same.
 *
 * Size and cover sit in the row itself because they decide most choices at a glance; the
 * rest is behind the chevron so a long list stays readable.
 *
 * The row wraps rather than sharing the queue grid (RD-110-27): it is drawn in the LinkGrabber's
 * review drawer and in the subscriptions page's half-width list column, and `.queue-row`
 * switches its tiers on by window width, measured for a row that spans the panel — in a column
 * half that wide it would switch on cells the row has no room for. So the title claims the
 * 200 px the accounting reserves for a name, and what does not fit beside that wraps under it.
 */
const { t } = useI18n()

const props = defineProps<{
  item: SubscriptionItem
  /** False when external image loading is switched off. */
  showImages?: boolean
}>()

const expanded = ref(false)
/** A cover whose address did not load; the row keeps a gap rather than a broken glyph. */
const coverBroken = ref(false)

const attributes = computed<Record<string, string>>(() => props.item.attributes ?? {})

/**
 * The title without the SABnzbd `{{secret}}` marker. The actual password has a dedicated,
 * consistently styled place next to the title and must not be leaked through the title too.
 */
const title = computed(() => hitTitle(props.item.title))
const expandable = computed(() => Object.entries(attributes.value).some(([name, value]) =>
  value !== '' && !['coverurl', 'imdbscore', 'genre', 'language', 'password'].includes(name)))
const thumbnail = computed(() => (coverBroken.value ? null : hitCover(attributes.value, props.showImages)))

/**
 * The cover, large, without leaving the row (RD-106-08, RD-107-16).
 *
 * A cover is what most decisions are actually made on, and at `size-12` nobody can make one.
 * It used to be shown a second time, larger, inside the expanded detail — two copies of one
 * picture, and the useful one behind a chevron. This shows it where the decision is taken
 * instead, and `design.md` was corrected in the same change rather than quietly broken.
 *
 * Three ways in, because hover is a pointer's way and not everybody has one: the pointer
 * opens it, focus opens it, and a tap pins it. `Escape` closes it, and the focus never leaves
 * the button that opened it. Nuxt UI owns the portal and collision handling; `showImages`
 * decides whether a third party's address is fetched at all.
 *
 * What the row does *not* decide any more is whether it may be the open one: two rows each
 * answering that for themselves is how two covers came to overlap. `useCoverPreview` holds
 * the single open row for the whole application; this row only reports its three inputs to it.
 */
const cover = useCoverPreview()
const coverOpen = computed(() => Boolean(thumbnail.value) && cover.open.value)
const coverLabel = computed(() =>
  (coverOpen.value ? t('subscriptions.items.hide_cover') : t('subscriptions.items.show_cover')))

/**
 * Nuxt UI reporting its own state back — and only the closing half of it is ours to act on.
 *
 * `true` used to pin the cover here, on the reading that an overlay announcing itself open
 * meant somebody had opened it. It does not: the overlay is controlled by `:open`, so it
 * reports `true` whenever *we* opened it, a hover included — and a pin outlives the pointer,
 * which is how a cover came to stand there after the pointer had long moved on. Pinning is
 * the tap's job and the tap alone (`cover.toggle()` on the trigger). The tests could not see
 * this: their popover stub never emits.
 *
 * `false` stays this row's business while this row is the open one — when another row holds
 * the single slot, the overlay is merely following our prop, and treating that as "the person
 * closed it" would throw away a pin a pointer never touched.
 */
function syncPopover(value: boolean): void {
  if (!value && cover.open.value) cover.close()
}

/**
 * A dead address: the row keeps a gap instead of a broken glyph, and gives the single open
 * slot back — a row with nothing to show must not hold it against the rest of the list.
 */
function dropCover(): void {
  coverBroken.value = true
  cover.close()
}

const size = computed(() => hitSize(attributes.value))
/** IMDb score, genre and language: shown whenever the indexer sent them (`subscriptionHit.ts`). */
const promotedFacts = computed(() => promotedFactsOf(attributes.value, t))
const locked = computed(() => hitLocked(attributes.value))

const detailsLabel = computed(() =>
  (expanded.value ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')))

</script>

<template>
  <li class="border border-muted">
    <div class="flex flex-wrap items-center gap-2 p-2">
      <UButton
        v-if="expandable"
        :icon="expanded ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        size="xs"
        color="neutral"
        variant="ghost"
        class="shrink-0"
        :aria-expanded="expanded"
        :aria-label="detailsLabel"
        :title="detailsLabel"
        @click="expanded = !expanded"
      />
      <!-- Keeps the titles aligned when a neighbour has details and this one has none. -->
      <span v-else class="size-6 shrink-0" />

      <!--
        No fade on the way out, and a key on the picture (RD-108-31).

        Every row carries its own overlay, and the enlarged pictures of two neighbouring rows
        land within a thumbnail's height of each other - so while the leaving row's content is
        still finishing its 100 ms `scale-out`, what a person crossing the list sees is one
        picture in one place, showing the row they have already left. `animate-none` on the
        closed state lets the old content go in the same tick the row loses the slot, because
        the overlay only waits for an animation that is actually running. The `:key` settles
        the other half: an `img` whose `src` changes keeps the pixels it has until the new
        address decodes, so the picture is a new element rather than the old one repainted.
      -->
      <UPopover
        v-if="thumbnail"
        :open="coverOpen"
        mode="click"
        :content="{ side: 'right', align: 'start', collisionPadding: 12 }"
        :ui="{
          content:
            'border border-muted bg-default p-1 shadow-lg data-[state=closed]:animate-none'
        }"
        @update:open="syncPopover"
      >
        <button
          type="button"
          class="block shrink-0 cursor-zoom-in"
          :aria-label="coverLabel"
          :title="coverLabel"
          :aria-expanded="coverOpen"
          @mouseenter="cover.setHovered(true)"
          @mouseleave="cover.setHovered(false)"
          @focus="cover.setFocused(true)"
          @blur="cover.setFocused(false)"
          @click="cover.toggle()"
          @keydown.esc="cover.close()"
        >
          <img
            :src="thumbnail"
            alt=""
            loading="eager"
            decoding="async"
            class="size-12 bg-elevated object-cover"
            @error="dropCover"
          >
        </button>
        <template #content>
          <img
            :key="thumbnail"
            :src="thumbnail"
            :alt="title"
            class="pointer-events-none h-[calc(var(--reka-popover-content-available-height)-0.75rem)] w-auto max-w-[min(48rem,calc(100vw-2rem))] bg-elevated object-contain"
            data-testid="cover-preview"
            @error="dropCover"
          >
        </template>
      </UPopover>
      <!-- No cover, no gap: without a stand-in the titles of a list start in two different
           places, which is what makes a long list read as unruly. Decoration, so it is hidden
           from screen readers and carries the application's own mark rather than a picture
           that pretends to be of the thing. -->
      <span
        v-else
        aria-hidden="true"
        data-testid="cover-placeholder"
        class="size-12 shrink-0 bg-elevated bg-[length:1.5rem] bg-center bg-no-repeat"
        style="background-image: url(/favicon.svg)"
      />

      <!-- Grows, shrinks, but starts from 200 px: below that the rest wraps under it. -->
      <div class="min-w-0 grow shrink basis-[200px]">
        <p class="truncate text-sm text-highlighted" :title="title">{{ title }}</p>
        <dl v-if="promotedFacts.length" class="flex flex-wrap gap-x-3 gap-y-0.5">
          <div v-for="fact in promotedFacts" :key="fact.label" class="flex items-baseline gap-1">
            <dt class="text-[11px] text-muted">{{ fact.label }}</dt>
            <dd class="numeric text-[11px] text-highlighted">{{ fact.value }}</dd>
          </div>
        </dl>
      </div>

      <span
        v-if="props.item.password"
        class="flex max-w-40 shrink-0 items-center gap-1 text-warning"
        :title="t('subscriptions.items.password_protected')"
        :aria-label="t('subscriptions.items.password_protected')"
      >
        <UIcon name="i-lucide-key-round" class="size-3.5 shrink-0" />
        <span class="truncate font-mono text-xs">{{ props.item.password }}</span>
      </span>
      <UIcon
        v-else-if="locked"
        name="i-lucide-lock"
        class="size-3.5 shrink-0 text-muted"
        :title="t('subscriptions.items.password_protected')"
        :aria-label="t('subscriptions.items.password_protected')"
      />
      <span v-if="size" class="numeric shrink-0 text-xs text-muted">{{ size }}</span>

      <slot name="actions" />
    </div>
    <!-- Why a hit was skipped is a sentence, and a sentence in the row is taken from the title. -->
    <p v-if="props.item.reason" class="px-2 pb-2 text-xs text-muted">{{ t(`subscriptions.reasons.${props.item.reason}`) }}</p>

    <div v-if="expanded" class="border-t border-muted px-2 py-2">
      <SubscriptionItemDetails :attributes="attributes" :show-images="props.showImages" />
    </div>
  </li>
</template>
