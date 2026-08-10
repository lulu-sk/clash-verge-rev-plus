import { invoke } from '@tauri-apps/api/core'

import type {
  BenchmarkPreview,
  BenchmarkSaveRequest,
  BenchmarkSettings,
  BenchmarkSnapshot,
  BenchmarkWindow,
  ManualBatchRequest,
  ManualBatchSnapshot,
  NodeUpdateRequest,
} from './types'

export * from './types'

/** 读取指定时间窗口的完整评选快照。 */
export function getNodeBenchmarkSnapshot(window: BenchmarkWindow) {
  return invoke<BenchmarkSnapshot>('get_node_benchmark_snapshot', { window })
}

/** 读取草稿范围内的具体节点，不保存设置也不启动测试。 */
export function previewNodeBenchmarkCandidates(settings: BenchmarkSettings) {
  return invoke<BenchmarkPreview>('preview_node_benchmark_candidates', {
    settings,
  })
}

/** 一次性保存设置和最终参赛节点，并保持评选停止。 */
export function saveNodeBenchmarkSettings(request: BenchmarkSaveRequest) {
  return invoke<BenchmarkSnapshot>('save_node_benchmark_settings', {
    request,
  })
}

/** 明确启动长期评选调度。 */
export function startNodeBenchmark() {
  return invoke<BenchmarkSnapshot>('start_node_benchmark')
}

/** 停止长期评选调度并保留历史结果。 */
export function pauseNodeBenchmark() {
  return invoke<BenchmarkSnapshot>('pause_node_benchmark')
}

/** 立即扫描已选择订阅并比较节点变化。 */
export function refreshNodeBenchmarkProfiles() {
  return invoke<void>('refresh_node_benchmark_profiles')
}

/** 更新节点参赛状态或提醒策略。 */
export function updateNodeBenchmarkNode(request: NodeUpdateRequest) {
  return invoke<void>('update_node_benchmark_node', { request })
}

/** 立即安排选中节点重新执行延迟和上下行测试。 */
export function retestNodeBenchmarkNodes(nodeIds: string[]) {
  return invoke<void>('retest_node_benchmark_nodes', { nodeIds })
}

/** 删除全部历史测试记录并返回删除数量；订阅、设置和参赛名单保持不变。 */
export function clearNodeBenchmarkHistory() {
  return invoke<number>('clear_node_benchmark_history')
}

/** 启动不切换用户当前节点的手动批量测速。 */
export function startNodeBenchmarkManualBatch(request: ManualBatchRequest) {
  return invoke<ManualBatchSnapshot>('start_node_benchmark_manual_batch', {
    request,
  })
}

/** 读取手动批量测速的最新状态。 */
export function getNodeBenchmarkManualBatch(jobId: string) {
  return invoke<ManualBatchSnapshot>('get_node_benchmark_manual_batch', {
    jobId,
  })
}

/** 取消当前和后续手动测速节点。 */
export function cancelNodeBenchmarkManualBatch(jobId: string) {
  return invoke<void>('cancel_node_benchmark_manual_batch', { jobId })
}
