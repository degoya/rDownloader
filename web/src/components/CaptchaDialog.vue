<script setup lang="ts">
import { useToast } from '@nuxt/ui/composables'
import { computed, onBeforeUnmount, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { useRouter } from 'vue-router'

import { useCaptchasStore } from '@/stores/captchas'

const captchas = useCaptchasStore()
const { t } = useI18n()
const toast = useToast()
const router = useRouter()

const answer = ref('')
/**
 * The spot marked in a click-point captcha, in pixels of the image as the hoster served it.
 * The rendering may be scaled to fit the dialog; the hoster only knows its own pixels.
 */
const point = ref<{ x: number, y: number } | null>(null)
const now = ref(Date.now())
/**
 * A widget challenge the user stepped away from to configure a solver: the dialog blocks the
 * page, so it has to get out of the way. The challenge stays pending and simply expires.
 */
const dismissedId = ref<string | null>(null)
let ticker: number | null = null

const current = computed(() => captchas.current)
const open = computed(() => current.value !== null && current.value.id !== dismissedId.value)
/** A click-point captcha is a picture answered by a click rather than by text (RD-110-15). */
const isClick = computed(() => current.value?.kind === 'click_point')
/**
 * Both kinds a person answers here by looking at a picture. Widget challenges are bound to
 * the hoster's origin and cannot be rendered here.
 */
const isImage = computed(() => current.value?.kind === 'image' || isClick.value)
/** The image's own size, learnt from the element at the first click or key press. */
const natural = ref<{ width: number, height: number } | null>(null)
const imageRef = ref<HTMLImageElement | null>(null)
/** How far one arrow key moves the mark, in image pixels; Shift makes it ten. */
const KEY_STEP = 1
const KEY_STEP_LARGE = 10
/** Where the mark is drawn, as a share of the image so it survives any scaling of it. */
const markerStyle = computed(() => {
  const chosen = point.value
  const size = natural.value
  if (!chosen || !size) return null
  return {
    left: `${(chosen.x / size.width) * 100}%`,
    top: `${(chosen.y / size.height) * 100}%`
  }
})
const canSubmit = computed(() => isClick.value ? point.value !== null : answer.value.trim() !== '')
/**
 * Whether a browser extension has polled the server recently. The server is the only party
 * that can know: the extension may live in another browser on another machine (RD-108-02).
 */
const extensionConnected = computed(() => captchas.answerers?.browser_extension_connected === true)
/** How often, in ticks of the one-second countdown, the extension question is asked again. */
const ANSWERERS_EVERY_TICKS = 15
let ticks = 0
const kindLabel = computed(() => current.value ? t(`captcha.kinds.${current.value.kind}`) : '')
const hostLabel = computed(() => current.value?.host || t('captcha.unknown_host'))
const remainingSeconds = computed(() => {
  const expires = current.value ? Date.parse(current.value.expires_at) : Number.NaN
  if (!Number.isFinite(expires)) return null
  return Math.max(0, Math.round((expires - now.value) / 1000))
})
const remainingLabel = computed(() => {
  const seconds = remainingSeconds.value
  if (seconds === null) return null
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`
})

// A fresh challenge gets an empty field and no mark; the countdown only ticks while one is shown.
watch(() => current.value?.id, () => {
  answer.value = ''
  point.value = null
  natural.value = null
  dismissedId.value = null
})
watch(open, (shown) => {
  if (shown && ticker === null) {
    now.value = Date.now()
    ticks = 0
    ticker = window.setInterval(() => {
      now.value = Date.now()
      ticks += 1
      // A widget waits on a browser extension; ask now and then whether one has appeared.
      if (!isImage.value && ticks % ANSWERERS_EVERY_TICKS === 0) void captchas.refreshAnswerers()
    }, 1_000)
  } else if (!shown && ticker !== null) {
    window.clearInterval(ticker)
    ticker = null
  }
}, { immediate: true })
// The hint for a widget depends on whether an extension is around, so ask as soon as one shows.
watch(() => open.value && !isImage.value, (widgetShown) => {
  if (widgetShown) void captchas.refreshAnswerers()
}, { immediate: true })
onBeforeUnmount(() => {
  if (ticker !== null) window.clearInterval(ticker)
})

async function submit(): Promise<void> {
  const captcha = current.value
  if (!captcha) return
  if (isClick.value) {
    const chosen = point.value
    if (!chosen) return
    report(await captchas.click(captcha.id, chosen.x, chosen.y))
    return
  }
  const token = answer.value.trim()
  if (!token) return
  report(await captchas.solve(captcha.id, token))
}

/**
 * The rendered image and its own size, or nothing while the picture has no dimensions yet.
 * Read from the element every time: the same dialog shows one challenge after another.
 */
function imageSize(): { bounds: DOMRect, size: { width: number, height: number } } | null {
  const image = imageRef.value
  if (!image) return null
  const bounds = image.getBoundingClientRect()
  const size = {
    width: image.naturalWidth || bounds.width,
    height: image.naturalHeight || bounds.height
  }
  if (!(bounds.width > 0) || !(bounds.height > 0) || !(size.width > 0) || !(size.height > 0)) return null
  natural.value = size
  return { bounds, size }
}

/**
 * Turns a click on the rendered image into a point in the image's own pixels. Marking is
 * separate from sending: a wrong spot is a wrong answer, which fails the download, so the
 * mark can be moved by clicking again and is only sent by the button. A click the keyboard
 * synthesised (Enter or Space on the button, `detail` 0) carries no position and is handled
 * by `nudge` instead.
 */
function mark(event: MouseEvent): void {
  if (!isClick.value || event.detail === 0) return
  const measured = imageSize()
  if (!measured) return
  const { bounds, size } = measured
  point.value = {
    x: clamp(Math.round(((event.clientX - bounds.left) / bounds.width) * size.width), size.width),
    y: clamp(Math.round(((event.clientY - bounds.top) / bounds.height) * size.height), size.height)
  }
}

/**
 * The keyboard route to the same answer (docs/accessibility.md, "Keyboard operation"): the
 * arrow keys move the mark one pixel, ten with Shift, starting from the centre of the picture
 * when nothing is marked yet; Enter or Space sends it, exactly as the button does. The
 * position is read out from the status line beneath the picture.
 */
function nudge(event: KeyboardEvent): void {
  if (!isClick.value) return
  if (event.key === 'Enter' || event.key === ' ') {
    event.preventDefault()
    if (point.value) void submit()
    return
  }
  const direction: Record<string, [number, number]> = {
    ArrowLeft: [-1, 0],
    ArrowRight: [1, 0],
    ArrowUp: [0, -1],
    ArrowDown: [0, 1]
  }
  const move = direction[event.key]
  if (!move) return
  event.preventDefault()
  const measured = imageSize()
  if (!measured) return
  const { size } = measured
  const step = event.shiftKey ? KEY_STEP_LARGE : KEY_STEP
  const from = point.value ?? { x: Math.round(size.width / 2), y: Math.round(size.height / 2) }
  point.value = {
    x: clamp(from.x + move[0] * step, size.width),
    y: clamp(from.y + move[1] * step, size.height)
  }
}

function clamp(value: number, max: number): number {
  return Math.min(Math.max(value, 0), Math.max(Math.round(max) - 1, 0))
}

async function skip(): Promise<void> {
  const captcha = current.value
  if (!captcha) return
  report(await captchas.skip(captcha.id))
}

function report(outcome: { ok: boolean, message: string }): void {
  toast.add({
    title: outcome.message,
    color: outcome.ok ? 'success' : 'error',
    icon: outcome.ok ? 'i-lucide-circle-check' : 'i-lucide-circle-alert'
  })
}

async function openSettings(): Promise<void> {
  dismissedId.value = current.value?.id ?? null
  await router.push('/settings/captcha')
}

/** Pairing the browser extension lives with the desktop client, next to the capture token. */
async function openExtensionSetup(): Promise<void> {
  dismissedId.value = current.value?.id ?? null
  await router.push('/settings/desktop')
}
</script>

<template>
  <UModal
    :open="open"
    :dismissible="false"
    :close="false"
    :title="t('captcha.title')"
    :description="t('captcha.description', { host: hostLabel })"
    :ui="{ footer: 'justify-end', content: 'sm:max-w-lg' }"
  >
    <template #body>
      <div v-if="current" class="space-y-4">
        <div class="flex items-center justify-between gap-3">
          <UBadge color="neutral" variant="subtle" icon="i-lucide-shield-question">{{ kindLabel }}</UBadge>
          <span v-if="remainingLabel" class="numeric text-xs text-muted">
            {{ t('captcha.expires_in', { time: remainingLabel }) }}
          </span>
        </div>

        <template v-if="isImage">
          <p v-if="current.prompt" class="text-sm leading-6 text-toned">{{ current.prompt }}</p>
          <div v-if="current.image" class="w-full border border-muted bg-elevated p-2">
            <!--
              The mark is positioned against the image itself, not the padded frame. For a
              click-point captcha the picture is a button: the click surface and the keyboard
              surface are the same control, with a name and a focus ring.
            -->
            <component
              :is="isClick ? 'button' : 'div'"
              :type="isClick ? 'button' : undefined"
              :aria-label="isClick ? t('captcha.click.surface') : undefined"
              :aria-describedby="isClick ? 'captcha-click-state' : undefined"
              :class="[
                'relative block w-full',
                isClick && 'cursor-crosshair rounded-none p-0 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary'
              ]"
              @click="mark"
              @keydown="nudge"
            >
              <img
                ref="imageRef"
                :src="current.image"
                :alt="t('captcha.image_alt')"
                class="block w-full"
                draggable="false"
              />
              <span
                v-if="isClick && markerStyle"
                data-testid="click-marker"
                class="pointer-events-none absolute size-4 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white bg-primary shadow"
                :style="markerStyle"
                aria-hidden="true"
              />
            </component>
          </div>
          <form v-if="isClick" id="captcha-answer-form" @submit.prevent="submit">
            <p class="text-sm leading-6 text-toned">{{ t('captcha.click.description') }}</p>
            <p class="text-xs leading-5 text-muted">{{ t('captcha.click.keyboard') }}</p>
            <p
              id="captcha-click-state"
              role="status"
              class="numeric mt-1 text-xs text-muted"
              data-testid="click-state"
            >
              {{ point ? t('captcha.click.marked', { x: point.x, y: point.y }) : t('captcha.click.none') }}
            </p>
          </form>
          <form v-else id="captcha-answer-form" @submit.prevent="submit">
            <UFormField :label="t('captcha.answer_label')" :description="t('captcha.answer_description')">
              <UInput
                v-model="answer"
                autofocus
                maxlength="200"
                autocomplete="off"
                spellcheck="false"
                icon="i-lucide-keyboard"
                class="w-full font-mono"
                :placeholder="t('captcha.answer_placeholder')"
              />
            </UFormField>
          </form>
        </template>

        <template v-else>
          <UAlert
            color="warning"
            variant="subtle"
            icon="i-lucide-shield-alert"
            :title="t('captcha.widget.title', { kind: kindLabel })"
            :description="t('captcha.widget.description', { host: hostLabel, kind: kindLabel })"
          />
          <UAlert
            v-if="extensionConnected"
            color="success"
            variant="subtle"
            icon="i-lucide-puzzle"
            data-testid="extension-connected"
            :title="t('captcha.widget.extension_connected', { host: hostLabel })"
          />
          <div v-else class="space-y-2" data-testid="extension-missing">
            <UAlert
              color="neutral"
              variant="subtle"
              icon="i-lucide-puzzle"
              :title="t('captcha.widget.extension_missing')"
            />
            <UButton
              :label="t('captcha.widget.extension_setup')"
              icon="i-lucide-puzzle"
              color="neutral"
              variant="outline"
              size="sm"
              @click="openExtensionSetup"
            />
          </div>
          <p class="text-xs leading-5 text-muted">{{ t('captcha.widget.hint') }}</p>
        </template>

        <UAlert
          v-if="captchas.error"
          color="error"
          variant="subtle"
          icon="i-lucide-circle-alert"
          :title="captchas.error"
        />

        <p v-if="captchas.pending.length > 1" class="text-xs leading-5 text-muted">
          {{ t('captcha.queued', captchas.pending.length - 1) }}
        </p>
      </div>
    </template>

    <template #footer>
      <UButton
        :label="t('captcha.skip')"
        icon="i-lucide-circle-slash"
        color="neutral"
        variant="outline"
        :loading="captchas.busy"
        :disabled="captchas.busy"
        @click="skip"
      />
      <UButton
        v-if="!isImage"
        :label="t('captcha.widget.configure')"
        icon="i-lucide-sliders-horizontal"
        @click="openSettings"
      />
      <UButton
        v-else
        type="submit"
        form="captcha-answer-form"
        :label="t('captcha.submit')"
        icon="i-lucide-send"
        :disabled="!canSubmit"
        :loading="captchas.busy"
      />
    </template>
  </UModal>
</template>
