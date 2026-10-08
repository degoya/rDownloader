// @vitest-environment node
/**
 * Pick lists of things you create are searchable (RD-1180-02).
 *
 * `design.md` (*Pick lists of things you create*) says a field that picks one of the user's own
 * categories, accounts, proxies, profiles, credentials, targets, indexers, scripts, storage
 * roots or channels is a `SearchableSelect`: a plain select until the list is long, a search
 * field from there on. Two checks hold the rule:
 *
 * - **The converted places** (`CONVERTED`): each still picks through `SearchableSelect` — the list
 *   the job file records, so a place switched back, or its list renamed past the word check
 *   below, fails here by name.
 * - **Every new place**: a `USelect`, or a `USelectMenu` with its search switched off, whose
 *   `:items` names a thing you create (`OWNED`) fails, unless `FIXED` says why that list is fixed
 *   after all (a kind of proxy, an update channel).
 *
 * **What it does not see.** The words are read from the `:items` expression as written, so a list
 * of your own things under a neutral name (`items`, `options`) inside a component of its own is
 * invisible to the second check; such a place goes into `CONVERTED` when it is converted. A
 * select built in a render function is invisible to both.
 */
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it } from 'vitest'

const sourceRoot = dirname(fileURLToPath(import.meta.url))

/** Words in an `:items` expression that name a list the user fills. */
const OWNED = /categor|account|prox|profile|credential|target|indexer|subscription|server|script|root|channel|hotfolder|download/i

/** Lists whose names hold one of those words but whose entries are fixed in the code. */
const FIXED: { file: string, items: string, reason: string }[] = [
  { file: 'components/settings/SettingsNetworkTab.vue', items: 'proxyKindItems', reason: 'the kinds of proxy the engine speaks' },
  { file: 'components/routing/RoutingCategoryRules.vue', items: 'nameTargetItems', reason: 'what a name rule matches against' },
  { file: 'components/settings/SettingsUpdateCard.vue', items: 'channelItems', reason: 'the update channels: stable, beta' }
]

/** Every place converted by RD-1180-02, with a word from its `:items`; one entry per field. */
const CONVERTED: [file: string, items: string][] = [
  ['components/BulkActionBar.vue', 'props.categories'],
  ['components/CollectorPackageGroup.vue', 'categoryItems'],
  ['components/DirectAddForm.vue', 'categoryItems'],
  ['components/DirectAddForm.vue', 'accountItems'],
  ['components/DirectAddForm.vue', 'proxyItems'],
  ['components/IndexerSearchPanel.vue', 'indexerItems'],
  ['components/MediaCookieProfileField.vue', 'profileItems'],
  ['components/NzbImportGroup.vue', 'categoryItems'],
  ['components/NzbImportModal.vue', 'categoryItems'],
  ['components/PackageEditModal.vue', 'scriptItems'],
  ['components/PackageGroup.vue', 'categoryItems'],
  ['components/SettingsTorrentCard.vue', 'proxyItems'],
  ['components/SubscriptionForm.vue', 'scriptItems'],
  ['components/SubscriptionForm.vue', 'categoryItems'],
  ['components/SubscriptionIndexerCategories.vue', 'mappableCategories'],
  ['components/SubscriptionIndexerCategories.vue', 'props.categories'],
  ['components/SubscriptionIndexerSearch.vue', 'indexerItems'],
  ['components/TorrentMoveModal.vue', 'rootItems'],
  ['components/TransferCard.vue', 'authProfileItems'],
  ['components/bandwidth/BandwidthSchedule.vue', 'profileItems'],
  ['components/bandwidth/BandwidthSchedule.vue', 'profileItems'],
  ['components/bandwidth/BandwidthStatusCard.vue', 'profileItems'],
  ['components/notifications/NotificationRules.vue', 'targetItems'],
  ['components/notifications/NotificationRules.vue', 'categoryItems'],
  ['components/routing/RoutingCategories.vue', 'rootItems'],
  ['components/routing/RoutingCategories.vue', 'scriptItems'],
  ['components/routing/RoutingCategoryRules.vue', 'categoryItems'],
  ['components/routing/RoutingHotfolders.vue', 'categoryItems'],
  ['components/settings/RemoteJobSubmitForm.vue', 'accountItems'],
  ['components/settings/SettingsAccountsCard.vue', 'proxyItems'],
  ['components/settings/SettingsBackupDestinations.vue', 'profileItems'],
  ['components/settings/SettingsNetworkTab.vue', 'proxyItems'],
  ['components/settings/UsenetServerChain.vue', 'proxyItems'],
  ['components/storage/PackageStorageModal.vue', 'downloadItems'],
  ['views/AutomationView.vue', 'scriptItems'],
  ['views/StreamsView.vue', 'categoryItems'],
  ['views/StreamsView.vue', 'channelItems']
]

