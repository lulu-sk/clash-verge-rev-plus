import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  MenuItem,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  TextField,
  Typography,
} from '@mui/material'
import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { selectNodeForGroup } from 'tauri-plugin-mihomo-api'

import { BaseDialog } from '@/components/base'
import { useAppRefreshers, useProxiesData } from '@/providers/app-data-context'
import { showNotice } from '@/services/notice-service'
import {
  type ProxySpeedTestSortId,
  type ProxySpeedTestStoredRow,
  PROXY_SPEED_TEST_PRESETS,
  PROXY_SPEED_TEST_SOURCES,
  formatProxySpeedTestBytes,
  formatProxySpeedTestDuration,
  formatProxySpeedTestSpeed,
  getProxySpeedTestOptions,
  getStoredProxySpeedTestCachedRows,
  getStoredProxySpeedTestCustomUrl,
  getStoredProxySpeedTestPresetId,
  getStoredProxySpeedTestSortId,
  getStoredProxySpeedTestSourceId,
  resolveProxySpeedTestUrl,
  runProxySpeedTest,
  setStoredProxySpeedTestCachedRows,
  setStoredProxySpeedTestCustomUrl,
  setStoredProxySpeedTestPresetId,
  setStoredProxySpeedTestSortId,
  setStoredProxySpeedTestSourceId,
} from '@/services/proxy-speed'
import {
  resolveMember,
  type ProxyGroupView,
  type ProxyViewV1,
} from '@/types/proxy-view'

interface ProxySpeedTestRow extends ProxySpeedTestStoredRow {
  status: 'idle' | 'testing' | 'success' | 'failed'
}

interface Props {
  open: boolean
  group: ProxyGroupView | null
  onClose: () => void
}

type ProxySpeedTestStatus = ProxySpeedTestRow['status']

/**
 * 提取当前代理组中适合进行下载测速的叶子节点。
 */
function extractSpeedTestRows(
  group: ProxyGroupView | null,
  proxyView: ProxyViewV1 | undefined,
): ProxySpeedTestRow[] {
  if (!group || !proxyView) return []

  return group.members.flatMap((memberRef) => {
    const member = resolveMember(proxyView, memberRef)
    if (member.kind !== 'node') return []
    if (member.ref.name === 'DIRECT' || member.ref.name === 'REJECT') return []

    return [
      {
        name: member.ref.name,
        type: member.node.type,
        status: 'idle' as const,
      },
    ]
  })
}

/**
 * 统一提取可展示的错误文本。
 */
function getErrorMessage(error: unknown) {
  if (error instanceof Error && error.message) {
    return error.message
  }

  if (typeof error === 'string') {
    return error
  }

  if (error && typeof error === 'object' && 'message' in error) {
    const message = (error as { message?: unknown }).message
    if (typeof message === 'string') {
      return message
    }
  }

  return '未知错误'
}

/**
 * 将缓存结果与当前代理组节点合并，避免节点变化后展示过期脏数据。
 */
function mergeStoredRows(
  rows: ProxySpeedTestRow[],
  storedRows: ProxySpeedTestStoredRow[],
): ProxySpeedTestRow[] {
  const storedRowMap = new Map(storedRows.map((row) => [row.name, row]))

  return rows.map((row) => {
    const storedRow = storedRowMap.get(row.name)
    if (!storedRow) return row

    return {
      ...row,
      ...storedRow,
      status: storedRow.status === 'testing' ? 'idle' : storedRow.status,
    }
  })
}

/**
 * 判断当前结果集中是否已有可保留的测速结果。
 */
function hasPersistableRows(rows: ProxySpeedTestRow[]) {
  return rows.some((row) => row.status === 'success' || row.status === 'failed')
}

/**
 * 比较可选数值，始终将无结果的节点排到后面。
 */
function compareOptionalNumber(
  left: number | undefined,
  right: number | undefined,
  direction: 'asc' | 'desc',
) {
  const leftExists = left !== undefined
  const rightExists = right !== undefined

  if (!leftExists && !rightExists) return 0
  if (!leftExists) return 1
  if (!rightExists) return -1

  return direction === 'asc' ? left - right : right - left
}

