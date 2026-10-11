<script setup lang="ts">
/**
 * "Install updates automatically" under Settings > System > Updates (RD-1240-27): the switch,
 * off by default, and the optional time window an automatic install may start in. Both are
 * fields of the settings document and are saved with it; the service decides the moment (nothing
 * running for five minutes, inside the window). Where the installation does not install itself —
 * a package manager, a container — the switch is greyed out and says why.
 *
 * Beside it "Restart automatically when needed" (RD-1240-32): a plugin change waits for the next
 * start, and this restarts once nothing has run for five minutes. It works for every installation
 * kind, so it is never locked, and it shares the time window, which shows while either is on.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { RestartSettings } from '@/api/restart'
import type { Settings } from '@/api/types'
import type { UpdateStatus } from '@/api/updates'
import { clockOf, timeFieldValue, type TimeLike } from '@/utils/timeFields'
import { minutesOf, timeOf } from '@/utils/weekWindows'

const settings = defineModel<Settings>({ required: true })
const props = defineProps<{ status: UpdateStatus | null }>()
const { t } = useI18n()

/** The window a switched-on window starts with: the small hours. */
const NIGHT = { start_minute: 3 * 60, end_minute: 6 * 60 }

/** Locked once the status says this installation does not install itself; open until then. */
const locked = computed(() => props.status !== null && props.status.installs_itself === false)

/** A document from before the setting has none: off. */
const enabled = computed({
  get: () => settings.value.update_auto_install ?? false,
  set: (on: boolean) => { settings.value.update_auto_install = on }
})

/** Until the schema is regenerated the document's type does not name the switch yet. */
const restartWhenNeeded = computed({
  get: () => (settings.value as RestartSettings).restart_when_needed ?? false,
  set: (on: boolean) => { (settings.value as RestartSettings).restart_when_needed = on }
})

/** The window bounds the automatic install and the automatic restart alike. */
const windowShown = computed(() => (enabled.value && !locked.value) || restartWhenNeeded.value)

const window = computed(() => settings.value.update_auto_install_window ?? null)

const windowed = computed({
  get: () => window.value !== null,
  set: (on: boolean) => { settings.value.update_auto_install_window = on ? { ...NIGHT } : null }
})

/** An empty window would never install; the service refuses it, said here before the save. */
const windowEmpty = computed(() => window.value !== null && window.value.start_minute === window.value.end_minute)

/** A field emptied while it is typed in keeps the stored time; a window has no empty end. */
function setTime(key: 'start_minute' | 'end_minute', value: TimeLike): void {
  const clock = clockOf(value)
  if (!clock || window.value === null) return
  settings.value.update_auto_install_window = { ...window.value, [key]: minutesOf(clock) % 1440 }
}
</script>

<template>
  <div class="grid gap-4" data-testid="update-auto-install">
    <UFormField :label="t('system.updates.auto_install.label')" :description="t('system.updates.auto_install.description')" orientation="horizontal">
      <USwitch v-model="enabled" :disabled="locked" data-testid="update-auto-install-switch" />
    </UFormField>
    <p v-if="locked && status" class="text-xs text-muted" data-testid="update-auto-install-locked">
      {{ t('system.updates.auto_install.locked', { kind: t(`system.updates.kind.${status.install_kind}`) }) }}
    </p>
    <UFormField :label="t('system.updates.auto_restart.label')" :description="t('system.updates.auto_restart.description')" orientation="horizontal">
      <USwitch v-model="restartWhenNeeded" data-testid="update-auto-restart-switch" />
    </UFormField>
    <template v-if="windowShown">
      <UFormField :label="t('system.updates.auto_install.window_label')" :description="t('system.updates.auto_install.window_description')" orientation="horizontal">
        <USwitch v-model="windowed" data-testid="update-auto-install-window" />
      </UFormField>
      <div v-if="window" class="flex flex-wrap items-end gap-2">
        <UFormField :label="t('system.updates.auto_install.from')">
          <UInputTime :model-value="timeFieldValue(timeOf(window.start_minute))" class="w-28" data-testid="update-auto-install-from" @update:model-value="setTime('start_minute', $event)" />
        </UFormField>
        <UFormField :label="t('system.updates.auto_install.until')">
          <UInputTime :model-value="timeFieldValue(timeOf(window.end_minute))" class="w-28" data-testid="update-auto-install-until" @update:model-value="setTime('end_minute', $event)" />
        </UFormField>
      </div>
      <p v-if="windowEmpty" class="text-xs text-error" data-testid="update-auto-install-window-empty">{{ t('system.updates.auto_install.window_empty') }}</p>
    </template>
  </div>
</template>
