import { computed, ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type {
  SiteRule,
  SiteRuleDocument,
  SiteRuleGroup,
  SiteRuleImportPreview,
  SiteRuleImportResult,
  SiteRuleTestResult
} from '@/api/types'
import { useFetchState } from '@/composables/useFetchState'
import type { useCopyName } from '@/composables/useCopyName'
import { copyBody, toBody, type RuleDraft } from '@/utils/siteRuleDraft'

/**
 * The site rules of this installation, and the writes the settings page performs (RD-110-08).
 *
 * The draft the editor draws and its translation to and from the rule body live in
 * `@/utils/siteRuleDraft`; they are re-exported here, so every importer keeps its path.
 */

export {
  STEP_KINDS,
  type StepKind,
  ENCODINGS,
  PACKAGE_SOURCES,
  GROUP_MIRRORS,
  type StepDraft,
  type PackageFields,
  type RuleDraft,
  emptyStep,
  emptyDraft,
  toBody,
  fromRule,
  copyId,
  copyBody,
  draftComplete
} from '@/utils/siteRuleDraft'

interface SiteRulesApi {
  rules: Ref<SiteRule[]>
  groups: Ref<SiteRuleGroup[]>
  loading: Ref<boolean>
  loadError: Ref<string | null>
  pending: Ref<boolean>
  busyId: Ref<string | null>
  error: Ref<string | null>
  /** Groups in order, each with the rules that carry it. */
  byGroup: Ref<{ group: SiteRuleGroup, rules: SiteRule[] }[]>
  refresh: () => Promise<void>
  setRuleEnabled: (rule: SiteRule, enabled: boolean) => Promise<boolean>
  setGroupEnabled: (group: string, enabled: boolean) => Promise<boolean>
  save: (draft: RuleDraft, editingId: string | null) => Promise<boolean>
  remove: (id: string) => Promise<boolean>
  /**
   * Creates a switched-off copy of `rule` and answers with the copy's identifier. The name comes
   * from `useCopyName()`, the one every list with a duplicate uses (RD-150-12).
   */
  duplicate: (rule: SiteRule, copyName: ReturnType<typeof useCopyName>) => Promise<string | null>
  test: (draft: RuleDraft, address: string) => Promise<SiteRuleTestResult | null>
  /** The exchange file (RD-1230-03): the rules named, or every rule when none is. */
  exportRules: (ids?: string[]) => Promise<SiteRuleDocument | null>
  /** What importing `document` would do, rule by rule; stores nothing. */
  previewImport: (document: SiteRuleDocument) => Promise<SiteRuleImportPreview | null>
  /** Imports `document`, replacing exactly the stored rules named in `replace`. */
  importRules: (document: SiteRuleDocument, replace: string[]) => Promise<SiteRuleImportResult | null>
  /** Deletes every rule with its check result; the group switches stay. Answers how many went. */
  clearAll: () => Promise<number | null>
  /** Writes the examples the app brings back, switched off; answers how many. */
  restoreExamples: () => Promise<number | null>
}

export function useSiteRules(): SiteRulesApi {
  const rules = ref<SiteRule[]>([])
  const groups = ref<SiteRuleGroup[]>([])
  const { loading, loadError, load } = useFetchState()
  const pending = ref(false)
  const busyId = ref<string | null>(null)
  const error = ref<string | null>(null)

  const byGroup = computed(() =>
    groups.value.map(group => ({
      group,
      rules: rules.value.filter(rule => rule.group === group.group)
    }))
  )

  async function refresh(): Promise<void> {
    await load(async () => {
      const response = await api.GET('/api/v1/site-rules')
      if (!response.data) return responseError(response)
      rules.value = response.data.rules
      groups.value = response.data.groups
      return null
    })
  }

  /** Runs one write, keeps the list in step, and reports the failure in the reader's language. */
  async function write(id: string | null, run: () => Promise<{ error?: unknown }>): Promise<boolean> {
    busyId.value = id
    pending.value = true
    error.value = null
    const response = await run()
    pending.value = false
    busyId.value = null
    if (response.error !== undefined) {
      error.value = responseError(response)
      return false
    }
    await refresh()
    return true
  }

  return {
    rules,
    groups,
    loading,
    loadError,
    pending,
    busyId,
    error,
    byGroup,
    refresh,
    setRuleEnabled: (rule, enabled) =>
      write(rule.id, () =>
        api.PUT('/api/v1/site-rules/{id}/enabled', {
          params: { path: { id: rule.id } },
          body: { enabled }
        })
      ),
    setGroupEnabled: (group, enabled) =>
      write(`group:${group}`, () =>
        api.PUT('/api/v1/site-rule-groups/{group}/enabled', {
          params: { path: { group } },
          body: { enabled }
        })
      ),
    save: (draft, editingId) =>
      write(editingId, () => {
        const body = { rule: toBody(draft) as never, enabled: draft.enabled }
        return editingId
          ? api.PUT('/api/v1/site-rules/{id}', { params: { path: { id: editingId } }, body })
          : api.POST('/api/v1/site-rules', { body })
      }),
    remove: id => write(id, () => api.DELETE('/api/v1/site-rules/{id}', { params: { path: { id } } })),
    async duplicate(rule, copyName) {
      const body = copyBody(rule, rules.value, copyName)
      // Switched off, like every rule that did not come out of the editor: two rules claiming
      // the same hosts would otherwise both be consulted the moment the copy exists.
      const stored = await write(`copy:${rule.id}`, () =>
        api.POST('/api/v1/site-rules', { body: { rule: body as never, enabled: false } })
      )
      return stored ? String(body.id) : null
    },
    async test(draft, address) {
      pending.value = true
      error.value = null
      const response = await api.POST('/api/v1/site-rules/test', {
        body: { rule: toBody(draft) as never, address }
      })
      pending.value = false
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      return response.data
    },
    async exportRules(ids) {
      error.value = null
      const response = await api.GET('/api/v1/site-rules/export', {
        params: { query: ids?.length ? { ids: ids.join(',') } : {} }
      })
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      return response.data
    },
    async previewImport(document) {
      pending.value = true
      error.value = null
      const response = await api.POST('/api/v1/site-rules/import/preview', { body: document })
      pending.value = false
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      return response.data
    },
    async importRules(document, replace) {
      pending.value = true
      error.value = null
      const response = await api.POST('/api/v1/site-rules/import', { body: { document, replace } })
      pending.value = false
      if (!response.data) {
        error.value = responseError(response)
        return null
      }
      await refresh()
      return response.data
    },
    async clearAll() {
      let removed: number | null = null
      const done = await write('clear', async () => {
        const response = await api.POST('/api/v1/site-rules/clear', { body: { confirmed: true } })
        removed = response.data?.removed ?? null
        return response
      })
      return done ? removed : null
    },
    async restoreExamples() {
      let restored: number | null = null
      const done = await write('examples', async () => {
        const response = await api.POST('/api/v1/site-rules/examples')
        restored = response.data?.restored ?? null
        return response
      })
      return done ? restored : null
    }
  }
}