/**
 * 按选定方式对测速结果做展示排序。
 */
function sortProxySpeedTestRows(
  rows: ProxySpeedTestRow[],
  sortId: ProxySpeedTestSortId,
) {
  if (sortId === 'default') {
    return rows
  }

  const nextRows = [...rows]
  nextRows.sort((left, right) => {
    switch (sortId) {
      case 'speedDesc':
        return compareOptionalNumber(
          left.result?.averageBytesPerSecond,
          right.result?.averageBytesPerSecond,
          'desc',
        )
      case 'speedAsc':
        return compareOptionalNumber(
          left.result?.averageBytesPerSecond,
          right.result?.averageBytesPerSecond,
          'asc',
        )
      case 'trafficDesc':
        return compareOptionalNumber(
          left.result?.bytesRead,
          right.result?.bytesRead,
          'desc',
        )
      case 'durationAsc':
        return compareOptionalNumber(
          left.result?.elapsedMs,
          right.result?.elapsedMs,
          'asc',
        )
      case 'nameAsc':
        return left.name.localeCompare(right.name, 'zh-Hans-CN')
      default:
        return 0
    }
  })

  return nextRows
}

/**
 * 提供代理组下载测速弹窗。
 */
export function ProxySpeedViewer({ open, group, onClose }: Props) {
  const { t } = useTranslation()
  const { refreshProxy } = useAppRefreshers()
  const { proxyView } = useProxiesData()
  const [sourceId, setSourceId] = useState(getStoredProxySpeedTestSourceId)
  const [presetId, setPresetId] = useState(getStoredProxySpeedTestPresetId)
  const [sortId, setSortId] = useState(getStoredProxySpeedTestSortId)
  const [customUrl, setCustomUrl] = useState(getStoredProxySpeedTestCustomUrl)
  const [rows, setRows] = useState<ProxySpeedTestRow[]>([])
  const [isRunning, setIsRunning] = useState(false)
  const [singleTestingName, setSingleTestingName] = useState<string | null>(
    null,
  )
  const stopRequestedRef = useRef(false)
  const runVersionRef = useRef(0)
  const rowsRef = useRef<ProxySpeedTestRow[]>([])

  const initialRows = useMemo(
    () => extractSpeedTestRows(group, proxyView),
    [group, proxyView],
  )
  const sourceOptions = useMemo(
    () =>
      PROXY_SPEED_TEST_SOURCES.map((source) => ({
        ...source,
        label:
          source.id === 'cloudflare'
            ? t('proxies.page.speedTest.sources.cloudflare.name')
            : source.id === 'bunny'
              ? t('proxies.page.speedTest.sources.bunny.name')
              : source.id === 'hetzner'
                ? t('proxies.page.speedTest.sources.hetzner.name')
                : t('proxies.page.speedTest.sources.custom.name'),
        description:
          source.id === 'cloudflare'
            ? t('proxies.page.speedTest.sources.cloudflare.description')
            : source.id === 'bunny'
              ? t('proxies.page.speedTest.sources.bunny.description')
              : source.id === 'hetzner'
                ? t('proxies.page.speedTest.sources.hetzner.description')
                : t('proxies.page.speedTest.sources.custom.description'),
      })),
    [t],
  )
  const presetOptions = useMemo(
    () =>
      PROXY_SPEED_TEST_PRESETS.map((preset) => ({
        ...preset,
        label:
          preset.id === 'quick'
            ? t('proxies.page.speedTest.presets.quick.name')
            : preset.id === 'balanced'
              ? t('proxies.page.speedTest.presets.balanced.name')
              : t('proxies.page.speedTest.presets.extended.name'),
        description:
          preset.id === 'quick'
            ? t('proxies.page.speedTest.presets.quick.description')
            : preset.id === 'balanced'
              ? t('proxies.page.speedTest.presets.balanced.description')
              : t('proxies.page.speedTest.presets.extended.description'),
      })),
    [t],
  )
  const resolvedUrl = useMemo(
    () => resolveProxySpeedTestUrl(sourceId, customUrl),
    [sourceId, customUrl],
  )
  const sortOptions = useMemo(
    () => [
      {
        id: 'default' as const,
        label: t('proxies.page.speedTest.sorts.default'),
      },
      {
        id: 'speedDesc' as const,
        label: t('proxies.page.speedTest.sorts.speedDesc'),
      },
      {
        id: 'speedAsc' as const,
        label: t('proxies.page.speedTest.sorts.speedAsc'),
      },
      {
        id: 'trafficDesc' as const,
        label: t('proxies.page.speedTest.sorts.trafficDesc'),
      },
      {
        id: 'durationAsc' as const,
        label: t('proxies.page.speedTest.sorts.durationAsc'),
      },
      {
        id: 'nameAsc' as const,
        label: t('proxies.page.speedTest.sorts.nameAsc'),
      },
    ],
    [t],
  )
  const selectedSource = useMemo(
    () =>
      sourceOptions.find((source) => source.id === sourceId) ||
      sourceOptions[0],
    [sourceId, sourceOptions],
  )
  const selectedPreset = useMemo(
    () =>
      presetOptions.find((preset) => preset.id === presetId) ||
      presetOptions[0],
    [presetId, presetOptions],
  )
  const completedCount = useMemo(
    () =>
      rows.filter((row) => row.status === 'success' || row.status === 'failed')
        .length,
    [rows],
  )
  const displayRows = useMemo(
    () => sortProxySpeedTestRows(rows, sortId),
    [rows, sortId],
  )

  useEffect(() => {
    if (!open || isRunning) return
    stopRequestedRef.current = false
    const storedRows = group
      ? getStoredProxySpeedTestCachedRows(
          group.name,
          sourceId,
          presetId,
          resolvedUrl,
        )
      : []
    const nextRows = mergeStoredRows(initialRows, storedRows)
    rowsRef.current = nextRows
    // eslint-disable-next-line @eslint-react/set-state-in-effect -- 打开弹窗时需要用缓存结果同步表格初始状态
    setRows(nextRows)
  }, [group, initialRows, isRunning, open, presetId, resolvedUrl, sourceId])

  useEffect(() => {
    rowsRef.current = rows
  }, [rows])

  /**
   * 将当前可见结果持久化，避免测速结束后被刷新流程覆盖。
   */
  function persistRows(rowsToPersist: ProxySpeedTestRow[]) {
    if (!group || !resolvedUrl || !hasPersistableRows(rowsToPersist)) return

    setStoredProxySpeedTestCachedRows(
      group.name,
      sourceId,
      presetId,
      resolvedUrl,
      rowsToPersist,
    )
  }

  /**
   * 更新单个测速行的状态。
   */
  function updateRow(
    rowName: string,
    patch: Partial<ProxySpeedTestRow>,
    runVersion: number,
  ) {
    if (runVersionRef.current !== runVersion) return

    setRows((currentRows) => {
      const nextRows = currentRows.map((row) =>
        row.name === rowName ? { ...row, ...patch } : row,
      )
      rowsRef.current = nextRows
      return nextRows
    })
  }

  /**
   * 保存测速源选择。
   */
  function handleSourceChange(sourceValue: string) {
    setSourceId(sourceValue)
    setStoredProxySpeedTestSourceId(sourceValue)
  }

  /**
   * 保存测速档位选择。
   */
  function handlePresetChange(value: string) {
    setPresetId(value)
    setStoredProxySpeedTestPresetId(value)
  }

  /**
   * 保存测速结果排序方式。
   */
  function handleSortChange(value: ProxySpeedTestSortId) {
    setSortId(value)
    setStoredProxySpeedTestSortId(value)
  }

  /**
   * 保存自定义测速链接。
   */
  function handleCustomUrlChange(url: string) {
    setCustomUrl(url)
    setStoredProxySpeedTestCustomUrl(url)
  }

  /**
   * 判断当前是否有测速任务正在运行。
   */
  function isTesting() {
    return isRunning || !!singleTestingName
  }

  /**
   * 请求停止当前批量测速任务。
   */
  function handleStop() {
    stopRequestedRef.current = true
  }

  /**
   * 关闭测速弹窗。
   */
  function handleClose() {
    if (isTesting()) return
    onClose()
  }

  /**
   * 对指定节点执行下载测速。
   */
  async function testProxyRow(
    row: ProxySpeedTestRow,
    testUrl: string,
    runVersion: number,
  ) {
    if (!group) return

    updateRow(
      row.name,
      { status: 'testing', error: undefined, result: undefined },
      runVersion,
    )

    await selectNodeForGroup(group.name, row.name)
    await new Promise((resolve) => setTimeout(resolve, 400))

    try {
      const result = await runProxySpeedTest(
        getProxySpeedTestOptions(testUrl, presetId),
      )
      updateRow(
        row.name,
        {
          status: 'success',
          result,
        },
        runVersion,
      )
    } catch (error) {
      updateRow(
        row.name,
        {
          status: 'failed',
          error: getErrorMessage(error),
        },
        runVersion,
      )
    }
  }

  /**
   * 获取当前可用测速链接。
   */
  function getRunnableTestUrl() {
    const testUrl = resolvedUrl.trim()
    if (!testUrl) {
      showNotice.error(t('proxies.page.speedTest.messages.sourceRequired'))
      return null
    }

    return testUrl
  }

  /**
   * 执行单个节点下载测速。
   */
  async function handleRunSingle(row: ProxySpeedTestRow) {
    if (!group) return
    if (isTesting()) return

    const testUrl = getRunnableTestUrl()
    if (!testUrl) return

    const runVersion = runVersionRef.current + 1
    runVersionRef.current = runVersion
    stopRequestedRef.current = false
    setSingleTestingName(row.name)

    const originalProxy = group.now

    try {
      await testProxyRow(row, testUrl, runVersion)
    } finally {
      if (originalProxy) {
        try {
          await selectNodeForGroup(group.name, originalProxy)
        } catch (error) {
          showNotice.error(
            t('proxies.page.speedTest.messages.restoreFailed'),
            error,
          )
        }
      }

      persistRows(rowsRef.current)

      await refreshProxy().catch((error) => {
        console.error('[ProxySpeedViewer] 刷新代理数据失败:', error)
      })

      if (runVersionRef.current === runVersion) {
        setSingleTestingName(null)
      }
    }
  }

  /**
   * 执行当前代理组的串行下载测速。
   */
  async function handleRun() {
    if (!group) return
    if (singleTestingName) return

    const testUrl = getRunnableTestUrl()
    if (!testUrl) return

    if (initialRows.length === 0) {
      showNotice.error(t('proxies.page.speedTest.messages.empty'))
      return
    }

    const runVersion = runVersionRef.current + 1
    runVersionRef.current = runVersion
    stopRequestedRef.current = false
    setIsRunning(true)
    rowsRef.current = initialRows
    setRows(initialRows)

    const originalProxy = group.now

    try {
      for (const row of initialRows) {
        if (stopRequestedRef.current) break

        await testProxyRow(row, testUrl, runVersion)
      }
    } finally {
      if (originalProxy) {
        try {
          await selectNodeForGroup(group.name, originalProxy)
        } catch (error) {
          showNotice.error(
            t('proxies.page.speedTest.messages.restoreFailed'),
            error,
          )
        }
      }

      persistRows(rowsRef.current)

      await refreshProxy().catch((error) => {
        console.error('[ProxySpeedViewer] 刷新代理数据失败:', error)
      })

      if (stopRequestedRef.current) {
        showNotice.info(t('proxies.page.speedTest.messages.stopped'))
      }

      if (runVersionRef.current === runVersion) {
        setIsRunning(false)
      }
    }
  }

  /**
   * 渲染单行测速状态标签。
   */
  function renderStatus(row: ProxySpeedTestRow) {
    if (row.status === 'testing') {
      return (
        <Box sx={{ display: 'flex', alignItems: 'center', gap: 1 }}>
          <CircularProgress size={14} />
          <Typography variant="caption">
            {t('proxies.page.speedTest.statuses.testing')}
          </Typography>
        </Box>
      )
    }

    const chipColor: 'default' | 'success' | 'error' =
      row.status === 'success'
        ? 'success'
        : row.status === 'failed'
          ? 'error'
          : 'default'
    const statusLabels: Record<ProxySpeedTestStatus, string> = {
      idle: t('proxies.page.speedTest.statuses.idle'),
      testing: t('proxies.page.speedTest.statuses.testing'),
      success: t('proxies.page.speedTest.statuses.success'),
      failed: t('proxies.page.speedTest.statuses.failed'),
    }

    return (
      <Box sx={{ display: 'flex', flexDirection: 'column', gap: 0.5 }}>
        <Chip
          size="small"
          color={chipColor}
          label={statusLabels[row.status]}
          sx={{ width: 'fit-content' }}
        />
        {row.error && (
          <Typography
            variant="caption"
            color="error.main"
            title={row.error}
            sx={{
              maxWidth: 220,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {row.error}
          </Typography>
        )}
      </Box>
    )
  }

  return (
    <BaseDialog
      open={open}
      maxWidth={false}
      title={`${t('proxies.page.speedTest.title')} · ${group?.name || '-'}`}
      contentSx={{
        width: { xs: 'calc(100vw - 32px)', md: 980 },
        maxWidth: 'calc(100vw - 32px)',
        height: { xs: '72vh', md: '78vh' },
        maxHeight: 'calc(100vh - 140px)',
        display: 'flex',
        flexDirection: 'column',
        overflow: 'hidden',
        pb: 0,
      }}
      okBtn={
        isRunning
          ? t('proxies.page.speedTest.actions.stop')
          : t('proxies.page.speedTest.actions.start')
      }
      cancelBtn={t('shared.actions.close')}
      disableOk={rows.length === 0 || !!singleTestingName}
      disableCancel={isTesting()}
      onClose={handleClose}
      onCancel={handleClose}
      onOk={isRunning ? handleStop : handleRun}
    >
      <Box
        sx={{
          display: 'flex',
          flexDirection: 'column',
          gap: 2,
          flex: 1,
          minHeight: 0,
        }}
      >
        <Alert severity="info">{t('proxies.page.speedTest.hint')}</Alert>

        <Box
          sx={{
            display: 'grid',
            gridTemplateColumns: {
              xs: '1fr',
              md: 'repeat(3, minmax(0, 1fr))',
            },
            gap: 1.5,
          }}
        >
          <TextField
            select
            size="small"
            label={t('proxies.page.speedTest.fields.source')}
            value={sourceId}
            helperText={selectedSource?.description}
            slotProps={{
              select: {
                renderValue: (value: unknown) =>
                  sourceOptions.find((source) => source.id === String(value))
                    ?.label || String(value),
              },
            }}
            onChange={(event) => handleSourceChange(event.target.value)}
          >
            {sourceOptions.map((source) => (
              <MenuItem key={source.id} value={source.id}>
                <Box
                  sx={{
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'flex-start',
                    py: 0.25,
                  }}
                >
                  <Typography variant="body2">{source.label}</Typography>
                  <Typography
                    variant="caption"
                    color="text.secondary"
                    sx={{ whiteSpace: 'normal' }}
                  >
                    {source.description}
                  </Typography>
                </Box>
              </MenuItem>
            ))}
          </TextField>

          <TextField
            select
            size="small"
            label={t('proxies.page.speedTest.fields.preset')}
            value={presetId}
            helperText={selectedPreset?.description}
            slotProps={{
              select: {
                renderValue: (value: unknown) =>
                  presetOptions.find((preset) => preset.id === String(value))
                    ?.label || String(value),
              },
            }}
            onChange={(event) => handlePresetChange(event.target.value)}
          >
            {presetOptions.map((preset) => (
              <MenuItem key={preset.id} value={preset.id}>
                <Box
                  sx={{
                    display: 'flex',
                    flexDirection: 'column',
                    alignItems: 'flex-start',
                    py: 0.25,
                  }}
                >
                  <Typography variant="body2">{preset.label}</Typography>
                  <Typography
                    variant="caption"
                    color="text.secondary"
                    sx={{ whiteSpace: 'normal' }}
                  >
                    {preset.description}
                  </Typography>
                </Box>
              </MenuItem>
            ))}
          </TextField>

          <TextField
            select
            size="small"
            label={t('proxies.page.speedTest.fields.sort')}
            value={sortId}
            helperText={t('proxies.page.speedTest.fields.sortHint')}
            slotProps={{
              select: {
                renderValue: (value: unknown) =>
                  sortOptions.find((option) => option.id === String(value))
                    ?.label || String(value),
              },
            }}
            onChange={(event) =>
              handleSortChange(event.target.value as ProxySpeedTestSortId)
            }
          >
            {sortOptions.map((option) => (
              <MenuItem key={option.id} value={option.id}>
                {option.label}
              </MenuItem>
            ))}
          </TextField>

          {sourceId === 'custom' && (
            <TextField
              size="small"
              label={t('proxies.page.speedTest.fields.customUrl')}
              value={customUrl}
              placeholder="https://example.com/100MB.bin"
              helperText={t('proxies.page.speedTest.fields.customUrlHint')}
              sx={{ gridColumn: { md: '1 / -1' } }}
              onChange={(event) => handleCustomUrlChange(event.target.value)}
            />
          )}
        </Box>

        <Box
          sx={{
            display: 'flex',
            justifyContent: 'space-between',
            gap: 1,
            flexWrap: 'wrap',
            alignItems: 'center',
          }}
        >
          <Typography
            variant="body2"
            color="text.secondary"
            sx={{ flex: 1, minWidth: 240, wordBreak: 'break-all' }}
          >
            {resolvedUrl || '-'}
          </Typography>
          <Typography variant="body2" color="text.secondary">
            {t('proxies.page.speedTest.labels.presetSummary', {
              duration: formatProxySpeedTestDuration(selectedPreset.durationMs),
              traffic: formatProxySpeedTestBytes(selectedPreset.maxBytes),
            })}
          </Typography>
          <Typography variant="body2" color="text.secondary">
            {completedCount} / {rows.length}
          </Typography>
        </Box>

        {rows.length === 0 ? (
          <Alert severity="warning">
            {t('proxies.page.speedTest.messages.empty')}
          </Alert>
        ) : (
          <TableContainer
            sx={{
              flex: 1,
              minHeight: 0,
              border: '1px solid',
              borderColor: 'divider',
              borderRadius: 1,
            }}
          >
            <Table stickyHeader size="small">
              <TableHead>
                <TableRow>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.name')}
                  </TableCell>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.type')}
                  </TableCell>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.speed')}
                  </TableCell>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.traffic')}
                  </TableCell>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.duration')}
                  </TableCell>
                  <TableCell>
                    {t('proxies.page.speedTest.columns.status')}
                  </TableCell>
                  <TableCell align="right">
                    {t('proxies.page.speedTest.columns.action')}
                  </TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {displayRows.map((row) => (
                  <TableRow key={row.name} hover>
                    <TableCell sx={{ minWidth: 220 }}>{row.name}</TableCell>
                    <TableCell sx={{ whiteSpace: 'nowrap' }}>
                      {row.type}
                    </TableCell>
                    <TableCell>
                      {row.result
                        ? formatProxySpeedTestSpeed(
                            row.result.averageBytesPerSecond,
                          )
                        : '-'}
                    </TableCell>
                    <TableCell>
                      {row.result
                        ? formatProxySpeedTestBytes(row.result.bytesRead)
                        : '-'}
                    </TableCell>
                    <TableCell>
                      {row.result
                        ? formatProxySpeedTestDuration(row.result.elapsedMs)
                        : '-'}
                    </TableCell>
                    <TableCell sx={{ minWidth: 160 }}>
                      {renderStatus(row)}
                    </TableCell>
                    <TableCell align="right">
                      <Button
                        size="small"
                        variant="text"
                        disabled={isTesting()}
                        onClick={() => handleRunSingle(row)}
                      >
                        {row.result
                          ? t('proxies.page.speedTest.actions.retestOne')
                          : t('proxies.page.speedTest.actions.testOne')}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </Box>
    </BaseDialog>
  )
}
