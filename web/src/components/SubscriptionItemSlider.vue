<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SubscriptionItem } from '@/api/types'
import SubscriptionItemCard from '@/components/SubscriptionItemCard.vue'
import SubscriptionItemDetails from '@/components/SubscriptionItemDetails.vue'
import { CARD_AUTOPLAY_MS as AUTOPLAY_MS, type CardRatio, DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

/**
 * One subscription's pending hits as a slider of cards (RD-120-37).
 *
 * Paged, not scrolled: as many cards as the width allows at 15rem each, never narrower, and the
 * page turns by the arrows, the arrow keys, a swipe or a dot. One details panel under the
 * slider shows the chosen card — the same `SubscriptionItemDetails` the list expands.
 *
 * Every hit of the subscription is in the slider, with no pagination bar under it
 * (RD-130-13): `total` is how many there are, `items` the ones read so far, and the slider asks
 * for the next fifty with `more` while the page it shows, or the one after it, reaches past
 * what it has. Only the current page is ever rendered, so a long archive costs a longer array
 * and not more cards. Past `DOTS_MAX` pages the dots give way to a counter, and the slider
 * wraps from the first page to the last only once it holds everything, since the last page of
 * a long archive would otherwise mean reading all of it first.
 *
 * Autoplay, when the subscription asks for it, turns a page every `AUTOPLAY_MS` and wraps at
 * the end. It never takes the page away from somebody using it (WCAG 2.2.2): a visible pause
 * control, and it holds while the pointer is over the slider, while anything in it has focus,
 * while the details panel is open and while the tab is hidden. Under
 * `prefers-reduced-motion: reduce` it does not run at all.
 */
const { t } = useI18n()

/** Narrowest a card may become; below this a narrow window shows fewer cards, not thinner ones. */
const CARD_MIN_PX = 240
const GAP_PX = 12
/** Horizontal travel that counts as a swipe rather than a tap. */
const SWIPE_PX = 50
/** The most pages that still get one dot each; past it the slider says "Page 3 of 40". */
const DOTS_MAX = 10

const props = withDefaults(defineProps<{
  items: SubscriptionItem[]
  label: string
  showImages?: boolean
  busyIds: string[]
  bulkBusy: boolean
  autoplay: boolean
  /** The shape of every card's picture area (RD-120-42). */
  ratio?: CardRatio
  /** How many hits the subscription holds, read or not; `items.length` when left out. */
  total?: number
}>(), { showImages: true, ratio: DEFAULT_CARD_RATIO })
const emit = defineEmits<{
  queue: [itemId: string]
  dismiss: [itemId: string]
  /** The slider is close to the end of `items` and `total` says there are more. */
  more: []
}>()

const track = ref<HTMLElement | null>(null)
const width = ref(0)
const page = ref(0)
const selectedId = ref<string | null>(null)

const perPage = computed(() => Math.max(1, Math.floor((width.value + GAP_PX) / (CARD_MIN_PX + GAP_PX))))
const count = computed(() => Math.max(props.items.length, props.total ?? 0))
const complete = computed(() => props.items.length >= count.value)
const pageCount = computed(() => Math.max(1, Math.ceil(count.value / perPage.value)))
const visible = computed(() => props.items.slice(page.value * perPage.value, (page.value + 1) * perPage.value))
/** Places on the current page whose hits are still being read. */
const pending = computed(() =>
  Math.max(0, Math.min(perPage.value, count.value - page.value * perPage.value) - visible.value.length))
const selected = computed(() => props.items.find(item => item.id === selectedId.value) ?? null)

// A queued or dismissed hit leaves the list; the page and the panel follow instead of pointing
// at something that is no longer there.
watch(pageCount, count => {
  if (page.value > count - 1) page.value = count - 1
})
watch(() => props.items, items => {
  if (selectedId.value && !items.some(item => item.id === selectedId.value)) selectedId.value = null
})
// The current page and the next one should be there before anybody turns to it. Asked again
// whenever an answer arrives, so a jump several pages ahead reads until it is covered.
watch([page, perPage, () => props.items.length, count], () => {
  if (!complete.value && props.items.length < (page.value + 2) * perPage.value) emit('more')
}, { immediate: true })

function go(target: number): void {
  const pages = pageCount.value
  page.value = ((target % pages) + pages) % pages
}
function previous(): void {
  if (page.value === 0 && !complete.value) return
  go(page.value - 1)
}
function next(): void {
  go(page.value + 1)
}

function onKeydown(event: KeyboardEvent): void {
  const target = event.target as HTMLElement | null
  if (target && ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return
  if (event.key === 'ArrowLeft') {
    event.preventDefault()
    previous()
  } else if (event.key === 'ArrowRight') {
    event.preventDefault()
    next()
  }
}

let swipeStart: number | null = null
function onPointerDown(event: PointerEvent): void {
  swipeStart = event.clientX
}
function onPointerUp(event: PointerEvent): void {
  if (swipeStart === null) return
  const travel = event.clientX - swipeStart
  swipeStart = null
  if (travel <= -SWIPE_PX) next()
  else if (travel >= SWIPE_PX) previous()
}

function toggleDetails(itemId: string): void {
  selectedId.value = selectedId.value === itemId ? null : itemId
}

function busy(itemId: string): boolean {
  return props.bulkBusy || props.busyIds.includes(itemId)
}

// ---- Autoplay and everything that holds it ----

const paused = ref(false)
const hovered = ref(false)
const focused = ref(false)
const hidden = ref(typeof document !== 'undefined' && document.visibilityState === 'hidden')
const reducedMotion = ref(false)

/** Whether autoplay is offered at all: the subscription asks for it and motion is welcome. */
const autoplayOffered = computed(() => props.autoplay && !reducedMotion.value)
const running = computed(() =>
  autoplayOffered.value
  && !paused.value
  && !hovered.value
  && !focused.value
  && !hidden.value
  && selectedId.value === null
  && pageCount.value > 1)

let timer: ReturnType<typeof setTimeout> | null = null
function stopTimer(): void {
  if (timer !== null) clearTimeout(timer)
  timer = null
}
function schedule(): void {
  stopTimer()
  if (!running.value) return
  timer = setTimeout(() => {
    timer = null
    next()
  }, AUTOPLAY_MS)
}
// A page turned by hand restarts the interval, so the next automatic turn is a full one away.
watch([running, page], schedule)

/** The pause control itself does not count as "using the slider", or pressing play would do nothing. */
function isControl(element: EventTarget | null): boolean {
  return element instanceof HTMLElement && element.closest('[data-autoplay-control]') !== null
}
function onFocusIn(event: FocusEvent): void {
  focused.value = !isControl(event.target)
}
function onFocusOut(event: FocusEvent): void {
  const into = event.relatedTarget
  const root = event.currentTarget as HTMLElement
  focused.value = into instanceof Node && root.contains(into) && !isControl(into)
}

function onVisibility(): void {
  hidden.value = document.visibilityState === 'hidden'
}

let motionQuery: MediaQueryList | null = null
function onMotion(event: MediaQueryListEvent): void {
  reducedMotion.value = event.matches
}

let observer: ResizeObserver | null = null

onMounted(() => {
  if (typeof window.matchMedia === 'function') {
    motionQuery = window.matchMedia('(prefers-reduced-motion: reduce)')
    reducedMotion.value = motionQuery.matches
    motionQuery.addEventListener?.('change', onMotion)
  }
  document.addEventListener('visibilitychange', onVisibility)
  if (track.value) {
    width.value = track.value.clientWidth
    if (typeof ResizeObserver !== 'undefined') {
      observer = new ResizeObserver(entries => {
        width.value = entries[0]?.contentRect.width ?? width.value
      })
      observer.observe(track.value)
    }
  }
  schedule()
})

onUnmounted(() => {
  stopTimer()
  observer?.disconnect()
  motionQuery?.removeEventListener?.('change', onMotion)
  document.removeEventListener('visibilitychange', onVisibility)
})

const autoplayLabel = computed(() =>
  (paused.value ? t('linkgrabber.indexers.cards.play') : t('linkgrabber.indexers.cards.pause')))
</script>

<template>
  <section
    :aria-label="t('linkgrabber.indexers.cards.label', { name: props.label })"
    :aria-roledescription="t('linkgrabber.indexers.cards.carousel')"
    data-testid="subscription-slider"
    @pointerenter="hovered = true"
    @pointerleave="hovered = false"
    @focusin="onFocusIn"
    @focusout="onFocusOut"
  >
    <div class="flex items-center gap-2">
      <UButton
        size="sm"
        color="neutral"
        variant="outline"
        icon="i-lucide-arrow-left"
        class="shrink-0 rounded-full"
        :aria-label="t('linkgrabber.indexers.cards.previous')"
        :disabled="pageCount < 2 || (page === 0 && !complete)"
        @click="previous"
      />
      <!-- Focusable so the arrow keys work before any card has been reached. -->
      <div
        ref="track"
        tabindex="0"
        class="grid min-w-0 flex-1 gap-3 touch-pan-y outline-none focus-visible:ring-2 focus-visible:ring-primary focus-visible:ring-offset-2 focus-visible:ring-offset-default"
        :style="{ gridTemplateColumns: `repeat(${perPage}, minmax(0, 1fr))` }"
        :aria-live="running ? 'off' : 'polite'"
        data-testid="slider-track"
        @keydown="onKeydown"
        @pointerdown="onPointerDown"
        @pointerup="onPointerUp"
        @pointercancel="swipeStart = null"
      >
        <SubscriptionItemCard
          v-for="item in visible"
          :key="item.id"
          :item="item"
          :show-images="props.showImages"
          :ratio="props.ratio"
          :selected="selectedId === item.id"
          :busy="busy(item.id)"
          @queue="emit('queue', item.id)"
          @dismiss="emit('dismiss', item.id)"
          @details="toggleDetails(item.id)"
        />
        <div
          v-for="index in pending"
          :key="`pending-${index}`"
          class="min-h-64 animate-pulse border border-muted bg-elevated"
          aria-hidden="true"
          data-testid="slider-pending"
        />
      </div>
      <UButton
        size="sm"
        color="neutral"
        variant="outline"
        icon="i-lucide-arrow-right"
        class="shrink-0 rounded-full"
        :aria-label="t('linkgrabber.indexers.cards.next')"
        :disabled="pageCount < 2"
        @click="next"
      />
    </div>

    <div class="mt-3 flex items-center justify-center gap-2">
      <span v-if="pageCount > DOTS_MAX" class="numeric text-xs text-muted" data-testid="slider-counter">
        {{ t('linkgrabber.indexers.cards.page', { page: page + 1, count: pageCount }) }}
      </span>
      <div v-else-if="pageCount > 1" role="group" :aria-label="t('linkgrabber.indexers.cards.pages')" class="flex items-center gap-1">
        <button
          v-for="index in pageCount"
          :key="index"
          type="button"
          class="flex h-6 items-center px-1 outline-none focus-visible:ring-2 focus-visible:ring-primary"
          :aria-label="t('linkgrabber.indexers.cards.page', { page: index, count: pageCount })"
          :aria-current="page === index - 1 ? 'true' : undefined"
          data-testid="slider-dot"
          @click="go(index - 1)"
        >
          <span
            class="block h-1.5 rounded-full"
            :class="page === index - 1 ? 'w-5 bg-primary' : 'w-1.5 bg-accented'"
          />
        </button>
      </div>
      <UButton
        v-if="autoplayOffered && pageCount > 1"
        size="xs"
        color="neutral"
        variant="ghost"
        :icon="paused ? 'i-lucide-play' : 'i-lucide-pause'"
        :aria-label="autoplayLabel"
        :title="autoplayLabel"
        data-autoplay-control
        data-testid="slider-autoplay"
        @click="paused = !paused"
      />
    </div>

    <div v-if="selected" class="mt-3 border border-muted p-3" data-testid="slider-details">
      <div class="mb-2 flex items-start gap-2">
        <span class="font-mono text-[11px] text-muted uppercase">{{ t('linkgrabber.indexers.cards.details_title') }}</span>
        <span class="min-w-0 flex-1 font-mono text-xs break-all text-highlighted">{{ selected.title }}</span>
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-x"
          :aria-label="t('linkgrabber.indexers.cards.close_details')"
          :title="t('linkgrabber.indexers.cards.close_details')"
          @click="selectedId = null"
        />
      </div>
      <SubscriptionItemDetails :attributes="selected.attributes ?? {}" :show-images="props.showImages" />
    </div>
  </section>
</template>
