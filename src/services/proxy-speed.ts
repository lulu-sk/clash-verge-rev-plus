import { cmdTestDownloadSpeed } from '@/services/cmds'

const SOURCE_STORAGE_KEY = 'proxy-speed-test-source-id'
const PRESET_STORAGE_KEY = 'proxy-speed-test-preset-id'
const SORT_STORAGE_KEY = 'proxy-speed-test-sort-id'
const CUSTOM_URL_STORAGE_KEY = 'proxy-speed-test-custom-url'
const RESULT_CACHE_STORAGE_KEY = 'proxy-speed-test-result-cache'
export const PROXY_SPEED_TEST_CACHE_CHANGE_EVENT =
  'proxy-speed-test-cache-change'
const DEFAULT_CONNECT_TIMEOUT_MS = 8000
const DEFAULT_READ_IDLE_TIMEOUT_MS = 3000
const MAX_RESULT_CACHE_ENTRIES = 20

export interface ProxySpeedTestSource {
  id: string
  url: string
}

export interface ProxySpeedTestPreset {
  id: string
  durationMs: number
  maxBytes: number
}

export interface ProxySpeedTestStoredRow {
  name: string
  type: string
  status: 'idle' | 'testing' | 'success' | 'failed'
  error?: string
  result?: IProxyDownloadSpeedTestResult
}

export type ProxySpeedTestSortId =
  | 'default'
  | 'speedDesc'
  | 'speedAsc'
  | 'trafficDesc'
  | 'durationAsc'
  | 'nameAsc'

interface ProxySpeedTestResultCacheEntry {
  key: string
  groupName: string
  sourceId: string
  presetId: string
  url: string
  rows: ProxySpeedTestStoredRow[]
  updatedAt: number
}

export const PROXY_SPEED_TEST_SOURCES: ProxySpeedTestSource[] = [
  {
    id: 'cloudflare',
    url: 'https://speed.cloudflare.com/__down?bytes=50000000',
  },
  {
    id: 'bunny',
    url: 'https://speedtest.bunnycdn.com/100mb.bin',
  },
  {
    id: 'hetzner',
    url: 'https://speed.hetzner.de/100MB.bin',
  },
  {
    id: 'custom',
    url: '',
  },
]

export const PROXY_SPEED_TEST_PRESETS: ProxySpeedTestPreset[] = [
  {
    id: 'quick',
    durationMs: 3000,
    maxBytes: 8 * 1024 * 1024,
  },
  {
    id: 'balanced',
    durationMs: 5000,
    maxBytes: 32 * 1024 * 1024,
  },
  {
    id: 'extended',
    durationMs: 10000,
    maxBytes: 64 * 1024 * 1024,
  },
]

/**
 * 获取可用的本地存储实例。
 */
function getStorage() {
  if (typeof window === 'undefined') return null
  return window.localStorage
}

/**
 * 构造测速结果缓存键。
 */
function getProxySpeedTestCacheKey(
  groupName: string,
  sourceId: string,
  presetId: string,
  url: string,
) {
  return `${groupName}::${sourceId}::${presetId}::${url}`
}

/**
 * 读取测速结果缓存表。
 */
function getProxySpeedTestResultCacheRecord() {
  const storage = getStorage()
  const rawValue = storage?.getItem(RESULT_CACHE_STORAGE_KEY)
  if (!rawValue) return {}

  try {
    return JSON.parse(rawValue) as Record<
      string,
      ProxySpeedTestResultCacheEntry
    >
  } catch {
    return {}
  }
}

/**
 * 保存测速结果缓存表。
 */
function setProxySpeedTestResultCacheRecord(
  cacheRecord: Record<string, ProxySpeedTestResultCacheEntry>,
) {
  const storage = getStorage()
  if (!storage) return

  const limitedEntries = Object.values(cacheRecord)
    .sort((left, right) => right.updatedAt - left.updatedAt)
    .slice(0, MAX_RESULT_CACHE_ENTRIES)
  const limitedRecord = Object.fromEntries(
    limitedEntries.map((entry) => [entry.key, entry]),
  )

  storage.setItem(RESULT_CACHE_STORAGE_KEY, JSON.stringify(limitedRecord))
}

/**
 * 读取上次选择的测速源标识。
 */
export function getStoredProxySpeedTestSourceId() {
  const storage = getStorage()
  return storage?.getItem(SOURCE_STORAGE_KEY) || PROXY_SPEED_TEST_SOURCES[0].id
}

/**
 * 读取上次选择的测速档位标识。
 */
export function getStoredProxySpeedTestPresetId() {
  const storage = getStorage()
  return storage?.getItem(PRESET_STORAGE_KEY) || 'balanced'
}

/**
 * 读取上次选择的测速结果排序方式。
 */
export function getStoredProxySpeedTestSortId(): ProxySpeedTestSortId {
  const storage = getStorage()
  const sortId = storage?.getItem(SORT_STORAGE_KEY)

  switch (sortId) {
    case 'speedDesc':
    case 'speedAsc':
    case 'trafficDesc':
    case 'durationAsc':
    case 'nameAsc':
      return sortId
    default:
      return 'default'
  }
}

/**
 * 保存当前测速源标识。
 */
export function setStoredProxySpeedTestSourceId(sourceId: string) {
  const storage = getStorage()
  storage?.setItem(SOURCE_STORAGE_KEY, sourceId)
}

/**
 * 保存当前测速档位标识。
 */
export function setStoredProxySpeedTestPresetId(presetId: string) {
  const storage = getStorage()
  storage?.setItem(PRESET_STORAGE_KEY, presetId)
}

