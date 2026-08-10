import {
  CheckCircleOutlined,
  DeleteSweepOutlined,
  ErrorOutlined,
  ExpandMoreOutlined,
  PlayArrowOutlined,
  RefreshOutlined,
  RestartAltOutlined,
  SaveOutlined,
  SpeedOutlined,
  StopCircleOutlined,
} from '@mui/icons-material'
import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  Checkbox,
  Chip,
  CircularProgress,
  Divider,
  FormControlLabel,
  MenuItem,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  ToggleButton,
  ToggleButtonGroup,
  Tooltip,
  Typography,
  useMediaQuery,
} from '@mui/material'
import { useTheme } from '@mui/material/styles'
import {
  Fragment,
  type ReactNode,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { useTranslation } from 'react-i18next'

import { BaseDialog, BasePage } from '@/components/base'
import { ensureLanguageSections } from '@/services/i18n'
import {
  type BenchmarkCandidateNode,
  type BenchmarkNodeRow,
  type BenchmarkPreview,
  type BenchmarkSettings,
  type BenchmarkSnapshot,
  type BenchmarkWindow,
  type ChampionSummary,
  type MetricSummary,
  type ParticipationState,
  clearNodeBenchmarkHistory,
  getNodeBenchmarkSnapshot,
  pauseNodeBenchmark,
  previewNodeBenchmarkCandidates,
  refreshNodeBenchmarkProfiles,
  retestNodeBenchmarkNodes,
  saveNodeBenchmarkSettings,
  startNodeBenchmark,
  updateNodeBenchmarkNode,
} from '@/services/node-benchmark'
import {
  benchmarkCandidateScopeMatches,
  benchmarkSettingsMatch,
  nodeMatchesBenchmarkFilters,
  shouldAutoRefreshBenchmarkCandidates,
} from '@/services/node-benchmark/selection'
import { showNotice } from '@/services/notice-service'

const GIB = 1024 ** 3
const MIB = 1024 ** 2

const WINDOW_OPTIONS: BenchmarkWindow[] = [
  'sixHours',
  'twelveHours',
  'oneDay',
  'sevenDays',
  'longTerm',
]

const LATENCY_INTERVALS = [5, 10, 15, 30, 60]
const SPEED_INTERVALS = [1, 3, 6, 12, 24, 48]

type NodeSortKey = 'overall' | 'download' | 'upload' | 'latency' | 'profile'
type ParticipationFilter = ParticipationState | 'all'

/** 把字节数格式化成适合流量卡片显示的短文本。 */
function formatBytes(bytes?: number | null) {
  if (bytes == null) return '—'
  if (bytes >= GIB) return `${(bytes / GIB).toFixed(2)} GiB`
  if (bytes >= MIB) return `${(bytes / MIB).toFixed(1)} MiB`
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`
  return `${bytes.toFixed(0)} B`
}

/** 把每秒字节数格式化成用户容易比较的速度。 */
function formatSpeed(bytesPerSecond?: number | null) {
  if (bytesPerSecond == null) return '—'
  if (bytesPerSecond >= GIB) return `${(bytesPerSecond / GIB).toFixed(2)} GiB/s`
  if (bytesPerSecond >= MIB) return `${(bytesPerSecond / MIB).toFixed(1)} MiB/s`
  return `${(bytesPerSecond / 1024).toFixed(1)} KiB/s`
}

/** 把毫秒延迟格式化为整数文本。 */
function formatLatency(value?: number | null) {
  return value == null ? '—' : `${value.toFixed(0)} ms`
}

/** 把 Unix 秒时间戳格式化成本地日期时间。 */
function formatTimestamp(timestamp?: number | null) {
  return timestamp ? new Date(timestamp * 1000).toLocaleString() : '—'
}

/** 把已保存的榜单节点转换成保存前列表使用的轻量候选节点。 */
function toCandidateNode(node: BenchmarkNodeRow): BenchmarkCandidateNode {
  return {
    nodeId: node.nodeId,
    profileUid: node.profileUid,
    profileName: node.profileName,
    nodeName: node.nodeName,
    nodeType: node.nodeType,
    fingerprint: node.fingerprint,
  }
}

interface ChampionCardProps {
  title: string
  champion?: ChampionSummary | null
  value: (champion: ChampionSummary) => string
}

/** 展示一项榜首结论，并始终同时显示来源订阅和正式性。 */
function ChampionCard({ title, champion, value }: ChampionCardProps) {
  const { t } = useTranslation()
  return (
    <Card variant="outlined" sx={{ minWidth: 0 }}>
      <CardContent sx={{ p: 2, '&:last-child': { pb: 2 } }}>
        <Typography color="text.secondary" variant="caption">
          {title}
        </Typography>
        {champion ? (
          <>
            <Typography noWrap sx={{ mt: 0.5, fontWeight: 700 }}>
              {champion.nodeName}
            </Typography>
            <Typography color="text.secondary" noWrap variant="caption">
              {t('node-benchmark.champions.source', {
                profile: champion.profileName,
              })}
            </Typography>
            <Stack
              direction="row"
              sx={{ mt: 1, justifyContent: 'space-between' }}
            >
              <Typography color="primary" sx={{ fontWeight: 700 }}>
                {value(champion)}
              </Typography>
              <Chip
                color={champion.official ? 'success' : 'default'}
                label={t(
                  champion.official
                    ? 'node-benchmark.champions.official'
                    : 'node-benchmark.champions.provisional',
                )}
                size="small"
              />
            </Stack>
          </>
        ) : (
          <Typography color="text.secondary" sx={{ mt: 2 }}>
            {t('node-benchmark.champions.waiting')}
          </Typography>
        )}
      </CardContent>
    </Card>
  )
}

interface MetricCellProps {
  metric: MetricSummary
  kind: 'speed' | 'latency'
  nextAt?: number | null
}

/** 展示指标主值、名次、样本和用于判断波动的辅助信息。 */
function MetricCell({ metric, kind, nextAt }: MetricCellProps) {
  const { t } = useTranslation()
  const lastErrorDescription = metric.lastError
    ? `${formatTimestamp(metric.lastErrorAt)} · ${metric.lastError}`
    : ''
  return (
    <Box sx={{ minWidth: 135 }}>
      <Typography sx={{ fontWeight: 700 }} variant="body2">
        {metric.median == null
          ? t('node-benchmark.table.noData')
          : kind === 'speed'
            ? formatSpeed(metric.median)
            : formatLatency(metric.median)}
      </Typography>
      <Typography color="text.secondary" variant="caption">
        {metric.rank
          ? t('node-benchmark.table.rank', {
              rank: metric.rank,
              samples: metric.samples,
            })
          : t('node-benchmark.table.samples', { samples: metric.samples })}
      </Typography>
      <Typography
        color="text.secondary"
        sx={{ display: 'block' }}
        variant="caption"
      >
        {t('node-benchmark.table.attemptDetail', {
          attempts: metric.attempts,
          failures: metric.failures,
        })}
      </Typography>
      {nextAt != null && (
        <Typography
          color="text.secondary"
          sx={{ display: 'block' }}
          variant="caption"
        >
          {t('node-benchmark.table.nextTest', {
            time: formatTimestamp(nextAt),
          })}
        </Typography>
      )}
      {metric.lastError && (
        <Tooltip arrow title={lastErrorDescription}>
          <Chip
            aria-label={lastErrorDescription}
            color="error"
            icon={<ErrorOutlined />}
            label={t('node-benchmark.table.lastFailed')}
            size="small"
            sx={{ mt: 0.5, maxWidth: 125 }}
            tabIndex={0}
            variant="outlined"
          />
        </Tooltip>
      )}
      {metric.median != null &&
      kind === 'latency' &&
      metric.average != null &&
      metric.p95 != null ? (
        <Typography
          color="text.secondary"
          sx={{ display: 'block' }}
          variant="caption"
        >
          {t('node-benchmark.table.latencyDetail', {
            average: formatLatency(metric.average),
            p95: formatLatency(metric.p95),
          })}
        </Typography>
      ) : metric.median != null && metric.peak != null ? (
        <Typography
          color="text.secondary"
          sx={{ display: 'block' }}
          variant="caption"
        >
          {t('node-benchmark.table.peak', {
            value: formatSpeed(metric.peak),
          })}
        </Typography>
      ) : null}
    </Box>
  )
}

interface CollapsibleSectionProps {
  children: ReactNode
  defaultExpanded?: boolean
  id: string
  subtitle?: ReactNode
  summary?: ReactNode
  title: ReactNode
}

interface NodeStateSelectProps {
  fullWidth?: boolean
  onChange: (state: ParticipationState) => void
  row: BenchmarkNodeRow
}

/** 提供统一、可访问且适配窄屏的顶层折叠区块。 */
function CollapsibleSection({
  children,
  defaultExpanded = false,
  id,
  subtitle,
  summary,
  title,
}: CollapsibleSectionProps) {
  const [expanded, setExpanded] = useState(defaultExpanded)
  return (
    <Accordion
      disableGutters
      expanded={expanded}
      onChange={(_, nextExpanded) => setExpanded(nextExpanded)}
      slotProps={{ transition: { unmountOnExit: true } }}
      sx={{
        minWidth: 0,
        '&:before': { display: 'none' },
        '&.Mui-expanded': { m: 0 },
      }}
      variant="outlined"
    >
      <AccordionSummary
        aria-controls={`${id}-content`}
        expandIcon={<ExpandMoreOutlined />}
        id={`${id}-header`}
        sx={{
          minHeight: 56,
          px: { xs: 1.5, sm: 2 },
          '& .MuiAccordionSummary-content': {
            alignItems: { xs: 'flex-start', sm: 'center' },
            flexWrap: 'wrap',
            gap: 1,
            minWidth: 0,
          },
        }}
      >
        <Box sx={{ flex: 1, minWidth: 0 }}>
          <Typography component="h2" sx={{ fontWeight: 700 }} variant="h6">
            {title}
          </Typography>
          {subtitle && expanded && (
            <Typography
              color="text.secondary"
              sx={{ display: 'block' }}
              variant="caption"
            >
              {subtitle}
            </Typography>
          )}
        </Box>
        {summary && (
          <Box
            sx={{
              flexShrink: 1,
              maxWidth: { xs: '100%', sm: '50%' },
              minWidth: 0,
              '& .MuiChip-root': { maxWidth: '100%' },
              '& .MuiChip-label': {
                overflow: 'hidden',
                textOverflow: 'ellipsis',
              },
            }}
          >
            {summary}
          </Box>
        )}
      </AccordionSummary>
      <AccordionDetails
        aria-labelledby={`${id}-header`}
        id={`${id}-content`}
        sx={{ px: { xs: 1.5, sm: 2 }, pt: 0, pb: { xs: 1.5, sm: 2 } }}
      >
        {children}
      </AccordionDetails>
    </Accordion>
  )
}

/** 统一桌面表格与窄屏卡片中的节点参赛状态选择。 */
function NodeStateSelect({
  fullWidth = false,
  onChange,
  row,
}: NodeStateSelectProps) {
  const { t } = useTranslation()
  return (
    <TextField
      fullWidth={fullWidth}
      onChange={(event) => onChange(event.target.value as ParticipationState)}
      select
      size="small"
      value={row.state}
    >
      <MenuItem value="participating">
        {t('node-benchmark.states.participating')}
      </MenuItem>
      <MenuItem value="observe">{t('node-benchmark.states.observe')}</MenuItem>
      <MenuItem value="excluded">
        {t('node-benchmark.states.excluded')}
      </MenuItem>
      {row.state === 'changed' && (
        <MenuItem value="changed" disabled>
          {t('node-benchmark.states.changed')}
        </MenuItem>
      )}
      {row.state === 'removed' && (
        <MenuItem value="removed" disabled>
          {t('node-benchmark.states.removed')}
        </MenuItem>
      )}
    </TextField>
  )
}

/** 提供多订阅长期节点评选的完整设置、排名、流量和人工淘汰面板。 */
export default function NodeBenchmarkPage() {
  const { t } = useTranslation()
  const theme = useTheme()
  const compactLeaderboard = useMediaQuery(theme.breakpoints.down('xl'))
  const [translationReady, setTranslationReady] = useState(false)
  const [snapshot, setSnapshot] = useState<BenchmarkSnapshot | null>(null)
  const [draft, setDraft] = useState<BenchmarkSettings | null>(null)
  const [windowValue, setWindowValue] = useState<BenchmarkWindow>('oneDay')
  const [search, setSearch] = useState('')
  const [sourceFilter, setSourceFilter] = useState('all')
  const [stateFilter, setStateFilter] =
    useState<ParticipationFilter>('participating')
  const [sortKey, setSortKey] = useState<NodeSortKey>('overall')
  const [groupBySource, setGroupBySource] = useState(false)
  const [selectedNodes, setSelectedNodes] = useState<Set<string>>(
    () => new Set(),
  )
  const [candidateNodeIds, setCandidateNodeIds] = useState<Set<string>>(
    () => new Set(),
  )
  const [candidatePreview, setCandidatePreview] =
    useState<BenchmarkPreview | null>(null)
  const [candidateSearch, setCandidateSearch] = useState('')
  const [candidateDirty, setCandidateDirty] = useState(false)
  const [previewLoading, setPreviewLoading] = useState(false)
  const [busy, setBusy] = useState(false)
  const [controlBusy, setControlBusy] = useState(false)
  const [clearHistoryOpen, setClearHistoryOpen] = useState(false)
  const [clearHistoryBusy, setClearHistoryBusy] = useState(false)
  const draftInitializedRef = useRef(false)
  const candidateInitializedRef = useRef(false)
  const previousSelectionConfirmedRef = useRef<boolean | null>(null)
  const candidateEditVersionRef = useRef(0)
  const snapshotRequestIdRef = useRef(0)
  const previewRequestIdRef = useRef(0)
  const previewTimerRef = useRef<number | null>(null)

  /** 读取后台快照并只在首次加载时初始化可编辑设置和已确认节点。 */
  const loadSnapshot = useCallback(async (targetWindow: BenchmarkWindow) => {
    const requestId = ++snapshotRequestIdRef.current
    const next = await getNodeBenchmarkSnapshot(targetWindow)
    if (requestId !== snapshotRequestIdRef.current) return next
    setSnapshot(next)
    if (!draftInitializedRef.current) {
      setDraft(next.settings)
      draftInitializedRef.current = true
    }
    if (!candidateInitializedRef.current) {
      setCandidatePreview({
        settings: { ...next.settings, enabled: false },
        nodes: next.nodes.map(toCandidateNode),
      })
      setCandidateNodeIds(
        new Set(
          next.nodes
            .filter((node) => node.state === 'participating')
            .map((node) => node.nodeId),
        ),
      )
      setCandidateDirty(!next.selectionConfirmed && next.nodes.length > 0)
      candidateInitializedRef.current = true
    }
    return next
  }, [])

  useEffect(() => {
    ensureLanguageSections('node-benchmark')
      .then(() => setTranslationReady(true))
      .catch((error) => showNotice.error(error))
  }, [])

  /** 首次进入页面后每五秒刷新一次后台快照。 */
  useEffect(() => {
    if (!translationReady) return
    loadSnapshot(windowValue).catch((error) => showNotice.error(error))
    const timer = window.setInterval(() => {
      loadSnapshot(windowValue).catch(console.error)
    }, 5000)
    return () => window.clearInterval(timer)
  }, [loadSnapshot, translationReady, windowValue])

  /** 标记候选列表存在用户编辑，防止后台异步刷新覆盖当前操作。 */
  const markCandidateDirty = useCallback(() => {
    candidateEditVersionRef.current += 1
    setCandidateDirty(true)
  }, [])

  /** 订阅出现待确认变化时，在不覆盖用户编辑的前提下自动刷新候选节点。 */
  useEffect(() => {
    if (!snapshot || !draft || !candidateInitializedRef.current) return
    const previousSelectionConfirmed = previousSelectionConfirmedRef.current
    previousSelectionConfirmedRef.current = snapshot.selectionConfirmed
    if (
      !shouldAutoRefreshBenchmarkCandidates(
        previousSelectionConfirmed,
        snapshot.selectionConfirmed,
        candidateDirty,
        draft,
        snapshot.settings,
      )
    )
      return

    const requestId = ++previewRequestIdRef.current
    const editVersion = candidateEditVersionRef.current
    void (async () => {
      await Promise.resolve()
      if (
        previewRequestIdRef.current !== requestId ||
        candidateEditVersionRef.current !== editVersion
      )
        return
      setPreviewLoading(true)
      try {
        const preview = await previewNodeBenchmarkCandidates({
          ...snapshot.settings,
          enabled: false,
        })
        if (
          previewRequestIdRef.current !== requestId ||
          candidateEditVersionRef.current !== editVersion
        )
          return
        const availableNodeIds = new Set(
          preview.nodes.map((node) => node.nodeId),
        )
        setCandidatePreview(preview)
        setCandidateNodeIds(
          new Set(
            snapshot.nodes
              .filter(
                (node) =>
                  node.state === 'participating' &&
                  availableNodeIds.has(node.nodeId),
              )
              .map((node) => node.nodeId),
          ),
        )
        setCandidateDirty(
          !snapshot.selectionConfirmed && preview.nodes.length > 0,
        )
      } catch (error) {
        if (previewRequestIdRef.current === requestId) showNotice.error(error)
      } finally {
        if (previewRequestIdRef.current === requestId) setPreviewLoading(false)
      }
    })()
  }, [candidateDirty, draft, snapshot])

  /** 合并一部分草稿设置，避免每个字段重复复制完整对象。 */
  const patchDraft = useCallback((patch: Partial<BenchmarkSettings>) => {
    setDraft((current) => (current ? { ...current, ...patch } : current))
  }, [])

  /** 实时应用关键字过滤，并移除已经不在当前过滤结果中的勾选。 */
  const handleFilterChange = useCallback(
    (
      patch: Pick<
        Partial<BenchmarkSettings>,
        'includeKeywords' | 'excludeKeywords' | 'caseSensitive'
      >,
    ) => {
      if (!draft) return
      const nextDraft = { ...draft, ...patch }
      setDraft(nextDraft)
      setCandidateNodeIds((current) => {
        if (!candidatePreview) return current
        const allowed = new Set(
          candidatePreview.nodes
            .filter((node) => nodeMatchesBenchmarkFilters(node, nextDraft))
            .map((node) => node.nodeId),
        )
        return new Set([...current].filter((nodeId) => allowed.has(nodeId)))
      })
      markCandidateDirty()
    },
    [candidatePreview, draft, markCandidateDirty],
  )

  useEffect(
    () => () => {
      previewRequestIdRef.current += 1
      if (previewTimerRef.current != null)
        window.clearTimeout(previewTimerRef.current)
    },
    [],
  )

  /** 更新订阅勾选并自动读取新范围内的全部候选节点。 */
  const handleProfileSelection = useCallback(
    (profileUid: string, checked: boolean) => {
      if (!draft) return
      const selectedProfileUids = checked
        ? [...draft.selectedProfileUids, profileUid]
        : draft.selectedProfileUids.filter((uid) => uid !== profileUid)
      const nextDraft = { ...draft, selectedProfileUids }
      const requestId = ++previewRequestIdRef.current
      setDraft(nextDraft)
      markCandidateDirty()
      if (previewTimerRef.current != null)
        window.clearTimeout(previewTimerRef.current)

      if (selectedProfileUids.length === 0) {
        setCandidatePreview({
          settings: { ...nextDraft, enabled: false },
          nodes: [],
        })
        setCandidateNodeIds(new Set())
        setPreviewLoading(false)
        return
      }

      setPreviewLoading(true)
      previewTimerRef.current = window.setTimeout(() => {
        previewTimerRef.current = null
        previewNodeBenchmarkCandidates({ ...nextDraft, enabled: false })
          .then((preview) => {
            if (previewRequestIdRef.current !== requestId) return
            setCandidatePreview(preview)
            setCandidateNodeIds((current) => {
              const available = new Set(
                preview.nodes.map((node) => node.nodeId),
              )
              return new Set(
                [...current].filter((nodeId) => available.has(nodeId)),
              )
            })
            setCandidateDirty(true)
          })
          .catch((error) => {
            if (previewRequestIdRef.current === requestId)
              showNotice.error(error)
          })
          .finally(() => {
            if (previewRequestIdRef.current === requestId)
              setPreviewLoading(false)
          })
      }, 250)
    },
    [draft, markCandidateDirty],
  )

  /** 一次性保存设置和当前勾选节点，并确保长期评选保持停止。 */
  const handleSave = useCallback(async () => {
    if (!draft || !candidatePreview) return
    previewRequestIdRef.current += 1
    candidateEditVersionRef.current += 1
    if (previewTimerRef.current != null) {
      window.clearTimeout(previewTimerRef.current)
      previewTimerRef.current = null
    }
    setPreviewLoading(false)
    snapshotRequestIdRef.current += 1
    setBusy(true)
    try {
      const next = await saveNodeBenchmarkSettings({
        settings: { ...draft, enabled: false },
        participatingNodeIds: candidatePreview.nodes
          .filter((node) => candidateNodeIds.has(node.nodeId))
          .map((node) => node.nodeId),
      })
      if (!next.selectionConfirmed)
        throw new Error(t('node-benchmark.notices.saveIncomplete'))
      snapshotRequestIdRef.current += 1
      setSnapshot(next)
      setDraft(next.settings)
      setCandidatePreview({
        settings: { ...next.settings, enabled: false },
        nodes: next.nodes.map(toCandidateNode),
      })
      setCandidateNodeIds(
        new Set(
          next.nodes
            .filter((node) => node.state === 'participating')
            .map((node) => node.nodeId),
        ),
      )
      setCandidateDirty(false)
      previousSelectionConfirmedRef.current = true
      showNotice.success(t('node-benchmark.notices.saved'))
    } catch (error) {
      showNotice.error(error)
    } finally {
      setBusy(false)
    }
  }, [candidateNodeIds, candidatePreview, draft, t])

  /** 明确启动长期评选，与保存配置动作完全分离。 */
  const handleStart = useCallback(async () => {
    snapshotRequestIdRef.current += 1
    setControlBusy(true)
    try {
      const next = await startNodeBenchmark()
      snapshotRequestIdRef.current += 1
      setSnapshot(next)
      setDraft((current) => (current ? { ...current, enabled: true } : current))
      showNotice.success(t('node-benchmark.notices.started'))
    } catch (error) {
      showNotice.error(error)
    } finally {
      setControlBusy(false)
    }
  }, [t])

  /** 暂停长期评选并保留配置、历史和当前排名。 */
  const handlePause = useCallback(async () => {
    snapshotRequestIdRef.current += 1
    setControlBusy(true)
    try {
      const next = await pauseNodeBenchmark()
      snapshotRequestIdRef.current += 1
      setSnapshot(next)
      setDraft((current) =>
        current ? { ...current, enabled: false } : current,
      )
      showNotice.success(t('node-benchmark.notices.paused'))
    } catch (error) {
      showNotice.error(error)
    } finally {
      setControlBusy(false)
    }
  }, [t])

  /** 强制扫描订阅并刷新当前窗口。 */
  const handleRefresh = useCallback(async () => {
    setBusy(true)
    try {
      await refreshNodeBenchmarkProfiles()
      const next = await loadSnapshot(windowValue)
      setCandidatePreview({
        settings: { ...next.settings, enabled: false },
        nodes: next.nodes.map(toCandidateNode),
      })
      setCandidateNodeIds(
        new Set(
          next.nodes
            .filter((node) => node.state === 'participating')
            .map((node) => node.nodeId),
        ),
      )
      setCandidateDirty(!next.selectionConfirmed && next.nodes.length > 0)
      showNotice.success(t('node-benchmark.notices.refreshed'))
    } catch (error) {
      showNotice.error(error)
    } finally {
      setBusy(false)
    }
  }, [loadSnapshot, t, windowValue])

  /** 更新节点状态；排除操作必须先得到用户明确确认。 */
  const handleState = useCallback(
    async (row: BenchmarkNodeRow, state: ParticipationState) => {
      if (
        state === 'excluded' &&
        !window.confirm(
          t('node-benchmark.notices.confirmExclude', {
            name: row.nodeName,
            profile: row.profileName,
          }),
        )
      )
        return
      try {
        await updateNodeBenchmarkNode({ nodeId: row.nodeId, state })
        const next = await loadSnapshot(windowValue)
        setCandidateNodeIds(
          new Set(
            next.nodes
              .filter((node) => node.state === 'participating')
              .map((node) => node.nodeId),
          ),
        )
        setCandidateDirty(!next.selectionConfirmed && next.nodes.length > 0)
        showNotice.success(t('node-benchmark.notices.updated'))
      } catch (error) {
        showNotice.error(error)
      }
    },
    [loadSnapshot, t, windowValue],
  )

  /** 对提醒执行仅观察、延后或不再提醒，排除仍复用确认逻辑。 */
  const handleReminder = useCallback(
    async (nodeId: string, action: 'snoozeSevenDays' | 'ignore') => {
      try {
        await updateNodeBenchmarkNode({ nodeId, reminderAction: action })
        await loadSnapshot(windowValue)
      } catch (error) {
        showNotice.error(error)
      }
    },
    [loadSnapshot, windowValue],
  )

  /** 把当前勾选节点的计划推进到现在。 */
  const handleRetest = useCallback(async () => {
    try {
      await retestNodeBenchmarkNodes([...selectedNodes])
      showNotice.success(t('node-benchmark.notices.retestQueued'))
      setSelectedNodes(new Set())
      await loadSnapshot(windowValue)
    } catch (error) {
      showNotice.error(error)
    }
  }, [loadSnapshot, selectedNodes, t, windowValue])

  /** 二次确认后清空历史成绩，并刷新为新一轮尚无样本的面板。 */
  const handleClearHistory = useCallback(async () => {
    setClearHistoryBusy(true)
    try {
      const count = await clearNodeBenchmarkHistory()
      setClearHistoryOpen(false)
      setSelectedNodes(new Set())
      await loadSnapshot(windowValue)
      showNotice.success(
        t('node-benchmark.history.cleared', {
          count,
        }),
      )
    } catch (error) {
      showNotice.error(error)
    } finally {
      setClearHistoryBusy(false)
    }
  }, [loadSnapshot, t, windowValue])

  const filteredNodes = useMemo(() => {
    const keyword = search.trim().toLocaleLowerCase()
    const rows = (snapshot?.nodes ?? []).filter(
      (row) =>
        (sourceFilter === 'all' || row.profileUid === sourceFilter) &&
        (stateFilter === 'all' || row.state === stateFilter) &&
        (!keyword ||
          `${row.nodeName} ${row.profileName}`
            .toLocaleLowerCase()
            .includes(keyword)),
    )
    rows.sort((left, right) => {
      const profileOrder = left.profileName.localeCompare(right.profileName)
      if (groupBySource && profileOrder !== 0) return profileOrder
      const valueOrder = (() => {
        switch (sortKey) {
          case 'download':
            return (right.download.median ?? -1) - (left.download.median ?? -1)
          case 'upload':
            return (right.upload.median ?? -1) - (left.upload.median ?? -1)
          case 'latency':
            return (
              (left.latency.median ?? Number.MAX_VALUE) -
              (right.latency.median ?? Number.MAX_VALUE)
            )
          case 'profile':
            return profileOrder
          default:
            return (
              (left.totalRankScore ?? Number.MAX_SAFE_INTEGER) -
              (right.totalRankScore ?? Number.MAX_SAFE_INTEGER)
            )
        }
      })()
      return (
        valueOrder ||
        profileOrder ||
        left.nodeName.localeCompare(right.nodeName)
      )
    })
    return rows
  }, [
    groupBySource,
    search,
    snapshot?.nodes,
    sortKey,
    sourceFilter,
    stateFilter,
  ])

  const candidateRows = useMemo(() => {
    return candidatePreview?.nodes ?? []
  }, [candidatePreview])

  const keywordFilteredCandidateRows = useMemo(() => {
    if (!draft) return []
    return candidateRows.filter((node) =>
      nodeMatchesBenchmarkFilters(node, draft),
    )
  }, [candidateRows, draft])

  const visibleCandidateRows = useMemo(() => {
    const keyword = candidateSearch.trim().toLocaleLowerCase()
    if (!keyword) return keywordFilteredCandidateRows
    return keywordFilteredCandidateRows.filter((node) =>
      `${node.nodeName} ${node.profileName} ${node.nodeType}`
        .toLocaleLowerCase()
        .includes(keyword),
    )
  }, [candidateSearch, keywordFilteredCandidateRows])

  const candidateSelectedCount = useMemo(
    () =>
      candidateRows.filter((node) => candidateNodeIds.has(node.nodeId)).length,
    [candidateNodeIds, candidateRows],
  )

  const filterPreview = useMemo(() => {
    const included = keywordFilteredCandidateRows.length
    return { included, excluded: candidateRows.length - included }
  }, [candidateRows.length, keywordFilteredCandidateRows.length])

  const candidateScopeMatches = useMemo(
    () =>
      draft && candidatePreview
        ? benchmarkCandidateScopeMatches(draft, candidatePreview.settings)
        : false,
    [candidatePreview, draft],
  )
  const settingsSaved = useMemo(
    () =>
      draft && snapshot
        ? benchmarkSettingsMatch(draft, snapshot.settings)
        : false,
    [draft, snapshot],
  )
  const candidateNeedsSave =
    candidateDirty ||
    previewLoading ||
    !candidateScopeMatches ||
    !settingsSaved ||
    !snapshot?.selectionConfirmed

  if (!translationReady || !snapshot || !draft) {
    return (
      <BasePage title="节点评选">
        <Stack sx={{ py: 8, alignItems: 'center', gap: 2 }}>
          <CircularProgress size={28} />
          <Typography color="text.secondary">
            {t('node-benchmark.loading')}
          </Typography>
        </Stack>
      </BasePage>
    )
  }

  const overLimit =
    snapshot.traffic.estimatedDownloadBytes >
      snapshot.traffic.downloadLimitBytes ||
    snapshot.traffic.estimatedUploadBytes > snapshot.traffic.uploadLimitBytes
  const availableProfileCount = snapshot.profiles.filter(
    (profile) => profile.available,
  ).length
  const participatingNodeCount = snapshot.nodes.filter(
    (node) => node.state === 'participating',
  ).length
  const phaseLabels: Record<string, string> = {
    preparing: t('node-benchmark.task.preparing'),
    checkingNetwork: t('node-benchmark.task.checkingNetwork'),
    startingCore: t('node-benchmark.task.startingCore'),
    latency: t('node-benchmark.task.latency'),
    download: t('node-benchmark.task.download'),
    upload: t('node-benchmark.task.upload'),
    cooldown: t('node-benchmark.task.cooldown'),
    waitingForOtherQueue: t('node-benchmark.task.waitingForOtherQueue'),
    cancelled: t('node-benchmark.task.cancelled'),
    failed: t('node-benchmark.task.failed'),
  }
  const nextLatencyQueueAt = snapshot.nodes
    .filter(
      (node) => node.state === 'participating' || node.state === 'observe',
    )
    .reduce<number | null>(
      (earliest, node) =>
        earliest == null
          ? node.nextLatencyAt
          : Math.min(earliest, node.nextLatencyAt),
      null,
    )
  const nextSpeedQueueAt = snapshot.nodes
    .filter((node) => node.state === 'participating')
    .reduce<number | null>((earliest, node) => {
      const nodeNext = Math.min(node.nextDownloadAt, node.nextUploadAt)
      return earliest == null ? nodeNext : Math.min(earliest, nodeNext)
    }, null)
  const workerSummaries = [
    {
      key: 'latency',
      title: t('node-benchmark.task.latencyQueue'),
      worker: snapshot.task.latency,
      nextAt: nextLatencyQueueAt,
    },
    {
      key: 'speed',
      title: t('node-benchmark.task.speedQueue'),
      worker: snapshot.task.speed,
      nextAt: nextSpeedQueueAt,
    },
  ]
  const startDisabledReason = candidateNeedsSave
    ? t('node-benchmark.control.saveFirst')
    : candidateSelectedCount === 0
      ? t('node-benchmark.control.selectFirst')
      : ''

  return (
    <BasePage
      full
      title={t('node-benchmark.title')}
      header={
        <Stack
          direction="row"
          sx={{
            alignItems: 'center',
            flexWrap: 'nowrap',
            gap: { xs: 0.5, sm: 1 },
          }}
        >
          <Chip
            color={snapshot.settings.enabled ? 'success' : 'default'}
            icon={
              snapshot.task.running ? (
                <CircularProgress color="inherit" size={14} />
              ) : snapshot.settings.enabled ? (
                <CheckCircleOutlined />
              ) : (
                <StopCircleOutlined />
              )
            }
            label={
              snapshot.task.running
                ? t('node-benchmark.task.running', {
                    phase:
                      phaseLabels[snapshot.task.phase ?? ''] ??
                      snapshot.task.phase,
                    completed: snapshot.task.completed,
                    total: snapshot.task.total,
                  })
                : t(
                    snapshot.settings.enabled
                      ? 'node-benchmark.task.waiting'
                      : 'node-benchmark.task.paused',
                  )
            }
            size="small"
            sx={{ display: { xs: 'none', md: 'flex' }, maxWidth: 300 }}
            variant="outlined"
          />
          {snapshot.settings.enabled ? (
            <Button
              aria-label={t('node-benchmark.actions.stop')}
              color="warning"
              disabled={controlBusy}
              onClick={handlePause}
              size="small"
              startIcon={
                controlBusy ? (
                  <CircularProgress color="inherit" size={14} />
                ) : (
                  <StopCircleOutlined />
                )
              }
              sx={{
                minHeight: 44,
                minWidth: { xs: 44, sm: 64 },
                px: { xs: 1, sm: 2 },
                '& .MuiButton-startIcon': {
                  m: { xs: 0, sm: '0 8px 0 -4px' },
                },
              }}
              variant="contained"
            >
              <Box
                component="span"
                sx={{ display: { xs: 'none', sm: 'inline' } }}
              >
                {t('node-benchmark.actions.stop')}
              </Box>
            </Button>
          ) : (
            <Tooltip title={startDisabledReason}>
              <span
                aria-label={startDisabledReason || undefined}
                tabIndex={startDisabledReason ? 0 : undefined}
              >
                <Button
                  aria-label={t('node-benchmark.actions.run')}
                  disabled={
                    busy ||
                    controlBusy ||
                    candidateNeedsSave ||
                    candidateSelectedCount === 0
                  }
                  onClick={handleStart}
                  size="small"
                  startIcon={
                    controlBusy ? (
                      <CircularProgress color="inherit" size={14} />
                    ) : (
                      <PlayArrowOutlined />
                    )
                  }
                  sx={{
                    minHeight: 44,
                    minWidth: { xs: 44, sm: 64 },
                    px: { xs: 1, sm: 2 },
                    '& .MuiButton-startIcon': {
                      m: { xs: 0, sm: '0 8px 0 -4px' },
                    },
                  }}
                  variant="contained"
                >
                  <Box
                    component="span"
                    sx={{ display: { xs: 'none', sm: 'inline' } }}
                  >
                    {t('node-benchmark.actions.run')}
                  </Box>
                </Button>
              </span>
            </Tooltip>
          )}
          <Button
            aria-label={t('node-benchmark.actions.refresh')}
            disabled={busy}
            onClick={handleRefresh}
            size="small"
            startIcon={
              busy ? (
                <CircularProgress color="inherit" size={14} />
              ) : (
                <RefreshOutlined />
              )
            }
            sx={{
              minHeight: 44,
              minWidth: { xs: 44, sm: 64 },
              px: { xs: 1, sm: 2 },
              '& .MuiButton-startIcon': {
                m: { xs: 0, sm: '0 8px 0 -4px' },
              },
            }}
          >
            <Box
              component="span"
              sx={{ display: { xs: 'none', sm: 'inline' } }}
            >
              {t('node-benchmark.actions.refresh')}
            </Box>
          </Button>
        </Stack>
      }
      contentStyle={{
        boxSizing: 'border-box',
        height: '100%',
        overflow: 'auto',
        padding: 0,
      }}
    >
      <Stack sx={{ gap: { xs: 1, sm: 2.5 }, minWidth: 0, p: { xs: 1, sm: 2 } }}>
        <Alert icon={<SpeedOutlined />} severity="info">
          {t('node-benchmark.subtitle')}
        </Alert>

        {snapshot.task.lastError && (
          <Alert severity="error">
            {snapshot.task.lastError} {t('node-benchmark.task.retryHelp')}
          </Alert>
        )}

        {snapshot.settings.enabled && !snapshot.selectionConfirmed && (
          <Alert severity="warning">
            {t('node-benchmark.candidates.pendingWhileRunning')}
          </Alert>
        )}

        <Box
          sx={{
            display: 'grid',
            gap: 1,
            gridTemplateColumns:
              'repeat(auto-fit, minmax(min(100%, 260px), 1fr))',
          }}
        >
          {workerSummaries.map(({ key, nextAt, title, worker }) => (
            <Paper key={key} sx={{ minWidth: 0, p: 1.5 }} variant="outlined">
              <Stack
                direction="row"
                sx={{
                  alignItems: 'center',
                  gap: 1,
                  justifyContent: 'space-between',
                }}
              >
                <Typography sx={{ fontWeight: 700 }} variant="body2">
                  {title}
                </Typography>
                <Chip
                  color={
                    worker.phase === 'waitingForOtherQueue'
                      ? 'warning'
                      : worker.running
                        ? 'info'
                        : snapshot.settings.enabled
                          ? 'success'
                          : 'default'
                  }
                  icon={
                    worker.running &&
                    worker.phase !== 'waitingForOtherQueue' ? (
                      <CircularProgress color="inherit" size={13} />
                    ) : undefined
                  }
                  label={t(
                    worker.phase === 'waitingForOtherQueue'
                      ? 'node-benchmark.task.queueBlocked'
                      : worker.running
                        ? 'node-benchmark.task.queueRunning'
                        : snapshot.settings.enabled
                          ? 'node-benchmark.task.queueWaiting'
                          : 'node-benchmark.task.queuePaused',
                  )}
                  size="small"
                  variant="outlined"
                />
              </Stack>
              {worker.running ? (
                <Typography
                  color="text.secondary"
                  sx={{ display: 'block', mt: 0.5 }}
                  variant="caption"
                >
                  {t('node-benchmark.task.running', {
                    phase: phaseLabels[worker.phase ?? ''] ?? worker.phase,
                    completed: worker.completed,
                    total: worker.total,
                  })}
                </Typography>
              ) : (
                <Typography
                  color="text.secondary"
                  sx={{ display: 'block', mt: 0.5 }}
                  variant="caption"
                >
                  {nextAt == null
                    ? t('node-benchmark.task.noScheduledNode')
                    : snapshot.settings.enabled &&
                        nextAt <= snapshot.generatedAt
                      ? t('node-benchmark.task.dueNow')
                      : t('node-benchmark.task.nextAt', {
                          time: formatTimestamp(nextAt),
                        })}
                </Typography>
              )}
              {worker.lastError && (
                <Typography
                  color="error"
                  sx={{ display: 'block', mt: 0.5, overflowWrap: 'anywhere' }}
                  variant="caption"
                >
                  {t('node-benchmark.task.lastQueueError', {
                    error: worker.lastError,
                  })}
                </Typography>
              )}
            </Paper>
          ))}
        </Box>

        <CollapsibleSection
          defaultExpanded={!snapshot.selectionConfirmed}
          id="benchmark-settings"
          subtitle={t('node-benchmark.settings.stepOneHelp')}
          summary={
            <Chip
              label={t('node-benchmark.settings.profileSummary', {
                selected: draft.selectedProfileUids.length,
                total: availableProfileCount,
              })}
              size="small"
              variant="outlined"
            />
          }
          title={t('node-benchmark.settings.stepOneTitle')}
        >
          <Stack sx={{ gap: 2 }}>
            <Box
              sx={{
                display: 'grid',
                gap: 1.5,
                gridTemplateColumns:
                  'repeat(auto-fit, minmax(min(100%, 230px), 1fr))',
              }}
            >
              {snapshot.profiles.map((profile) => {
                const selected = draft.selectedProfileUids.includes(profile.uid)
                return (
                  <Paper key={profile.uid} sx={{ p: 1.5 }} variant="outlined">
                    <Stack
                      direction="row"
                      sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
                    >
                      <Checkbox
                        checked={selected}
                        onChange={(_, checked) =>
                          handleProfileSelection(profile.uid, checked)
                        }
                      />
                      <Box sx={{ flex: 1, minWidth: 0 }}>
                        <Typography
                          sx={{ fontWeight: 600, overflowWrap: 'anywhere' }}
                        >
                          {profile.name}
                        </Typography>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.settings.nodeCount', {
                            count: profile.nodeCount,
                          })}{' '}
                          · {formatTimestamp(profile.lastScannedAt)}
                        </Typography>
                        {!profile.available && (
                          <Typography color="error" variant="caption">
                            {t('node-benchmark.settings.profileUnavailable')}
                          </Typography>
                        )}
                      </Box>
                      <TextField
                        disabled={!selected}
                        slotProps={{
                          htmlInput: { min: 0.1, max: 100, step: 0.1 },
                        }}
                        label={t('node-benchmark.settings.multiplier')}
                        onChange={(event) =>
                          patchDraft({
                            profileTrafficMultipliers: {
                              ...draft.profileTrafficMultipliers,
                              [profile.uid]: Number(event.target.value) || 1,
                            },
                          })
                        }
                        size="small"
                        sx={{
                          ml: { xs: 6, sm: 0 },
                          width: { xs: 'calc(100% - 48px)', sm: 105 },
                        }}
                        type="number"
                        value={
                          draft.profileTrafficMultipliers[profile.uid] ?? 1
                        }
                      />
                    </Stack>
                    {profile.available && profile.scanError && (
                      <Typography
                        color="error"
                        sx={{ display: 'block' }}
                        variant="caption"
                      >
                        {profile.scanError}
                      </Typography>
                    )}
                  </Paper>
                )
              })}
            </Box>

            <Box
              sx={{
                display: 'grid',
                gap: 1.5,
                gridTemplateColumns:
                  'repeat(auto-fit, minmax(min(100%, 220px), 1fr))',
              }}
            >
              <TextField
                helperText={t('node-benchmark.settings.includeHelp')}
                label={t('node-benchmark.settings.include')}
                onChange={(event) =>
                  handleFilterChange({ includeKeywords: event.target.value })
                }
                value={draft.includeKeywords}
              />
              <TextField
                helperText={t('node-benchmark.settings.excludeHelp')}
                label={t('node-benchmark.settings.exclude')}
                onChange={(event) =>
                  handleFilterChange({ excludeKeywords: event.target.value })
                }
                value={draft.excludeKeywords}
              />
            </Box>
            <Stack
              direction="row"
              sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 2 }}
            >
              <FormControlLabel
                control={
                  <Checkbox
                    checked={draft.caseSensitive}
                    onChange={(_, caseSensitive) =>
                      handleFilterChange({ caseSensitive })
                    }
                  />
                }
                label={t('node-benchmark.settings.caseSensitive')}
              />
              <Chip
                label={t('node-benchmark.settings.preview', filterPreview)}
                variant="outlined"
              />
            </Stack>

            <Divider />
            <Box
              sx={{
                display: 'grid',
                gap: 1.5,
                gridTemplateColumns:
                  'repeat(auto-fit, minmax(min(100%, 180px), 1fr))',
              }}
            >
              <TextField
                label={t('node-benchmark.settings.latencyInterval')}
                onChange={(event) =>
                  patchDraft({
                    latencyIntervalMinutes: Number(event.target.value),
                  })
                }
                select
                value={draft.latencyIntervalMinutes}
              >
                {LATENCY_INTERVALS.map((value) => (
                  <MenuItem key={value} value={value}>
                    {t('node-benchmark.settings.frequencyMinutes', { value })}
                  </MenuItem>
                ))}
              </TextField>
              <TextField
                label={t('node-benchmark.settings.speedInterval')}
                onChange={(event) =>
                  patchDraft({ speedIntervalHours: Number(event.target.value) })
                }
                select
                value={draft.speedIntervalHours}
              >
                {SPEED_INTERVALS.map((value) => (
                  <MenuItem key={value} value={value}>
                    {t('node-benchmark.settings.frequencyHours', { value })}
                  </MenuItem>
                ))}
              </TextField>
              <TextField
                slotProps={{ htmlInput: { min: 0, step: 0.5 } }}
                label={t('node-benchmark.settings.downloadLimit')}
                onChange={(event) =>
                  patchDraft({
                    downloadDailyLimitBytes: Number(event.target.value) * GIB,
                  })
                }
                type="number"
                value={(draft.downloadDailyLimitBytes / GIB).toFixed(1)}
              />
              <TextField
                slotProps={{ htmlInput: { min: 0, step: 0.5 } }}
                label={t('node-benchmark.settings.uploadLimit')}
                onChange={(event) =>
                  patchDraft({
                    uploadDailyLimitBytes: Number(event.target.value) * GIB,
                  })
                }
                type="number"
                value={(draft.uploadDailyLimitBytes / GIB).toFixed(1)}
              />
              <TextField
                slotProps={{ htmlInput: { min: 3, max: 30, step: 1 } }}
                label={t('node-benchmark.settings.duration')}
                onChange={(event) =>
                  patchDraft({
                    speedDurationMs: Number(event.target.value) * 1000,
                  })
                }
                type="number"
                value={draft.speedDurationMs / 1000}
              />
              <TextField
                slotProps={{ htmlInput: { min: 8, max: 2048, step: 8 } }}
                label={t('node-benchmark.settings.maxBytes')}
                onChange={(event) =>
                  patchDraft({
                    speedMaxBytes: Number(event.target.value) * MIB,
                  })
                }
                type="number"
                value={Math.round(draft.speedMaxBytes / MIB)}
              />
            </Box>
            <Box
              sx={{
                display: 'grid',
                gap: 1.5,
                gridTemplateColumns:
                  'repeat(auto-fit, minmax(min(100%, 260px), 1fr))',
              }}
            >
              <TextField
                label={t('node-benchmark.settings.latencyUrl')}
                onChange={(event) =>
                  patchDraft({ latencyUrl: event.target.value })
                }
                value={draft.latencyUrl}
              />
              <TextField
                label={t('node-benchmark.settings.downloadUrl')}
                onChange={(event) =>
                  patchDraft({ downloadUrl: event.target.value })
                }
                value={draft.downloadUrl}
              />
              <TextField
                label={t('node-benchmark.settings.uploadUrl')}
                onChange={(event) =>
                  patchDraft({ uploadUrl: event.target.value })
                }
                value={draft.uploadUrl}
              />
            </Box>
            {overLimit && (
              <Alert severity="warning">
                {t('node-benchmark.notices.estimatedOverLimit')}
              </Alert>
            )}
            <Stack
              direction="row"
              sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
            >
              <Typography
                color="text.secondary"
                sx={{ flex: 1 }}
                variant="caption"
              >
                {t('node-benchmark.settings.automaticPreview')}
              </Typography>
              <Chip
                color={previewLoading ? 'info' : 'default'}
                icon={
                  previewLoading ? <CircularProgress size={14} /> : undefined
                }
                label={t(
                  previewLoading
                    ? 'node-benchmark.candidates.loading'
                    : 'node-benchmark.candidates.loaded',
                  { total: candidateRows.length },
                )}
                variant="outlined"
              />
            </Stack>
          </Stack>
        </CollapsibleSection>

        <CollapsibleSection
          defaultExpanded={!snapshot.selectionConfirmed}
          id="benchmark-candidates"
          subtitle={t('node-benchmark.candidates.help')}
          summary={
            <Chip
              color={candidateNeedsSave ? 'warning' : 'success'}
              icon={candidateNeedsSave ? undefined : <CheckCircleOutlined />}
              label={t(
                candidateNeedsSave
                  ? 'node-benchmark.candidates.pending'
                  : 'node-benchmark.candidates.confirmed',
                {
                  selected: candidateSelectedCount,
                  total: candidateRows.length,
                },
              )}
              size="small"
              sx={{
                maxWidth: { xs: 150, sm: 280 },
                '& .MuiChip-label': {
                  overflow: 'hidden',
                  textOverflow: 'ellipsis',
                },
              }}
              variant={candidateNeedsSave ? 'filled' : 'outlined'}
            />
          }
          title={t('node-benchmark.candidates.title')}
        >
          <Stack sx={{ gap: 1.5 }}>
            {!candidateScopeMatches && (
              <Alert severity={previewLoading ? 'info' : 'warning'}>
                {t(
                  previewLoading
                    ? 'node-benchmark.candidates.loading'
                    : 'node-benchmark.candidates.scopeChanged',
                )}
              </Alert>
            )}
            {candidateScopeMatches && candidateRows.length === 0 && (
              <Alert severity="info">
                {t('node-benchmark.candidates.empty')}
              </Alert>
            )}

            {candidateRows.length > 0 && (
              <>
                <Stack
                  direction="row"
                  sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
                >
                  <TextField
                    label={t('node-benchmark.candidates.search')}
                    onChange={(event) => setCandidateSearch(event.target.value)}
                    size="small"
                    sx={{
                      flex: { xs: '1 1 100%', sm: '0 1 280px' },
                      minWidth: 0,
                    }}
                    value={candidateSearch}
                  />
                  <Button
                    disabled={!candidateScopeMatches}
                    onClick={() => {
                      setCandidateNodeIds(
                        new Set(
                          candidateRows
                            .filter((node) =>
                              nodeMatchesBenchmarkFilters(node, draft),
                            )
                            .map((node) => node.nodeId),
                        ),
                      )
                      markCandidateDirty()
                    }}
                    size="small"
                  >
                    {t('node-benchmark.actions.useKeywordSelection')}
                  </Button>
                  <Button
                    disabled={!candidateScopeMatches}
                    onClick={() => {
                      setCandidateNodeIds(
                        new Set(
                          visibleCandidateRows.map((node) => node.nodeId),
                        ),
                      )
                      markCandidateDirty()
                    }}
                    size="small"
                  >
                    {t('node-benchmark.actions.selectAllCandidates')}
                  </Button>
                  <Button
                    disabled={!candidateScopeMatches}
                    onClick={() => {
                      setCandidateNodeIds(new Set())
                      markCandidateDirty()
                    }}
                    size="small"
                  >
                    {t('node-benchmark.actions.clearCandidates')}
                  </Button>
                </Stack>

                {visibleCandidateRows.length === 0 && (
                  <Alert severity="info">
                    {t('node-benchmark.candidates.filterEmpty')}
                  </Alert>
                )}

                <TableContainer sx={{ maxHeight: 400, maxWidth: '100%' }}>
                  <Table size="small" stickyHeader sx={{ minWidth: 620 }}>
                    <TableHead>
                      <TableRow>
                        <TableCell padding="checkbox" />
                        <TableCell>
                          {t('node-benchmark.candidates.profile')}
                        </TableCell>
                        <TableCell>
                          {t('node-benchmark.candidates.node')}
                        </TableCell>
                        <TableCell>
                          {t('node-benchmark.candidates.type')}
                        </TableCell>
                      </TableRow>
                    </TableHead>
                    <TableBody>
                      {visibleCandidateRows.map((node) => (
                        <TableRow hover key={node.nodeId}>
                          <TableCell padding="checkbox">
                            <Checkbox
                              checked={candidateNodeIds.has(node.nodeId)}
                              disabled={!candidateScopeMatches}
                              slotProps={{
                                input: {
                                  'aria-label': `${node.profileName} ${node.nodeName}`,
                                },
                              }}
                              onChange={(_, checked) => {
                                setCandidateNodeIds((current) => {
                                  const next = new Set(current)
                                  if (checked) next.add(node.nodeId)
                                  else next.delete(node.nodeId)
                                  return next
                                })
                                markCandidateDirty()
                              }}
                            />
                          </TableCell>
                          <TableCell>{node.profileName}</TableCell>
                          <TableCell>
                            <Typography
                              sx={{ fontWeight: 600 }}
                              variant="body2"
                            >
                              {node.nodeName}
                            </Typography>
                          </TableCell>
                          <TableCell>{node.nodeType}</TableCell>
                        </TableRow>
                      ))}
                    </TableBody>
                  </Table>
                </TableContainer>

                <Stack
                  direction="row"
                  sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
                >
                  <Typography
                    color="text.secondary"
                    sx={{ flex: 1 }}
                    variant="caption"
                  >
                    {t('node-benchmark.candidates.confirmHelp')}
                  </Typography>
                  <Button
                    disabled={
                      busy ||
                      previewLoading ||
                      !candidateScopeMatches ||
                      candidateRows.length === 0 ||
                      candidateSelectedCount === 0 ||
                      !candidateNeedsSave
                    }
                    onClick={handleSave}
                    startIcon={
                      busy ? <CircularProgress size={16} /> : <SaveOutlined />
                    }
                    sx={{ width: { xs: '100%', sm: 'auto' } }}
                    variant="contained"
                  >
                    {t('node-benchmark.actions.saveSelection')}
                  </Button>
                </Stack>
              </>
            )}
          </Stack>
        </CollapsibleSection>

        <CollapsibleSection
          defaultExpanded
          id="benchmark-overview"
          subtitle={t('node-benchmark.windows.help')}
          summary={
            <Chip
              label={t('node-benchmark.windows.summary', {
                champion:
                  snapshot.champions.overall?.nodeName ??
                  t('node-benchmark.champions.waiting'),
                window: t(`node-benchmark.windows.${windowValue}`),
              })}
              size="small"
              variant="outlined"
            />
          }
          title={t('node-benchmark.sections.overview')}
        >
          <Stack
            direction="row"
            sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
          >
            <ToggleButtonGroup
              exclusive
              onChange={(_, value: BenchmarkWindow | null) => {
                if (value) setWindowValue(value)
              }}
              size="small"
              sx={{
                maxWidth: '100%',
                overflowX: 'auto',
                '& .MuiToggleButton-root': { flexShrink: 0 },
              }}
              value={windowValue}
            >
              {WINDOW_OPTIONS.map((value) => (
                <ToggleButton key={value} value={value}>
                  {t(`node-benchmark.windows.${value}`)}
                </ToggleButton>
              ))}
            </ToggleButtonGroup>
          </Stack>

          <Box
            sx={{
              display: 'grid',
              gap: 1.5,
              gridTemplateColumns:
                'repeat(auto-fit, minmax(min(100%, 180px), 1fr))',
              mt: 1.5,
            }}
          >
            <ChampionCard
              champion={snapshot.champions.overall}
              title={t('node-benchmark.champions.overall')}
              value={(champion) =>
                t('node-benchmark.champions.score', { value: champion.value })
              }
            />
            <ChampionCard
              champion={snapshot.champions.download}
              title={t('node-benchmark.champions.download')}
              value={(champion) => formatSpeed(champion.value)}
            />
            <ChampionCard
              champion={snapshot.champions.upload}
              title={t('node-benchmark.champions.upload')}
              value={(champion) => formatSpeed(champion.value)}
            />
            <ChampionCard
              champion={snapshot.champions.latency}
              title={t('node-benchmark.champions.latency')}
              value={(champion) => formatLatency(champion.value)}
            />
          </Box>
        </CollapsibleSection>

        <CollapsibleSection
          id="benchmark-traffic"
          subtitle={t('node-benchmark.traffic.help')}
          summary={
            <Chip
              label={t('node-benchmark.traffic.summary', {
                download: formatBytes(snapshot.traffic.actualDownloadBytes),
                upload: formatBytes(snapshot.traffic.actualUploadBytes),
              })}
              size="small"
              variant="outlined"
            />
          }
          title={t('node-benchmark.traffic.title')}
        >
          <Box
            sx={{
              display: 'grid',
              gap: 1.5,
              gridTemplateColumns:
                'repeat(auto-fit, minmax(min(100%, 180px), 1fr))',
              my: 1.5,
            }}
          >
            {(
              [
                [
                  'estimated',
                  snapshot.traffic.estimatedDownloadBytes,
                  snapshot.traffic.estimatedUploadBytes,
                ],
                [
                  'actual',
                  snapshot.traffic.actualDownloadBytes,
                  snapshot.traffic.actualUploadBytes,
                ],
                [
                  'limit',
                  snapshot.traffic.downloadLimitBytes,
                  snapshot.traffic.uploadLimitBytes,
                ],
              ] as const
            ).map(([label, download, upload]) => (
              <Box key={label}>
                <Typography color="text.secondary" variant="caption">
                  {t(`node-benchmark.traffic.${label}`)}
                </Typography>
                <Typography sx={{ fontWeight: 700 }}>
                  ↓ {formatBytes(download)} · ↑ {formatBytes(upload)}
                </Typography>
              </Box>
            ))}
          </Box>
          <TableContainer sx={{ maxWidth: '100%' }}>
            <Table size="small" sx={{ minWidth: 720 }}>
              <TableHead>
                <TableRow>
                  <TableCell>{t('node-benchmark.traffic.source')}</TableCell>
                  <TableCell>{t('node-benchmark.traffic.estimated')}</TableCell>
                  <TableCell>{t('node-benchmark.traffic.actual')}</TableCell>
                  <TableCell>{t('node-benchmark.traffic.billed')}</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {snapshot.traffic.profiles.map((profile) => (
                  <TableRow key={profile.profileUid}>
                    <TableCell>
                      {profile.profileName} · ×{profile.multiplier.toFixed(1)}
                    </TableCell>
                    <TableCell>
                      ↓ {formatBytes(profile.estimatedDownloadBytes)} · ↑{' '}
                      {formatBytes(profile.estimatedUploadBytes)}
                    </TableCell>
                    <TableCell>
                      ↓ {formatBytes(profile.actualDownloadBytes)} · ↑{' '}
                      {formatBytes(profile.actualUploadBytes)}
                    </TableCell>
                    <TableCell>
                      {formatBytes(profile.estimatedBilledBytes)} /{' '}
                      {formatBytes(profile.actualBilledBytes)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        </CollapsibleSection>

        {snapshot.reminders.length > 0 && (
          <CollapsibleSection
            defaultExpanded
            id="benchmark-reminders"
            summary={
              <Chip
                color="warning"
                label={t('node-benchmark.reminders.countSummary', {
                  count: snapshot.reminders.length,
                })}
                size="small"
                variant="outlined"
              />
            }
            title={t('node-benchmark.reminders.title')}
          >
            <Stack sx={{ gap: 1.5 }}>
              {snapshot.reminders.map((reminder) => {
                const row = snapshot.nodes.find(
                  (node) => node.nodeId === reminder.nodeId,
                )
                return (
                  <Alert key={reminder.nodeId} severity="warning">
                    <Typography sx={{ fontWeight: 700 }}>
                      {reminder.nodeName} · {reminder.profileName}
                    </Typography>
                    <Typography variant="body2">
                      {t('node-benchmark.reminders.summary', {
                        rank: reminder.latencyRank,
                        total: reminder.totalNodes,
                        median: formatLatency(reminder.medianLatency),
                        reference: formatLatency(reminder.referenceLatency),
                      })}
                    </Typography>
                    <Typography variant="body2">
                      {t('node-benchmark.reminders.speed', {
                        download: reminder.downloadRank ?? '—',
                        upload: reminder.uploadRank ?? '—',
                        stability:
                          reminder.stabilityPercent == null
                            ? '—'
                            : `${reminder.stabilityPercent.toFixed(1)}%`,
                      })}
                    </Typography>
                    <Typography variant="body2">
                      {t('node-benchmark.reminders.saving', {
                        download: formatBytes(
                          reminder.savedDownloadBytesPerDay,
                        ),
                        upload: formatBytes(reminder.savedUploadBytesPerDay),
                      })}
                    </Typography>
                    <Stack
                      direction="row"
                      sx={{ mt: 1, flexWrap: 'wrap', gap: 1 }}
                    >
                      {row && (
                        <>
                          <Button
                            color="error"
                            onClick={() => handleState(row, 'excluded')}
                            size="small"
                          >
                            {t('node-benchmark.actions.exclude')}
                          </Button>
                          <Button
                            onClick={() => handleState(row, 'observe')}
                            size="small"
                          >
                            {t('node-benchmark.actions.observe')}
                          </Button>
                        </>
                      )}
                      <Button
                        onClick={() =>
                          handleReminder(reminder.nodeId, 'snoozeSevenDays')
                        }
                        size="small"
                      >
                        {t('node-benchmark.actions.snooze')}
                      </Button>
                      <Button
                        onClick={() =>
                          handleReminder(reminder.nodeId, 'ignore')
                        }
                        size="small"
                      >
                        {t('node-benchmark.actions.ignore')}
                      </Button>
                    </Stack>
                  </Alert>
                )
              })}
            </Stack>
          </CollapsibleSection>
        )}

        {snapshot.changes.length > 0 && (
          <CollapsibleSection
            id="benchmark-changes"
            summary={
              <Chip
                label={t('node-benchmark.changes.countSummary', {
                  count: snapshot.changes.length,
                })}
                size="small"
                variant="outlined"
              />
            }
            title={t('node-benchmark.changes.title')}
          >
            <Stack direction="row" sx={{ flexWrap: 'wrap', gap: 1 }}>
              {snapshot.changes.slice(0, 12).map((change) => (
                <Tooltip
                  key={change.id}
                  title={formatTimestamp(change.createdAt)}
                >
                  <Chip
                    label={`${change.profileName} · ${change.nodeName ?? ''} · ${t(`node-benchmark.changes.${change.changeType as 'added'}`)}`}
                    size="small"
                    sx={{
                      maxWidth: '100%',
                      '& .MuiChip-label': {
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                      },
                    }}
                    variant="outlined"
                  />
                </Tooltip>
              ))}
            </Stack>
          </CollapsibleSection>
        )}

        <CollapsibleSection
          defaultExpanded
          id="benchmark-leaderboard"
          summary={
            <Chip
              label={t('node-benchmark.table.summary', {
                participating: participatingNodeCount,
                shown: filteredNodes.length,
                total: snapshot.nodes.length,
              })}
              size="small"
              variant="outlined"
            />
          }
          title={t('node-benchmark.table.title')}
        >
          <Stack
            direction="row"
            sx={{ alignItems: 'center', flexWrap: 'wrap', gap: 1 }}
          >
            {selectedNodes.size > 0 && (
              <>
                <Button
                  onClick={handleRetest}
                  size="small"
                  startIcon={<RestartAltOutlined />}
                  variant="contained"
                >
                  {t('node-benchmark.actions.retest')} ({selectedNodes.size})
                </Button>
                <Button
                  onClick={() => setSelectedNodes(new Set())}
                  size="small"
                >
                  {t('node-benchmark.actions.clearSelection')}
                </Button>
              </>
            )}
            <TextField
              label={t('node-benchmark.table.profile')}
              onChange={(event) => setSourceFilter(event.target.value)}
              select
              size="small"
              sx={{ flex: '1 1 160px', minWidth: 0 }}
              value={sourceFilter}
            >
              <MenuItem value="all">
                {t('node-benchmark.table.allSources')}
              </MenuItem>
              {snapshot.profiles
                .filter((profile) => profile.selected)
                .map((profile) => (
                  <MenuItem key={profile.uid} value={profile.uid}>
                    {profile.name}
                  </MenuItem>
                ))}
            </TextField>
            <TextField
              label={t('node-benchmark.table.stateFilter')}
              onChange={(event) =>
                setStateFilter(event.target.value as ParticipationFilter)
              }
              select
              size="small"
              sx={{ flex: '1 1 150px', minWidth: 0 }}
              value={stateFilter}
            >
              <MenuItem value="participating">
                {t('node-benchmark.states.participating')}
              </MenuItem>
              <MenuItem value="observe">
                {t('node-benchmark.states.observe')}
              </MenuItem>
              <MenuItem value="excluded">
                {t('node-benchmark.states.excluded')}
              </MenuItem>
              <MenuItem value="changed">
                {t('node-benchmark.states.changed')}
              </MenuItem>
              <MenuItem value="removed">
                {t('node-benchmark.states.removed')}
              </MenuItem>
              <MenuItem value="all">
                {t('node-benchmark.table.allStates')}
              </MenuItem>
            </TextField>
            <TextField
              label={t('node-benchmark.table.sort')}
              onChange={(event) =>
                setSortKey(event.target.value as NodeSortKey)
              }
              select
              size="small"
              sx={{ flex: '1 1 160px', minWidth: 0 }}
              value={sortKey}
            >
              {(
                ['overall', 'download', 'upload', 'latency', 'profile'] as const
              ).map((value) => (
                <MenuItem key={value} value={value}>
                  {t(`node-benchmark.table.sortOptions.${value}`)}
                </MenuItem>
              ))}
            </TextField>
            <FormControlLabel
              control={
                <Checkbox
                  checked={groupBySource}
                  onChange={(_, checked) => setGroupBySource(checked)}
                />
              }
              label={t('node-benchmark.table.groupBySource')}
              sx={{ flex: '1 1 auto', m: 0, minHeight: 40 }}
            />
            <TextField
              label={t('node-benchmark.table.search')}
              onChange={(event) => setSearch(event.target.value)}
              size="small"
              sx={{ flex: '1 1 220px', minWidth: 0 }}
              value={search}
            />
            <Tooltip
              title={
                snapshot.settings.enabled
                  ? t('node-benchmark.history.stopFirst')
                  : ''
              }
            >
              <Box
                component="span"
                sx={{ flex: { xs: '1 1 100%', sm: '0 0 auto' } }}
              >
                <Button
                  color="error"
                  disabled={
                    snapshot.settings.enabled ||
                    busy ||
                    controlBusy ||
                    clearHistoryBusy
                  }
                  fullWidth
                  onClick={() => setClearHistoryOpen(true)}
                  size="small"
                  startIcon={<DeleteSweepOutlined />}
                  sx={{ minHeight: 44 }}
                  variant="outlined"
                >
                  {t('node-benchmark.actions.clearHistory')}
                </Button>
              </Box>
            </Tooltip>
          </Stack>
          {filteredNodes.length === 0 ? (
            <Alert severity="info" sx={{ mt: 2 }}>
              {t('node-benchmark.empty')}
            </Alert>
          ) : compactLeaderboard ? (
            <Stack sx={{ gap: 1, mt: 1.5 }}>
              {filteredNodes.map((row) => (
                <Card key={row.nodeId} variant="outlined">
                  <CardContent sx={{ p: 1.5, '&:last-child': { pb: 1.5 } }}>
                    <Stack
                      direction="row"
                      sx={{ alignItems: 'flex-start', gap: 1 }}
                    >
                      <Checkbox
                        checked={selectedNodes.has(row.nodeId)}
                        slotProps={{
                          input: {
                            'aria-label': `${row.profileName} ${row.nodeName}`,
                          },
                        }}
                        onChange={(_, checked) =>
                          setSelectedNodes((current) => {
                            const next = new Set(current)
                            if (checked) next.add(row.nodeId)
                            else next.delete(row.nodeId)
                            return next
                          })
                        }
                      />
                      <Box sx={{ flex: 1, minWidth: 0, pt: 0.5 }}>
                        <Typography
                          sx={{ fontWeight: 700, overflowWrap: 'anywhere' }}
                          variant="body2"
                        >
                          {row.nodeName}
                        </Typography>
                        <Typography
                          color="text.secondary"
                          sx={{ display: 'block', overflowWrap: 'anywhere' }}
                          variant="caption"
                        >
                          {row.profileName} · {row.nodeType} ·{' '}
                          {row.fingerprint.slice(0, 8)}
                        </Typography>
                        <Typography color="text.secondary" variant="caption">
                          {formatTimestamp(row.lastTestedAt)}
                        </Typography>
                      </Box>
                      <Chip
                        label={t(`node-benchmark.confidence.${row.confidence}`)}
                        size="small"
                        variant="outlined"
                      />
                    </Stack>

                    <Box
                      sx={{
                        display: 'grid',
                        gap: 1.5,
                        gridTemplateColumns:
                          'repeat(auto-fit, minmax(min(100%, 140px), 1fr))',
                        mt: 1.5,
                      }}
                    >
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.state')}
                        </Typography>
                        <NodeStateSelect
                          fullWidth
                          onChange={(state) => handleState(row, state)}
                          row={row}
                        />
                      </Box>
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.stability')}
                        </Typography>
                        <Typography sx={{ fontWeight: 700 }} variant="body2">
                          {row.stabilityPercent == null
                            ? '—'
                            : `${row.stabilityPercent.toFixed(1)}%`}
                        </Typography>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.stabilityDetail', {
                            successes: row.latencySuccesses,
                            attempts: row.latencyAttempts,
                          })}
                        </Typography>
                      </Box>
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.download')}
                        </Typography>
                        <MetricCell
                          kind="speed"
                          metric={row.download}
                          nextAt={row.nextDownloadAt}
                        />
                      </Box>
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.upload')}
                        </Typography>
                        <MetricCell
                          kind="speed"
                          metric={row.upload}
                          nextAt={row.nextUploadAt}
                        />
                      </Box>
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.latency')}
                        </Typography>
                        <MetricCell
                          kind="latency"
                          metric={row.latency}
                          nextAt={row.nextLatencyAt}
                        />
                      </Box>
                      <Box>
                        <Typography color="text.secondary" variant="caption">
                          {t('node-benchmark.table.score')}
                        </Typography>
                        <Stack
                          direction="row"
                          sx={{
                            alignItems: 'center',
                            flexWrap: 'wrap',
                            gap: 0.5,
                          }}
                        >
                          <Typography sx={{ fontWeight: 700 }}>
                            {row.totalRankScore ?? '—'}
                          </Typography>
                          {row.officialEligible && (
                            <Chip
                              color="success"
                              label={t('node-benchmark.champions.official')}
                              size="small"
                            />
                          )}
                        </Stack>
                      </Box>
                    </Box>
                  </CardContent>
                </Card>
              ))}
            </Stack>
          ) : (
            <TableContainer sx={{ mt: 1, maxHeight: '62vh', maxWidth: '100%' }}>
              <Table
                stickyHeader
                size="small"
                sx={{
                  minWidth: 1420,
                  '& th': { whiteSpace: 'nowrap' },
                  '& td': { verticalAlign: 'top' },
                }}
              >
                <TableHead>
                  <TableRow>
                    <TableCell padding="checkbox" />
                    <TableCell>{t('node-benchmark.table.profile')}</TableCell>
                    <TableCell>{t('node-benchmark.table.node')}</TableCell>
                    <TableCell>{t('node-benchmark.table.state')}</TableCell>
                    <TableCell>{t('node-benchmark.table.stability')}</TableCell>
                    <TableCell>{t('node-benchmark.table.download')}</TableCell>
                    <TableCell>{t('node-benchmark.table.upload')}</TableCell>
                    <TableCell>{t('node-benchmark.table.latency')}</TableCell>
                    <TableCell>{t('node-benchmark.table.score')}</TableCell>
                    <TableCell>
                      {t('node-benchmark.table.confidence')}
                    </TableCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {filteredNodes.map((row, index) => (
                    <Fragment key={row.nodeId}>
                      {groupBySource &&
                        (index === 0 ||
                          filteredNodes[index - 1].profileUid !==
                            row.profileUid) && (
                          <TableRow>
                            <TableCell colSpan={10} sx={{ fontWeight: 700 }}>
                              {row.profileName}
                            </TableCell>
                          </TableRow>
                        )}
                      <TableRow hover>
                        <TableCell padding="checkbox">
                          <Checkbox
                            checked={selectedNodes.has(row.nodeId)}
                            onChange={(_, checked) =>
                              setSelectedNodes((current) => {
                                const next = new Set(current)
                                if (checked) next.add(row.nodeId)
                                else next.delete(row.nodeId)
                                return next
                              })
                            }
                          />
                        </TableCell>
                        <TableCell>
                          <Typography
                            noWrap
                            sx={{ fontWeight: 600 }}
                            variant="body2"
                          >
                            {row.profileName}
                          </Typography>
                          <Typography color="text.secondary" variant="caption">
                            {row.profileUid}
                          </Typography>
                        </TableCell>
                        <TableCell>
                          <Typography
                            noWrap
                            sx={{ fontWeight: 600 }}
                            variant="body2"
                          >
                            {row.nodeName}
                          </Typography>
                          <Typography color="text.secondary" variant="caption">
                            {row.nodeType} · {row.fingerprint.slice(0, 8)} ·{' '}
                            {formatTimestamp(row.lastTestedAt)}
                          </Typography>
                        </TableCell>
                        <TableCell>
                          <NodeStateSelect
                            onChange={(state) => handleState(row, state)}
                            row={row}
                          />
                        </TableCell>
                        <TableCell>
                          <Typography sx={{ fontWeight: 700 }} variant="body2">
                            {row.stabilityPercent == null
                              ? '—'
                              : `${row.stabilityPercent.toFixed(1)}%`}
                          </Typography>
                          <Typography color="text.secondary" variant="caption">
                            {t('node-benchmark.table.stabilityDetail', {
                              successes: row.latencySuccesses,
                              attempts: row.latencyAttempts,
                            })}
                          </Typography>
                        </TableCell>
                        <TableCell>
                          <MetricCell
                            kind="speed"
                            metric={row.download}
                            nextAt={row.nextDownloadAt}
                          />
                        </TableCell>
                        <TableCell>
                          <MetricCell
                            kind="speed"
                            metric={row.upload}
                            nextAt={row.nextUploadAt}
                          />
                        </TableCell>
                        <TableCell>
                          <MetricCell
                            kind="latency"
                            metric={row.latency}
                            nextAt={row.nextLatencyAt}
                          />
                        </TableCell>
                        <TableCell>
                          <Typography sx={{ fontWeight: 700 }}>
                            {row.totalRankScore ?? '—'}
                          </Typography>
                          {row.officialEligible && (
                            <Chip
                              color="success"
                              label={t('node-benchmark.champions.official')}
                              size="small"
                            />
                          )}
                        </TableCell>
                        <TableCell>
                          <Chip
                            label={t(
                              `node-benchmark.confidence.${row.confidence}`,
                            )}
                            size="small"
                            variant="outlined"
                          />
                        </TableCell>
                      </TableRow>
                    </Fragment>
                  ))}
                </TableBody>
              </Table>
            </TableContainer>
          )}
        </CollapsibleSection>
      </Stack>
      <BaseDialog
        cancelBtn={t('node-benchmark.history.cancel')}
        disableCancel={clearHistoryBusy}
        disableOk={snapshot.settings.enabled}
        fullWidth
        loading={clearHistoryBusy}
        maxWidth="xs"
        okBtn={t('node-benchmark.history.confirm')}
        okBtnColor="error"
        onCancel={() => setClearHistoryOpen(false)}
        onClose={() => {
          if (!clearHistoryBusy) setClearHistoryOpen(false)
        }}
        onOk={handleClearHistory}
        open={clearHistoryOpen}
        title={t('node-benchmark.history.title')}
      >
        <Alert severity="warning" sx={{ mt: 0.5 }}>
          {t('node-benchmark.history.warning')}
        </Alert>
        <Typography color="text.secondary" sx={{ mt: 1.5 }} variant="body2">
          {t('node-benchmark.history.preserved')}
        </Typography>
      </BaseDialog>
    </BasePage>
  )
}
