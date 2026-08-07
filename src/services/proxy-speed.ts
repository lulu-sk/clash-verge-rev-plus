import { cmdTestDownloadSpeed, cmdTestUploadSpeed } from '@/services/cmds'

const MODE_STORAGE_KEY = 'proxy-speed-test-mode'
const SOURCE_STORAGE_KEY = 'proxy-speed-test-source-id'
const UPLOAD_SOURCE_STORAGE_KEY = 'proxy-speed-test-upload-source-id'
const PRESET_STORAGE_KEY = 'proxy-speed-test-preset-id'
const SORT_STORAGE_KEY = 'proxy-speed-test-sort-id'
const CUSTOM_URL_STORAGE_KEY = 'proxy-speed-test-custom-url'
const UPLOAD_CUSTOM_URL_STORAGE_KEY = 'proxy-speed-test-upload-custom-url'
const RESULT_CACHE_STORAGE_KEY = 'proxy-speed-test-result-cache'
export const PROXY_SPEED_TEST_CACHE_CHANGE_EVENT =
  'proxy-speed-test-cache-change'
const DEFAULT_CONNECT_TIMEOUT_MS = 8000
const DEFAULT_READ_IDLE_TIMEOUT_MS = 3000
const MAX_RESULT_CACHE_ENTRIES = 20

export interface ProxySpeedTestSource {
  id: string
  downloadUrl?: string
  uploadUrl?: string
}

export type ProxySpeedTestMode = 'download' | 'upload'

export type ProxySpeedTestResult =
  | IProxyDownloadSpeedTestResult
  | IProxyUploadSpeedTestResult

export interface ProxySpeedTestLatestResults {
  download?: IProxyDownloadSpeedTestResult
  upload?: IProxyUploadSpeedTestResult
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
  result?: ProxySpeedTestResult
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
  mode?: ProxySpeedTestMode
  sourceId: string
  presetId: string
  url: string
  rows: ProxySpeedTestStoredRow[]
  updatedAt: number
}