interface Select {
  file: string
  line: number
  name: string
  items: string
  /** A `USelectMenu` with `:search-input="false"`. */
  searchOff: boolean
}

const SELECT = /<(USelect|USelectMenu|SearchableSelect)\b((?:\s+[^\s"'=<>/]+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s>]+))?)*)\s*\/?>/g

/** Every select of a component's template; comments are blanked, lines kept. */
function selects(file: string, source: string): Select[] {
  const template = source.replace(/<!--[\s\S]*?-->/g, comment => comment.replace(/[^\n]/g, ' '))
  return [...template.matchAll(SELECT)].map(match => ({
    file,
    line: template.slice(0, match.index).split('\n').length,
    name: match[1] ?? '',
    items: /:items="([^"]*)"/.exec(match[2] ?? '')?.[1] ?? '',
    searchOff: /:search-input="false"/.test(match[2] ?? '')
  }))
}

function components(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name)
    if (entry.isDirectory()) {
      return components(path)
    }
    return entry.name.endsWith('.vue') ? [path] : []
  })
}

/** A select of the user's own things that cannot search, unless `FIXED` names its list. */
function unsearchable(found: Select[], fixed = FIXED): Select[] {
  return found.filter(select =>
    (select.name === 'USelect' || (select.name === 'USelectMenu' && select.searchOff))
    && OWNED.test(select.items)
    && !fixed.some(entry => entry.file === select.file && select.items.includes(entry.items)))
}

const sourceSelects = components(sourceRoot).flatMap(path =>
  selects(relative(sourceRoot, path), readFileSync(path, 'utf8')))

describe('pick lists of things you create', () => {
  it('search, wherever the list names something you create', () => {
    const where = unsearchable(sourceSelects).map(select => `${select.file}:${select.line} <${select.name} :items="${select.items}">`)
    expect(where, 'Use SearchableSelect (design.md, "Pick lists of things you create"), or name a fixed list in FIXED').toEqual([])
  })

  it('search at every place RD-1180-02 converted', () => {
    const missing = [...new Set(CONVERTED.map(entry => entry.join('\0')))].flatMap((key) => {
      const [file, items] = key.split('\0') as [string, string]
      const wanted = CONVERTED.filter(entry => entry[0] === file && entry[1] === items).length
      const found = sourceSelects.filter(select => select.file === file && select.name === 'SearchableSelect' && select.items.includes(items)).length
      return found < wanted ? [`${file}: ${wanted} SearchableSelect over ${items}, found ${found}`] : []
    })
    expect(missing).toEqual([])
  })

  it('names in FIXED only lists that are still there', () => {
    const stale = FIXED.filter(entry => !sourceSelects.some(select => select.file === entry.file && select.items.includes(entry.items)))
    expect(stale.map(entry => `${entry.file}: ${entry.items}`)).toEqual([])
  })

  describe('the guard itself', () => {
    const fixture = (template: string) => selects('Fixture.vue', `<template>\n${template}\n</template>\n`)

    it('turns red on a plain select of your own things, naming file and line', () => {
      const found = unsearchable(fixture('  <USelect v-model="category" :items="categoryItems" />\n  <USelect\n    v-model="account"\n    :items="accounts.map(account => ({ label: account.name, value: account.id }))"\n  />'))
      expect(found.map(select => select.line)).toEqual([2, 3])
    })

    it('turns red on a select menu whose search is switched off', () => {
      const found = unsearchable(fixture('  <USelectMenu v-model="proxy" :items="proxyItems" :search-input="false" />\n  <USelectMenu v-model="proxy" :items="proxyItems" />'))
      expect(found.map(select => select.line)).toEqual([2])
    })

    it('leaves fixed lists, searchable ones and listed exceptions alone', () => {
      const found = unsearchable(
        fixture('  <USelect v-model="priority" :items="PRIORITY_ITEMS" />\n  <SearchableSelect v-model="category" :items="categoryItems" />\n  <USelect v-model="kind" :items="proxyKindItems" />'),
        [{ file: 'Fixture.vue', items: 'proxyKindItems', reason: 'fixed' }]
      )
      expect(found).toEqual([])
    })

    it('ignores a select that is only mentioned in a comment', () => {
      expect(unsearchable(fixture('  <!-- was <USelect :items="categoryItems" /> -->'))).toEqual([])
    })
  })
})
