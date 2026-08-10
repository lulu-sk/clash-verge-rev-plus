import { describe, expect, it } from 'vitest'

import {
  benchmarkCandidateScopeMatches,
  benchmarkSettingsMatch,
  nodeMatchesBenchmarkFilters,
  shouldAutoRefreshBenchmarkCandidates,
} from './selection'
import type { BenchmarkSettings } from './types'

const settings: BenchmarkSettings = {
  enabled: false,
  selectedProfileUids: ['profile-a'],
  includeKeywords: '香港|日本',
  excludeKeywords: '倍率',
  caseSensitive: false,
  latencyIntervalMinutes: 15,
  speedIntervalHours: 12,
  latencyUrl: 'https://example.com/ping',
  downloadUrl: 'https://example.com/down',
  uploadUrl: 'https://example.com/up',
  downloadDailyLimitBytes: 1,
  uploadDailyLimitBytes: 1,
  profileTrafficMultipliers: {},
  speedDurationMs: 12_000,
  speedMaxBytes: 8 * 1024 * 1024,
}

describe('节点评选候选范围', () => {
  it('先匹配包含关键字，再让排除关键字优先', () => {
    expect(
      nodeMatchesBenchmarkFilters(
        { nodeName: '香港 01', profileName: '订阅 A' },
        settings,
      ),
    ).toBe(true)
    expect(
      nodeMatchesBenchmarkFilters(
        { nodeName: '香港 2倍率', profileName: '订阅 A' },
        settings,
      ),
    ).toBe(false)
    expect(
      nodeMatchesBenchmarkFilters(
        { nodeName: 'Singapore', profileName: '订阅 A' },
        settings,
      ),
    ).toBe(false)
  })

  it('只有订阅变化才需要重新读取候选节点', () => {
    expect(
      benchmarkCandidateScopeMatches(
        { ...settings, includeKeywords: '新加坡' },
        settings,
      ),
    ).toBe(true)
    expect(
      benchmarkCandidateScopeMatches(
        { ...settings, selectedProfileUids: ['profile-b'] },
        settings,
      ),
    ).toBe(false)
  })

  it('运行开关不影响设置保存判断，其他字段变化会影响', () => {
    expect(
      benchmarkSettingsMatch({ ...settings, enabled: true }, settings),
    ).toBe(true)
    expect(
      benchmarkSettingsMatch(
        { ...settings, latencyIntervalMinutes: 30 },
        settings,
      ),
    ).toBe(false)
  })

  it('订阅倍率对象的字段顺序不同仍视为相同设置', () => {
    const draft = {
      ...settings,
      profileTrafficMultipliers: { beta: 2, alpha: 1 },
    }
    const saved = {
      ...settings,
      profileTrafficMultipliers: { alpha: 1, beta: 2 },
    }
    expect(benchmarkSettingsMatch(draft, saved)).toBe(true)
  })

  it('只在订阅由已确认变为待确认且用户没有编辑时自动刷新候选节点', () => {
    expect(
      shouldAutoRefreshBenchmarkCandidates(
        true,
        false,
        false,
        settings,
        settings,
      ),
    ).toBe(true)
    expect(
      shouldAutoRefreshBenchmarkCandidates(
        false,
        false,
        false,
        settings,
        settings,
      ),
    ).toBe(false)
    expect(
      shouldAutoRefreshBenchmarkCandidates(
        true,
        false,
        true,
        settings,
        settings,
      ),
    ).toBe(false)
    expect(
      shouldAutoRefreshBenchmarkCandidates(
        true,
        false,
        false,
        { ...settings, includeKeywords: '新加坡' },
        settings,
      ),
    ).toBe(false)
  })
})
