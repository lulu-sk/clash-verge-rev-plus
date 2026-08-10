use crate::{
    cmd::{CmdResult, StringifyErr as _},
    feat::node_benchmark::{
        BenchmarkManager,
        models::{
            BenchmarkPreview, BenchmarkSaveRequest, BenchmarkSettings, BenchmarkSnapshot, BenchmarkWindow,
            ManualBatchRequest, ManualBatchSnapshot, NodeUpdateRequest,
        },
    },
};
use std::sync::Arc;

/// 返回已初始化管理器并把启动错误转换为前端可读文本。
fn manager() -> CmdResult<&'static Arc<BenchmarkManager>> {
    BenchmarkManager::global().stringify_err()
}

/// 读取指定时间窗口的节点评选面板快照。
#[tauri::command]
pub async fn get_node_benchmark_snapshot(window: BenchmarkWindow) -> CmdResult<BenchmarkSnapshot> {
    manager()?.snapshot(window).await.stringify_err()
}

/// 读取草稿范围内的候选节点，不保存设置或启动测试。
#[tauri::command]
pub async fn preview_node_benchmark_candidates(settings: BenchmarkSettings) -> CmdResult<BenchmarkPreview> {
    manager()?.preview_nodes(settings).await.stringify_err()
}

/// 一次性保存设置和最终参赛节点，并返回停止状态下的24小时快照。
#[tauri::command]
pub async fn save_node_benchmark_settings(request: BenchmarkSaveRequest) -> CmdResult<BenchmarkSnapshot> {
    manager()?.save_settings(request).await.stringify_err()
}

/// 用户明确确认后启动长期评选。
#[tauri::command]
pub async fn start_node_benchmark() -> CmdResult<BenchmarkSnapshot> {
    manager()?.start_scheduled().await.stringify_err()
}

/// 停止长期评选并保留已经产生的结果。
#[tauri::command]
pub async fn pause_node_benchmark() -> CmdResult<BenchmarkSnapshot> {
    manager()?.pause_scheduled().await.stringify_err()
}

/// 立即重新扫描当前选中的全部订阅。
#[tauri::command]
pub async fn refresh_node_benchmark_profiles() -> CmdResult {
    manager()?.refresh_profiles().await.stringify_err()
}

/// 更新节点参赛状态或延迟提醒处理结果。
#[tauri::command]
pub fn update_node_benchmark_node(request: NodeUpdateRequest) -> CmdResult {
    manager()?.update_node(request).stringify_err()
}

/// 将选中节点的计划提前到当前时间。
#[tauri::command]
pub fn retest_node_benchmark_nodes(node_ids: Vec<String>) -> CmdResult {
    manager()?.retest_nodes(node_ids).stringify_err()
}

/// 在任务停止后删除全部历史测试数据，并返回删除的记录数量。
#[tauri::command]
pub async fn clear_node_benchmark_history() -> CmdResult<usize> {
    manager()?.clear_history().await.stringify_err()
}

/// 取消当前长期评选任务但保留已经保存的结果。
#[tauri::command]
pub fn cancel_node_benchmark_task() -> CmdResult {
    manager()?.cancel_scheduled();
    Ok(())
}

/// 启动现有代理页面与长期评选共用的独立批量测速任务。
#[tauri::command]
pub async fn start_node_benchmark_manual_batch(request: ManualBatchRequest) -> CmdResult<ManualBatchSnapshot> {
    manager()?.start_manual_batch(request).await.stringify_err()
}

/// 读取手动批量测速任务的逐节点进度。
#[tauri::command]
pub fn get_node_benchmark_manual_batch(job_id: String) -> CmdResult<ManualBatchSnapshot> {
    manager()?.manual_batch(&job_id).stringify_err()
}

/// 停止当前请求和手动批量队列中的后续节点。
#[tauri::command]
pub fn cancel_node_benchmark_manual_batch(job_id: String) -> CmdResult {
    manager()?.cancel_manual_batch(&job_id).stringify_err()
}