/**
 * 保存当前测速结果排序方式。
 */
export function setStoredProxySpeedTestSortId(sortId: ProxySpeedTestSortId) {
  const storage = getStorage()
  storage?.setItem(SORT_STORAGE_KEY, sortId)
}

/**
 * 读取上次输入的自定义测速链接。
 */
export function getStoredProxySpeedTestCustomUrl() {
  const storage = getStorage()
  return storage?.getItem(CUSTOM_URL_STORAGE_KEY) || ''
}

/**
 * 保存自定义测速链接。
 */
export function setStoredProxySpeedTestCustomUrl(url: string) {
  const storage = getStorage()
  storage?.setItem(CUSTOM_URL_STORAGE_KEY, url)
}

/**
 * 将测速源选择解析为最终的测速链接。
 */
export function resolveProxySpeedTestUrl(sourceId: string, customUrl: string) {
  if (sourceId === 'custom') {
    return customUrl.trim()
  }

  return (
    PROXY_SPEED_TEST_SOURCES.find((source) => source.id === sourceId)?.url || ''
  )
}

/**
 * 读取指定测速档位的配置。
 */
export function getProxySpeedTestPreset(presetId: string) {
  return (
    PROXY_SPEED_TEST_PRESETS.find((preset) => preset.id === presetId) ||
    PROXY_SPEED_TEST_PRESETS.find((preset) => preset.id === 'balanced') ||
    PROXY_SPEED_TEST_PRESETS[0]
  )
}

/**
 * 读取指定条件下缓存的测速结果。
 */
export function getStoredProxySpeedTestCachedRows(
  groupName: string,
  sourceId: string,
  presetId: string,
  url: string,
) {
  if (!groupName || !url) return []

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const cacheKey = getProxySpeedTestCacheKey(groupName, sourceId, presetId, url)
  return cacheRecord[cacheKey]?.rows || []
}

/**
 * 保存指定条件下的测速结果缓存。
 */
export function setStoredProxySpeedTestCachedRows(
  groupName: string,
  sourceId: string,
  presetId: string,
  url: string,
  rows: ProxySpeedTestStoredRow[],
) {
  if (!groupName || !url || rows.length === 0) return

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const cacheKey = getProxySpeedTestCacheKey(groupName, sourceId, presetId, url)

  cacheRecord[cacheKey] = {
    key: cacheKey,
    groupName,
    sourceId,
    presetId,
    url,
    rows,
    updatedAt: Date.now(),
  }

  setProxySpeedTestResultCacheRecord(cacheRecord)

  window.dispatchEvent(new Event(PROXY_SPEED_TEST_CACHE_CHANGE_EVENT))
}

/**
 * 读取指定代理组每个节点最近一次成功的下载测速结果。
 */
export function getLatestProxySpeedTestResultMap(groupName: string) {
  const resultMap = new Map<string, IProxyDownloadSpeedTestResult>()
  if (!groupName) return resultMap

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const entries = Object.values(cacheRecord)
    .filter((entry) => entry.groupName === groupName)
    .sort((left, right) => right.updatedAt - left.updatedAt)

  for (const entry of entries) {
    for (const row of entry.rows) {
      if (resultMap.has(row.name)) continue
      if (row.status !== 'success' || !row.result) continue

      resultMap.set(row.name, row.result)
    }
  }

  return resultMap
}

/**
 * 构造后端测速命令的参数。
 */
export function getProxySpeedTestOptions(
  url: string,
  presetId: string,
): IProxyDownloadSpeedTestOptions {
  const preset = getProxySpeedTestPreset(presetId)
  return {
    url,
    durationMs: preset.durationMs,
    maxBytes: preset.maxBytes,
    connectTimeoutMs: DEFAULT_CONNECT_TIMEOUT_MS,
    readIdleTimeoutMs: DEFAULT_READ_IDLE_TIMEOUT_MS,
  }
}

/**
 * 将下载字节数格式化为更易读的文本。
 */
export function formatProxySpeedTestBytes(bytes: number) {
  if (bytes >= 1024 ** 3) {
    return `${(bytes / 1024 ** 3).toFixed(2)} GB`
  }
  if (bytes >= 1024 ** 2) {
    return `${(bytes / 1024 ** 2).toFixed(2)} MB`
  }
  if (bytes >= 1024) {
    return `${(bytes / 1024).toFixed(2)} KB`
  }
  return `${bytes} B`
}

/**
 * 将平均下载速度格式化为更易读的文本。
 */
export function formatProxySpeedTestSpeed(bytesPerSecond: number) {
  if (bytesPerSecond >= 1024 ** 3) {
    return `${(bytesPerSecond / 1024 ** 3).toFixed(2)} GB/s`
  }
  if (bytesPerSecond >= 1024 ** 2) {
    return `${(bytesPerSecond / 1024 ** 2).toFixed(2)} MB/s`
  }
  if (bytesPerSecond >= 1024) {
    return `${(bytesPerSecond / 1024).toFixed(2)} KB/s`
  }
  return `${bytesPerSecond.toFixed(0)} B/s`
}

/**
 * 将测速耗时格式化为秒文本。
 */
export function formatProxySpeedTestDuration(elapsedMs: number) {
  return `${(elapsedMs / 1000).toFixed(2)} s`
}

/**
 * 调用后端执行单次下载测速。
 */
export async function runProxySpeedTest(
  options: IProxyDownloadSpeedTestOptions,
) {
  return cmdTestDownloadSpeed(options)
}
