<script setup lang="ts">
import { computed, onMounted, onScopeDispose, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PostprocessLevel } from '@/api/types'
import { INHERIT_LEVEL, postprocessLevelItems } from '@/utils/format'

const props = defineProps<{
  name: string
  hasPassword: boolean
  password: string | null
  postprocessLevel: PostprocessLevel | null
  script: string | null
  scripts: string[]
  /** Offers "rename the folder too"; only the download list can move data (RD-106-13). */
  canRenameFolder?: boolean
  /**
   * The package's own speed limit in MiB/s, null for none (RD-1100-01). Left out, the field is
   * not offered: the LinkGrabber's packages have no transfers to limit yet.
   */
  speedLimitMiB?: number | null
  /** False while the package holds a torrent, which the engine cannot limit on its own. */
  speedLimitSupported?: boolean
}>()
const emit = defineEmits<{
  close: [result: { name: string, password: string | null, clearPassword: boolean, postprocessLevel: PostprocessLevel | null, script: string | null, renameFolder: boolean, speedLimitMiB?: number | null } | null]
}>()
const { t } = useI18n()
const name = ref(props.name)
const password = ref(props.password ?? '')
const clearPassword = ref(false)
const renameFolder = ref(false)
const level = ref<string>(props.postprocessLevel ?? INHERIT_LEVEL)
const script = ref<string>(props.script ?? INHERIT_LEVEL)
const speedLimitOffered = props.speedLimitMiB !== undefined
const speedLimit = ref<number | string | null>(props.speedLimitMiB ?? null)

/** The entered limit in MiB/s; empty, zero or not a number is no limit of its own. */
function enteredSpeedLimit(): number | null {
  if (speedLimit.value === null || speedLimit.value === '') return null
  const value = Number(speedLimit.value)
  return Number.isFinite(value) && value > 0 ? value : null
}

/**
 * The name field, focused with its whole text selected when the dialog opens (RD-130-13), so a
 * name pasted from elsewhere replaces the old one instead of landing beside it. `autofocus` only
 * focused it, with the caret at the end. A timeout rather than `nextTick`: the dialog's focus
 * trap places its own focus a microtask after mounting, and this has to come after it — the
 * same moment `UInput`'s `autofocus` used.
 */
const nameField = ref<{ inputRef?: HTMLInputElement | null } | null>(null)
let focusTimer: ReturnType<typeof setTimeout> | undefined
onMounted(() => {
  focusTimer = setTimeout(() => {
    const input = nameField.value?.inputRef
    input?.focus()
    input?.select()
  })
})
onScopeDispose(() => clearTimeout(focusTimer))

const levelItems = computed(() => postprocessLevelItems())
const scriptItems = computed(() => [
  { label: t('downloads.edit_package.script_inherit'), value: INHERIT_LEVEL },
  ...(props.script && !props.scripts.includes(props.script) ? [{ label: props.script, value: props.script }] : []),
  ...props.scripts.map(item => ({ label: item, value: item }))
])

function submit(): void {
  const trimmed = name.value.trim()
  if (!trimmed) return
  emit('close', {
    name: trimmed,
    password: password.value.trim() || null,
    clearPassword: clearPassword.value,
    postprocessLevel: level.value === INHERIT_LEVEL ? null : level.value as PostprocessLevel,
    script: script.value === INHERIT_LEVEL ? null : script.value,
    renameFolder: renameFolder.value && trimmed !== props.name,
    ...(speedLimitOffered && props.speedLimitSupported ? { speedLimitMiB: enteredSpeedLimit() } : {})
  })
}
</script>

<template>
  <UModal :title="t('downloads.edit_package.title')" :description="t('downloads.edit_package.description')" :close="{ onClick: () => emit('close', null) }" :ui="{ footer: 'justify-end' }">
    <template #body>
      <form id="package-edit-form" class="space-y-3" @submit.prevent="submit">
        <UFormField :label="t('downloads.edit_package.name')">
          <UInput ref="nameField" v-model="name" maxlength="200" class="w-full" />
        </UFormField>
        <USwitch
          v-if="props.canRenameFolder"
          v-model="renameFolder"
          size="sm"
          :disabled="name.trim() === props.name"
          :label="t('downloads.edit_package.rename_folder')"
          :description="renameFolder ? t('downloads.edit_package.rename_folder_hint') : undefined"
        />
        <UFormField :label="t('downloads.edit_package.password')" :description="props.hasPassword ? t('downloads.edit_package.password_stored') : t('downloads.edit_package.password_optional')">
          <UInput v-model="password" maxlength="1024" class="w-full font-mono" :placeholder="t('downloads.edit_package.password_placeholder')" />
        </UFormField>
        <USwitch v-if="props.hasPassword" v-model="clearPassword" size="sm" :label="t('downloads.edit_package.clear_password')" />
        <UFormField :label="t('downloads.edit_package.postprocess_level')" :description="t('downloads.edit_package.postprocess_level_hint')">
          <USelect v-model="level" :items="levelItems" value-key="value" class="w-full" />
        </UFormField>
        <UFormField :label="t('downloads.edit_package.script')" :description="props.scripts.length ? t('downloads.edit_package.script_hint') : t('downloads.edit_package.script_empty')">
          <USelect v-model="script" :items="scriptItems" value-key="value" class="w-full font-mono" />
        </UFormField>
        <UFormField
          v-if="speedLimitOffered"
          :label="t('downloads.edit_package.speed_limit')"
          :description="props.speedLimitSupported ? t('downloads.edit_package.speed_limit_hint') : t('downloads.edit_package.speed_limit_unsupported')"
        >
          <UInput
            v-model.number="speedLimit"
            type="number"
            min="0"
            step="0.5"
            class="w-full"
            data-testid="package-speed-limit"
            :disabled="!props.speedLimitSupported"
            :placeholder="t('downloads.toolbar.speed_limit_placeholder')"
            :aria-label="t('downloads.edit_package.speed_limit_aria')"
            :ui="{ base: 'pe-12 font-mono', trailing: 'pointer-events-none pe-2' }"
          >
            <template #trailing><span class="font-mono text-[10px] text-muted">MiB/s</span></template>
          </UInput>
        </UFormField>
      </form>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="emit('close', null)" />
      <UButton :label="t('common.actions.save')" icon="i-lucide-save" type="submit" form="package-edit-form" :disabled="!name.trim()" />
    </template>
  </UModal>
</template>
