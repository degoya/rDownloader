<script setup lang="ts">
/**
 * The package-name regex rules (RD-1140-05): find → replace pairs that run after the switches, in
 * the order shown. Each pair is edited in the regex editor's replacement mode, which tries it on
 * sample names with the service's own engine; the list adds, removes and reorders.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PackageNameRegex } from '@/api/types'
import { usePackageNameRegexEditor } from '@/composables/usePackageNameRegexEditor'
import { MAX_PACKAGE_NAME_REGEX_RULES } from '@/utils/packageNameRules'

const pairs = defineModel<PackageNameRegex[]>({ required: true })
const { t } = useI18n()
const editPair = usePackageNameRegexEditor()

const full = computed(() => pairs.value.length >= MAX_PACKAGE_NAME_REGEX_RULES)

async function add(): Promise<void> {
  const added = await editPair(null)
  if (added) pairs.value = [...pairs.value, added]
}

async function edit(index: number): Promise<void> {
  const edited = await editPair(pairs.value[index] ?? null)
  if (edited) pairs.value = pairs.value.map((pair, at) => (at === index ? edited : pair))
}

function remove(index: number): void {
  pairs.value = pairs.value.filter((_, at) => at !== index)
}

function move(index: number, delta: -1 | 1): void {
  const target = index + delta
  if (target < 0 || target >= pairs.value.length) return
  const next = [...pairs.value]
  const [moved] = next.splice(index, 1)
  if (moved) next.splice(target, 0, moved)
  pairs.value = next
}
</script>

<template>
  <div class="space-y-2" data-testid="package-name-regex">
    <p v-if="!pairs.length" class="text-xs text-muted">{{ t('settings.postprocess.package_names.regex.empty') }}</p>
    <ol v-else class="space-y-1">
      <li v-for="(pair, index) in pairs" :key="index" class="flex items-center gap-2 border border-muted px-2 py-1" data-testid="package-name-regex-row">
        <span class="numeric w-5 shrink-0 text-xs text-muted">{{ index + 1 }}.</span>
        <span class="min-w-0 flex-1 truncate font-mono text-xs">
          <span class="text-highlighted">{{ pair.pattern }}</span>
          <span class="text-muted"> → </span>
          <span :class="pair.replacement ? 'text-highlighted' : 'text-muted'">{{ pair.replacement || t('settings.postprocess.package_names.regex.replacement_empty') }}</span>
        </span>
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-arrow-up" :disabled="index === 0" :aria-label="t('settings.postprocess.package_names.regex.move_up')" @click="move(index, -1)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-arrow-down" :disabled="index === pairs.length - 1" :aria-label="t('settings.postprocess.package_names.regex.move_down')" @click="move(index, 1)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('settings.postprocess.package_names.regex.edit')" @click="edit(index)" />
        <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('settings.postprocess.package_names.regex.remove')" @click="remove(index)" />
      </li>
    </ol>
    <UButton size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('settings.postprocess.package_names.regex.add')" :disabled="full" data-testid="package-name-regex-add" @click="add" />
  </div>
</template>
