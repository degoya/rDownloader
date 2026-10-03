<script setup lang="ts">
/**
 * Which release files a git-release subscription downloads (RD-190-13), beside the repository
 * address it polls.
 *
 * Platform and architecture are ready-made patterns the server matches against the file name;
 * the name patterns are for everything those do not say. A file that names no architecture —
 * a universal macOS image, an installer — passes the architecture choice, because that is what
 * such a file is for. Drafts are never downloaded, and that is not a choice.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { ARCHITECTURES, FORGE_FROM_ADDRESS, PLATFORM_LABELS, PLATFORMS, type GitReleaseFields } from '@/utils/gitRelease'

const fields = defineModel<GitReleaseFields>('fields', { required: true })

const { t } = useI18n()

const forgeItems = computed(() => [
  { value: FORGE_FROM_ADDRESS, label: t('subscriptions.form.git_forges.auto') },
  { value: 'github', label: t('subscriptions.form.git_forges.github') },
  { value: 'gitlab', label: t('subscriptions.form.git_forges.gitlab') }
])

const platformItems = PLATFORMS.map(platform => ({ value: platform, label: PLATFORM_LABELS[platform] }))
// Spelled as the file names spell them; there is nothing to translate.
const architectureItems = ARCHITECTURES.map(architecture => ({ value: architecture, label: architecture }))
</script>

<template>
  <div class="grid gap-3" data-testid="subscription-git-release">
    <UFormField :label="t('subscriptions.form.git_forge')" :description="t('subscriptions.form.git_forge_description')">
      <USelect v-model="fields.forge" class="w-full" :items="forgeItems" value-key="value" data-testid="subscription-git-forge" />
    </UFormField>
    <UFormField :label="t('subscriptions.form.git_platforms')" :description="t('subscriptions.form.git_platforms_description')">
      <USelectMenu
        v-model="fields.platforms"
        multiple
        class="w-full"
        value-key="value"
        :items="platformItems"
        :placeholder="t('subscriptions.form.git_all')"
        data-testid="subscription-git-platforms"
      />
    </UFormField>
    <UFormField :label="t('subscriptions.form.git_architectures')" :description="t('subscriptions.form.git_architectures_description')">
      <USelectMenu
        v-model="fields.architectures"
        multiple
        class="w-full font-mono"
        value-key="value"
        :items="architectureItems"
        :placeholder="t('subscriptions.form.git_all')"
        data-testid="subscription-git-architectures"
      />
    </UFormField>
    <UFormField :label="t('subscriptions.form.git_asset_patterns')" :description="t('subscriptions.form.git_asset_patterns_description')">
      <UInput
        v-model="fields.patterns"
        class="w-full font-mono"
        autocomplete="off"
        spellcheck="false"
        placeholder="*.AppImage"
        data-testid="subscription-git-patterns"
      />
    </UFormField>
    <USwitch v-model="fields.prereleases" :label="t('subscriptions.form.git_prereleases')" data-testid="subscription-git-prereleases" />
    <USwitch v-model="fields.sourceArchives" :label="t('subscriptions.form.git_source_archives')" data-testid="subscription-git-source-archives" />
  </div>
</template>
