import type { BenchmarkNodeRow, BenchmarkSettings } from './types'

/** 按设置中的分隔规则拆分并规范化关键字。 */
function splitKeywords(value: string, caseSensitive: boolean) {
  return value
    .split(/[|,;\n]/)
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => (caseSensitive ? part : part.toLocaleLowerCase()))
}

/** 判断节点是否命中当前包含和排除关键字，排除规则始终优先。 */
export function nodeMatchesBenchmarkFilters(
  node: Pick<BenchmarkNodeRow, 'nodeName' | 'profileName'>,
  settings: Pick<
    BenchmarkSettings,
    'includeKeywords' | 'excludeKeywords' | 'caseSensitive'
  >,
) {
  const normalize = (value: string) =>
    settings.caseSensitive ? value : value.toLocaleLowerCase()
  const searchable = normalize(`${node.profileName} ${node.nodeName}`)
  const includes = splitKeywords(
    settings.includeKeywords,
    settings.caseSensitive,
  )
  const excludes = splitKeywords(
    settings.excludeKeywords,
    settings.caseSensitive,
  )
  return (
    (includes.length === 0 ||
      includes.some((keyword) => searchable.includes(keyword))) &&
    !excludes.some((keyword) => searchable.includes(keyword))
  )
}

/** 判断候选节点所属订阅是否与最近一次读取范围完全一致。 */
export function benchmarkCandidateScopeMatches(
  draft: BenchmarkSettings,
  saved: BenchmarkSettings,
) {
  const normalizeProfiles = (uids: string[]) => [...uids].sort().join('\n')
  return (
    normalizeProfiles(draft.selectedProfileUids) ===
    normalizeProfiles(saved.selectedProfileUids)
  )
}

/** 判断除运行开关外的全部草稿设置是否已经保存。 */
export function benchmarkSettingsMatch(
  draft: BenchmarkSettings,
  saved: BenchmarkSettings,
) {
  const comparable = (settings: BenchmarkSettings) => {
    const {
      enabled: _enabled,
      profileTrafficMultipliers,
      selectedProfileUids,
      ...rest
    } = settings
    return {
      ...rest,
      selectedProfileUids: [...selectedProfileUids].sort(),
      profileTrafficMultipliers: Object.entries(profileTrafficMultipliers).sort(
        ([left], [right]) => left.localeCompare(right),
      ),
    }
  }
  return JSON.stringify(comparable(draft)) === JSON.stringify(comparable(saved))
}

/** 判断订阅从已确认变为待确认时，能否安全自动刷新候选节点。 */
export function shouldAutoRefreshBenchmarkCandidates(
  previousSelectionConfirmed: boolean | null,
  selectionConfirmed: boolean,
  candidateDirty: boolean,
  draft: BenchmarkSettings,
  saved: BenchmarkSettings,
) {
  return (
    previousSelectionConfirmed === true &&
    !selectionConfirmed &&
    !candidateDirty &&
    benchmarkSettingsMatch(draft, saved)
  )
}
