import { ArrowDownwardRounded, ArrowUpwardRounded } from '@mui/icons-material'
import { Box, Tooltip, Typography } from '@mui/material'
import { useTranslation } from 'react-i18next'

import { formatProxySpeedTestSpeed } from '@/services/proxy-speed'

interface Props {
  download?: number
  upload?: number
}

/**
 * 将测速速度压缩为适合代理卡片展示的短文本。
 */
function formatCompactSpeed(bytesPerSecond: number) {
  if (bytesPerSecond >= 1024 ** 3) {
    return `${(bytesPerSecond / 1024 ** 3).toFixed(1)}G/s`
  }
  if (bytesPerSecond >= 1024 ** 2) {
    return `${(bytesPerSecond / 1024 ** 2).toFixed(1)}M/s`
  }
  if (bytesPerSecond >= 1024) {
    return `${(bytesPerSecond / 1024).toFixed(0)}K/s`
  }
  return `${bytesPerSecond.toFixed(0)}B/s`
}

/**
 * 紧凑展示节点最近一次成功的下载和上传测速速度。
 */
export function ProxySpeedSummary({ download, upload }: Props) {
  const { t } = useTranslation()
  const hasDownload = typeof download === 'number'
  const hasUpload = typeof upload === 'number'

  if (!hasDownload && !hasUpload) return null

  return (
    <Box
      sx={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-end',
        flexShrink: 0,
        gap: 0.125,
      }}
    >
      {hasDownload && (
        <Tooltip
          arrow
          title={`${t('proxies.page.speedTest.columns.speed')}: ${formatProxySpeedTestSpeed(download)}`}
        >
          <Typography
            component="span"
            aria-label={`${t('proxies.page.speedTest.columns.speed')}: ${formatProxySpeedTestSpeed(download)}`}
            sx={{
              display: 'flex',
              alignItems: 'center',
              color: 'primary.main',
              fontSize: 11,
              fontVariantNumeric: 'tabular-nums',
              lineHeight: 1.2,
              whiteSpace: 'nowrap',
            }}
          >
            <ArrowDownwardRounded sx={{ fontSize: 12 }} />
            {formatCompactSpeed(download)}
          </Typography>
        </Tooltip>
      )}
      {hasUpload && (
        <Tooltip
          arrow
          title={`${t('proxies.page.speedTest.columns.uploadSpeed')}: ${formatProxySpeedTestSpeed(upload)}`}
        >
          <Typography
            component="span"
            aria-label={`${t('proxies.page.speedTest.columns.uploadSpeed')}: ${formatProxySpeedTestSpeed(upload)}`}
            sx={{
              display: 'flex',
              alignItems: 'center',
              color: 'secondary.main',
              fontSize: 11,
              fontVariantNumeric: 'tabular-nums',
              lineHeight: 1.2,
              whiteSpace: 'nowrap',
            }}
          >
            <ArrowUpwardRounded sx={{ fontSize: 12 }} />
            {formatCompactSpeed(upload)}
          </Typography>
        </Tooltip>
      )}
    </Box>
  )
}