export const PROXY_SPEED_TEST_SOURCES: ProxySpeedTestSource[] = [
  {
    id: 'cloudflare',
    downloadUrl: 'https://speed.cloudflare.com/__down?bytes=50000000',
    uploadUrl: 'https://speed.cloudflare.com/__up',
  },
  {
    id: 'bunny',
    downloadUrl: 'https://speedtest.bunnycdn.com/100mb.bin',
  },
  {
    id: 'hetzner',
    downloadUrl: 'https://speed.hetzner.de/100MB.bin',
  },
  {
    id: 'custom',
    downloadUrl: '',
    uploadUrl: '',
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
  mode: ProxySpeedTestMode,
  sourceId: string,
  presetId: string,
  url: string,
) {
  if (mode === 'upload') {
    return `${groupName}::upload::${sourceId}::${presetId}::${url}`
  }

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
export function getStoredProxySpeedTestSourceId(mode: ProxySpeedTestMode) {
  const storage = getStorage()
  const storageKey =
    mode === 'download' ? SOURCE_STORAGE_KEY : UPLOAD_SOURCE_STORAGE_KEY
  const storedSourceId = storage?.getItem(storageKey)
  const availableSources = getProxySpeedTestSources(mode)
  return (
    availableSources.find((source) => source.id === storedSourceId)?.id ||
    availableSources[0].id
  )
}

/**
 * 读取上次选择的测速方向。
 */
export function getStoredProxySpeedTestMode(): ProxySpeedTestMode {
  return getStorage()?.getItem(MODE_STORAGE_KEY) === 'upload'
    ? 'upload'
    : 'download'
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
export function setStoredProxySpeedTestSourceId(
  mode: ProxySpeedTestMode,
  sourceId: string,
) {
  const storage = getStorage()
  const storageKey =
    mode === 'download' ? SOURCE_STORAGE_KEY : UPLOAD_SOURCE_STORAGE_KEY
  storage?.setItem(storageKey, sourceId)
}

/**
 * 保存当前测速方向。
 */
export function setStoredProxySpeedTestMode(mode: ProxySpeedTestMode) {
  getStorage()?.setItem(MODE_STORAGE_KEY, mode)
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
export function getStoredProxySpeedTestCustomUrl(mode: ProxySpeedTestMode) {
  const storage = getStorage()
  const storageKey =
    mode === 'download' ? CUSTOM_URL_STORAGE_KEY : UPLOAD_CUSTOM_URL_STORAGE_KEY
  return storage?.getItem(storageKey) || ''
}

/**
 * 保存自定义测速链接。
 */
export function setStoredProxySpeedTestCustomUrl(
  mode: ProxySpeedTestMode,
  url: string,
) {
  const storage = getStorage()
  const storageKey =
    mode === 'download' ? CUSTOM_URL_STORAGE_KEY : UPLOAD_CUSTOM_URL_STORAGE_KEY
  storage?.setItem(storageKey, url)
}

/**
 * 读取支持指定方向的测速源。
 */
export function getProxySpeedTestSources(mode: ProxySpeedTestMode) {
  return PROXY_SPEED_TEST_SOURCES.filter((source) =>
    mode === 'download'
      ? source.downloadUrl !== undefined
      : source.uploadUrl !== undefined,
  )
}

/**
 * 将测速源选择解析为最终的测速链接。
 */
export function resolveProxySpeedTestUrl(
  mode: ProxySpeedTestMode,
  sourceId: string,
  customUrl: string,
) {
  if (sourceId === 'custom') {
    return customUrl.trim()
  }

  const source = PROXY_SPEED_TEST_SOURCES.find(
    (source) => source.id === sourceId,
  )
  return mode === 'download'
    ? source?.downloadUrl || ''
    : source?.uploadUrl || ''
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
  mode: ProxySpeedTestMode,
  sourceId: string,
  presetId: string,
  url: string,
) {
  if (!groupName || !url) return []

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const cacheKey = getProxySpeedTestCacheKey(
    groupName,
    mode,
    sourceId,
    presetId,
    url,
  )
  return cacheRecord[cacheKey]?.rows || []
}

/**
 * 保存指定条件下的测速结果缓存。
 */
export function setStoredProxySpeedTestCachedRows(
  groupName: string,
  mode: ProxySpeedTestMode,
  sourceId: string,
  presetId: string,
  url: string,
  rows: ProxySpeedTestStoredRow[],
) {
  if (!groupName || !url || rows.length === 0) return

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const cacheKey = getProxySpeedTestCacheKey(
    groupName,
    mode,
    sourceId,
    presetId,
    url,
  )

  cacheRecord[cacheKey] = {
    key: cacheKey,
    groupName,
    mode,
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
 * 读取指定代理组每个节点最近一次成功的双向测速结果。
 */
export function getLatestProxySpeedTestResultsMap(groupName: string) {
  const resultMap = new Map<string, ProxySpeedTestLatestResults>()
  if (!groupName) return resultMap

  const cacheRecord = getProxySpeedTestResultCacheRecord()
  const entries = Object.values(cacheRecord)
    .filter((entry) => entry.groupName === groupName)
    .sort((left, right) => right.updatedAt - left.updatedAt)

  for (const entry of entries) {
    for (const row of entry.rows) {
      if (row.status !== 'success' || !row.result) continue

      const latestResults = resultMap.get(row.name) || {}
      if ('bytesRead' in row.result && !latestResults.download) {
        resultMap.set(row.name, {
          ...latestResults,
          download: row.result,
        })
      } else if ('bytesSent' in row.result && !latestResults.upload) {
        resultMap.set(row.name, {
          ...latestResults,
          upload: row.result,
        })
      }
    }
  }

  return resultMap
}

/**
 * 读取指定代理组每个节点最近一次成功的下载测速结果。
 */
export function getLatestProxySpeedTestResultMap(groupName: string) {
  const downloadResultMap = new Map<string, IProxyDownloadSpeedTestResult>()

  for (const [proxyName, results] of getLatestProxySpeedTestResultsMap(
    groupName,
  )) {
    if (results.download) {
      downloadResultMap.set(proxyName, results.download)
    }
  }

  return downloadResultMap
}

/**
 * 构造后端测速命令的参数。
 */
export function getProxySpeedTestOptions(
  url: string,
  presetId: string,
): IProxySpeedTestOptions {
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
 * 将传输字节数格式化为更易读的文本。
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
 * 将平均传输速度格式化为更易读的文本。
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
 * 读取下载或上传结果中的实际传输字节数。
 */
export function getProxySpeedTestTransferredBytes(
  result: ProxySpeedTestResult,
) {
  return 'bytesRead' in result ? result.bytesRead : result.bytesSent
}

/**
 * 调用后端执行指定方向的单次测速。
 */
export async function runProxySpeedTest(
  mode: ProxySpeedTestMode,
  options: IProxySpeedTestOptions,
) {
  return mode === 'download'
    ? cmdTestDownloadSpeed(options)
    : cmdTestUploadSpeed(options)
}
