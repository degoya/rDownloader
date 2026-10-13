import { useI18n } from 'vue-i18n'

import type { SiteRule } from '@/api/types'
import { bundledDescriptionKey, originView } from '@/utils/siteRuleOrigin'

/** How a site rule reads in the list of the page-recognition tab, in the reader's language. */
export function useSiteRuleLabels() {
  const { t, te } = useI18n()

  /** The group's own name when a catalogue has one, and the raw word otherwise: groups come
   *  from rule bodies and a rule somebody wrote may carry any word at all. */
  function groupLabel(group: string): string {
    const key = `siterules.groups.${group}`
    return te(key) ? t(key) : group
  }

  /** A bundled rule's description in the reader's language, any other rule's own (RD-1240-33). */
  function descriptionOf(rule: SiteRule): string | null | undefined {
    const key = bundledDescriptionKey(rule)
    return key && te(key) ? t(key) : rule.description
  }

  /**
   * What the last self-test found here, or nothing: a rule no self-test has checked carries no
   * badge (RD-1230-03). The `checked` day a rule body carries is what its author wrote, so it is
   * not read as a measurement of this installation's reach.
   */
  function stateColor(rule: SiteRule): 'success' | 'warning' | 'error' {
    switch (rule.check?.verdict) {
      case 'ok': return 'success'
      case 'structural': return 'warning'
      default: return 'error'
    }
  }

  /** The sentence behind the badge: the four-way verdict, plus the refusal's own reason. */
  function stateTitle(rule: SiteRule): string {
    if (!rule.check) return ''
    const verdict = t(`server.codes.site_rules.state.${rule.check.verdict}`)
    return rule.check.code ? `${verdict} — ${t(`server.codes.${rule.check.code}`)}` : verdict
  }

  /** Where the rule came from, as the glyph, its word and the sentence behind it (RD-1200-05). */
  function originOf(rule: SiteRule): { icon: string, color: 'success' | 'neutral', label: string, detail: string } {
    const view = originView(rule.origin)
    return { icon: view.icon, color: view.color, label: t(view.label), detail: t(view.detail) }
  }

  return { groupLabel, descriptionOf, stateColor, stateTitle, originOf }
}
