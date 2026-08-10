export type BenchmarkWindow =
  | 'sixHours'
  | 'twelveHours'
  | 'oneDay'
  | 'sevenDays'
  | 'longTerm'

export type ParticipationState =
  | 'participating'
  | 'observe'
  | 'excluded'
  | 'changed'
  | 'removed'

export interface BenchmarkSettings {
  enabled: boolean
  selectedProfileUids: string[]
  includeKeywords: string
  excludeKeywords: string
  caseSensitive: boolean
  latencyIntervalMinutes: number
  speedIntervalHours: number
  latencyUrl: string
  downloadUrl: string
  uploadUrl: string
  downloadDailyLimitBytes: number
  uploadDailyLimitBytes: number
  profileTrafficMultipliers: Record<string, number>
  speedDurationMs: number
  speedMaxBytes: number
}

interface BenchmarkProfileSummary {
  uid: string
  name: string
  available: boolean
  selected: boolean
  nodeCount: number
  scanError?: string | null
  lastScannedAt?: number | null
}

export interface MetricSummary {
  median?: number | null
  average?: number | null
  p95?: number | null
  peak?: number | null
  samples: number
  attempts: number
  failures: number
  lastAttemptedAt?: number | null
  lastSuccessAt?: number | null
  lastError?: string | null
  lastErrorAt?: number | null
  rank?: number | null
}

export interface BenchmarkNodeRow {
  nodeId: string
  profileUid: string
  profileName: string
  nodeName: string
  nodeType: string
  fingerprint: string
  lastSeenAt: number
  state: ParticipationState
  stabilityPercent?: number | null
  latencyAttempts: number
  latencySuccesses: number
  latency: MetricSummary
  download: MetricSummary
  upload: MetricSummary
  totalRankScore?: number | null
  officialEligible: boolean
  confidence: 'temporary' | 'initial' | 'daily' | 'longTerm'
  lastTestedAt?: number | null
  bootstrapStage: number
  downloadBootstrapStage: number
  uploadBootstrapStage: number
  nextLatencyAt: number
  nextSpeedAt: number
  nextDownloadAt: number
  nextUploadAt: number
}

export interface ChampionSummary {
  nodeId: string
  nodeName: string
  profileName: string
  value: number
  official: boolean
}

interface ChampionCollection {
  overall?: ChampionSummary | null
  download?: ChampionSummary | null
  upload?: ChampionSummary | null
  latency?: ChampionSummary | null
}

interface ProfileTrafficSummary {
  profileUid: string
  profileName: string
  estimatedDownloadBytes: number
  estimatedUploadBytes: number
  actualDownloadBytes: number
  actualUploadBytes: number
  multiplier: number
  estimatedBilledBytes: number
  actualBilledBytes: number
}

interface TrafficSummary {
  estimatedDownloadBytes: number
  estimatedUploadBytes: number
  actualDownloadBytes: number
  actualUploadBytes: number
  downloadLimitBytes: number
  uploadLimitBytes: number
  profiles: ProfileTrafficSummary[]
}

interface ExclusionReminder {
  nodeId: string
  nodeName: string
  profileName: string
  latencyRank: number
  totalNodes: number
  medianLatency: number
  referenceLatency: number
  downloadRank?: number | null
  uploadRank?: number | null
  stabilityPercent?: number | null
  savedDownloadBytesPerDay: number
  savedUploadBytesPerDay: number
}

interface BenchmarkChangeEvent {
  id: number
  profileUid: string
  profileName: string
  nodeName?: string | null
  changeType: string
  createdAt: number
}

interface BenchmarkWorkerStatus {
  running: boolean
  phase?: string | null
  profileName?: string | null
  nodeName?: string | null
  completed: number
  total: number
  lastError?: string | null
}

interface BenchmarkTaskStatus extends BenchmarkWorkerStatus {
  cancellable: boolean
  latency: BenchmarkWorkerStatus
  speed: BenchmarkWorkerStatus
}

export interface BenchmarkSnapshot {
  settings: BenchmarkSettings
  selectionConfirmed: boolean
  profiles: BenchmarkProfileSummary[]
  nodes: BenchmarkNodeRow[]
  champions: ChampionCollection
  traffic: TrafficSummary
  reminders: ExclusionReminder[]
  changes: BenchmarkChangeEvent[]
  task: BenchmarkTaskStatus
  generatedAt: number
}

export interface BenchmarkCandidateNode {
  nodeId: string
  profileUid: string
  profileName: string
  nodeName: string
  nodeType: string
  fingerprint: string
}

export interface BenchmarkPreview {
  settings: BenchmarkSettings
  nodes: BenchmarkCandidateNode[]
}

export interface BenchmarkSaveRequest {
  settings: BenchmarkSettings
  participatingNodeIds: string[]
}

export interface NodeUpdateRequest {
  nodeId: string
  state?: ParticipationState
  reminderAction?: 'snoozeSevenDays' | 'ignore' | 'reset'
}

type ManualSpeedMode = 'download' | 'upload'

export interface ManualBatchRequest {
  profileUid?: string
  nodeNames: string[]
  mode: ManualSpeedMode
  options: IProxySpeedTestOptions
  includeInRanking?: boolean
}

interface ManualBatchRow {
  nodeName: string
  status: 'queued' | 'testing' | 'success' | 'failed' | 'cancelled'
  error?: string | null
  result?: IProxyDownloadSpeedTestResult | IProxyUploadSpeedTestResult | null
}

export interface ManualBatchSnapshot {
  jobId: string
  running: boolean
  mode: ManualSpeedMode
  rows: ManualBatchRow[]
}
