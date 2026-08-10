use super::{
    core::BenchmarkCore,
    models::{
        BenchmarkCandidateNode, BenchmarkPreview, BenchmarkSaveRequest, BenchmarkSettings, BenchmarkSnapshot,
        BenchmarkTaskStatus, BenchmarkWindow, BenchmarkWorkerStatus, DiscoveredNode, LATENCY_ESTIMATED_DOWNLOAD_BYTES,
        LATENCY_ESTIMATED_UPLOAD_BYTES, ManualBatchRequest, ManualBatchRow, ManualBatchSnapshot, ManualSpeedMode,
        MeasurementKind, MeasurementRecord, NodeUpdateRequest, ParticipationState,
    },
    profile::{available_profiles, current_profile_uid, prepare_profile},
    stats::{build_rankings, build_traffic_summary},
    store::BenchmarkStore,
};
use crate::{
    feat::{SpeedTestFailure, SpeedTestOptions, test_download_speed_with_port, test_upload_speed_with_port},
    utils::{connections_stream, dirs},
};
use anyhow::{Context as _, Result};
use chrono::{Days, Local, TimeZone as _};
use clash_verge_logging::{Type, logging};
use once_cell::sync::OnceCell;
use parking_lot::{Mutex, RwLock};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicI64, AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

static BENCHMARK_MANAGER: OnceCell<Arc<BenchmarkManager>> = OnceCell::new();

/// 主线路总速率达到约40 Mbps时暂缓高流量测速，避免明显抢占用户正在使用的带宽。
const BUSY_NETWORK_BYTES_PER_SECOND: u64 = 5 * 1024 * 1024;

/// 相邻测量之间统一静置两秒，让带宽队列和连接状态恢复稳定。
const MEASUREMENT_COOLDOWN: Duration = Duration::from_secs(2);

/// 一次纯读取订阅得到的节点和变更识别信息。
struct ProfileDiscovery {
    profile_name: String,
    source_hash: String,
    nodes: Vec<DiscoveredNode>,
}

/// 长期评选中的两类后台队列。
#[derive(Clone, Copy)]
enum ScheduledWorker {
    Latency,
    Speed,
}

/// 统筹订阅发现、长期调度、统一测速队列和面板快照。
pub struct BenchmarkManager {
    store: Arc<BenchmarkStore>,
    operation_lock: tokio::sync::Mutex<()>,
    measurement_lock: tokio::sync::Mutex<()>,
    latency_cycle_lock: tokio::sync::Mutex<()>,
    speed_cycle_lock: tokio::sync::Mutex<()>,
    task_status: RwLock<BenchmarkTaskStatus>,
    scheduled_latency_cancel: Mutex<Option<CancellationToken>>,
    scheduled_speed_cancel: Mutex<Option<CancellationToken>>,
    manual_jobs: RwLock<HashMap<String, ManualBatchSnapshot>>,
    manual_tokens: Mutex<HashMap<String, CancellationToken>>,
    shutdown: CancellationToken,
    last_scan_at: AtomicI64,
    measurement_round: AtomicU64,
}

impl BenchmarkManager {
    /// 初始化本地数据库并启动不依赖前端页面的后台循环。
    pub fn initialize() -> Result<&'static Arc<Self>> {
        if let Some(manager) = BENCHMARK_MANAGER.get() {
            return Ok(manager);
        }
        let database_path = dirs::app_home_dir()?.join("node-benchmark").join("benchmark.sqlite3");
        let manager = Arc::new(Self {
            store: Arc::new(BenchmarkStore::open(&database_path)?),
            operation_lock: tokio::sync::Mutex::new(()),
            measurement_lock: tokio::sync::Mutex::new(()),
            latency_cycle_lock: tokio::sync::Mutex::new(()),
            speed_cycle_lock: tokio::sync::Mutex::new(()),
            task_status: RwLock::new(BenchmarkTaskStatus::default()),
            scheduled_latency_cancel: Mutex::new(None),
            scheduled_speed_cancel: Mutex::new(None),
            manual_jobs: RwLock::new(HashMap::new()),
            manual_tokens: Mutex::new(HashMap::new()),
            shutdown: CancellationToken::new(),
            last_scan_at: AtomicI64::new(0),
            measurement_round: AtomicU64::new(0),
        });
        BENCHMARK_MANAGER
            .set(Arc::clone(&manager))
            .map_err(|_| anyhow::anyhow!("节点评选管理器重复初始化"))?;
        tokio::spawn(manager.run_loop());
        Ok(BENCHMARK_MANAGER.get().expect("节点评选管理器刚刚已经初始化"))
    }

    /// 返回已经初始化的全局管理器。
    pub fn global() -> Result<&'static Arc<Self>> {
        BENCHMARK_MANAGER
            .get()
            .ok_or_else(|| anyhow::anyhow!("节点评选后台尚未初始化"))
    }

    /// 只读取草稿所选订阅的全部节点，不保存设置也不启动任何测试。
    pub async fn preview_nodes(&self, settings: BenchmarkSettings) -> Result<BenchmarkPreview> {
        let mut settings = settings.normalized()?;
        settings.enabled = false;
        if settings.selected_profile_uids.is_empty() {
            anyhow::bail!("请先选择至少一个订阅")
        }

        let _guard = self.operation_lock.lock().await;
        let mut nodes = Vec::new();
        for profile_uid in &settings.selected_profile_uids {
            let discovery = Self::discover_profile(profile_uid)
                .await
                .with_context(|| format!("无法读取订阅 {profile_uid} 的节点"))?;
            nodes.extend(discovery.nodes.into_iter().map(|node| BenchmarkCandidateNode {
                node_id: node.node_id,
                profile_uid: node.profile_uid,
                profile_name: node.profile_name,
                node_name: node.node_name,
                node_type: node.node_type,
                fingerprint: node.fingerprint,
            }));
        }
        nodes.sort_by(|left, right| {
            left.profile_name
                .cmp(&right.profile_name)
                .then_with(|| left.node_name.cmp(&right.node_name))
        });
        Ok(BenchmarkPreview { settings, nodes })
    }

    /// 一次性保存设置和最终参赛节点，暂停状态下绝不直接开始测速。
    pub async fn save_settings(&self, request: BenchmarkSaveRequest) -> Result<BenchmarkSnapshot> {
        let mut settings = request.settings.normalized()?;
        settings.enabled = false;
        if settings.selected_profile_uids.is_empty() {
            anyhow::bail!("请先选择至少一个订阅")
        }
        if request.participating_node_ids.is_empty() {
            anyhow::bail!("请至少选择一个参赛节点")
        }

        self.cancel_scheduled();
        let snapshot = {
            let _guard = self.operation_lock.lock().await;
            self.store.save_settings(&settings)?;
            self.scan_selected_profiles(&settings).await?;
            self.store.confirm_participating_nodes(
                &settings.selected_profile_uids,
                &request.participating_node_ids,
                unix_now(),
            )?;
            if !self.store.node_selection_confirmed(&settings.selected_profile_uids)? {
                anyhow::bail!("参赛列表没有完整保存，请重新预览后再试")
            }
            self.snapshot(BenchmarkWindow::OneDay).await?
        };
        Ok(snapshot)
    }

    /// 明确启动长期评选调度；设置保存本身不会调用此方法。
    pub async fn start_scheduled(&self) -> Result<BenchmarkSnapshot> {
        let mut settings = self.store.load_settings()?;
        if settings.selected_profile_uids.is_empty() {
            anyhow::bail!("请先选择订阅、勾选节点并保存参赛列表")
        }
        if !self.store.node_selection_confirmed(&settings.selected_profile_uids)? {
            anyhow::bail!("订阅节点已经发生变化，请重新检查并保存参赛列表")
        }
        let selected_profiles = settings
            .selected_profile_uids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let participating_count = self
            .store
            .active_nodes()?
            .into_iter()
            .filter(|node| {
                selected_profiles.contains(node.profile_uid.as_str()) && node.state == ParticipationState::Participating
            })
            .count();
        if participating_count == 0 {
            anyhow::bail!("请先勾选并保存至少一个参赛节点")
        }
        settings.enabled = true;
        self.store.save_settings(&settings)?;
        self.snapshot(BenchmarkWindow::OneDay).await
    }

    /// 停止长期评选并取消当前任务，同时保留配置和全部历史结果。
    pub async fn pause_scheduled(&self) -> Result<BenchmarkSnapshot> {
        let mut settings = self.store.load_settings()?;
        settings.enabled = false;
        self.store.save_settings(&settings)?;
        self.cancel_scheduled();
        self.snapshot(BenchmarkWindow::OneDay).await
    }

    /// 强制重新读取所选订阅，并进行新增、移除、改名和配置变化比较。
    pub async fn refresh_profiles(&self) -> Result<()> {
        let _guard = self.operation_lock.lock().await;
        let settings = self.store.load_settings()?;
        self.scan_selected_profiles(&settings).await
    }

    /// 更新节点参赛状态或提醒策略。
    pub fn update_node(&self, request: NodeUpdateRequest) -> Result<()> {
        let now = unix_now();
        if let Some(state) = request.state {
            if matches!(state, ParticipationState::Changed | ParticipationState::Removed) {
                anyhow::bail!("该状态只能由订阅变化产生")
            }
            self.store.update_node_state(&request.node_id, state, now)?;
        }
        if let Some(action) = request.reminder_action {
            self.store.update_reminder(&request.node_id, action, now)?;
        }
        Ok(())
    }

    /// 把选中节点的延迟和上下行任务提前到当前时间。
    pub fn retest_nodes(&self, node_ids: Vec<String>) -> Result<()> {
        if node_ids.is_empty() {
            anyhow::bail!("至少选择一个节点")
        }
        self.store.force_due(&node_ids, unix_now())
    }

    /// 在评选停止后删除全部历史成绩并把已保存节点恢复到新一轮快速评测起点。
    pub async fn clear_history(&self) -> Result<usize> {
        if self.store.load_settings()?.enabled {
            anyhow::bail!("请先停止节点评选，再清理历史测试数据")
        }
        if self.manual_jobs.read().values().any(|job| job.running) {
            anyhow::bail!("仍有手动批量测速正在运行，请先停止后再清理")
        }

        self.cancel_scheduled();
        let _operation_guard = self.operation_lock.lock().await;
        let _measurement_guard = self.measurement_lock.lock().await;
        if self.store.load_settings()?.enabled {
            anyhow::bail!("节点评选已经重新运行，请停止后再清理")
        }
        if self.manual_jobs.read().values().any(|job| job.running) {
            anyhow::bail!("仍有手动批量测速正在运行，请先停止后再清理")
        }
        let deleted = self.store.clear_measurements(unix_now())?;
        *self.task_status.write() = BenchmarkTaskStatus::default();
        Ok(deleted)
    }

    /// 生成页面一次刷新所需的全部数据。
    pub async fn snapshot(&self, window: BenchmarkWindow) -> Result<BenchmarkSnapshot> {
        let now = unix_now();
        let settings = self.store.load_settings()?;
        let available = available_profiles().await;
        let selected = settings
            .selected_profile_uids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let nodes = self
            .store
            .active_nodes()?
            .into_iter()
            .filter(|node| selected.contains(node.profile_uid.as_str()))
            .collect::<Vec<_>>();
        let measurement_start = if window == BenchmarkWindow::LongTerm {
            0
        } else {
            now - 7 * 24 * 60 * 60
        };
        let node_ids = nodes.iter().map(|node| node.node_id.clone()).collect::<Vec<_>>();
        let measurements = self.store.measurements_for_nodes_since(&node_ids, measurement_start)?;
        let (node_rows, champions, reminders) = build_rankings(&nodes, &measurements, window, &settings, now);
        let day_start = local_day_start();
        let actual_by_profile = self.store.actual_traffic_by_profile_since(day_start)?;
        let traffic = build_traffic_summary(
            &nodes,
            &measurements,
            &actual_by_profile,
            &settings,
            now,
            seconds_until_local_day_end(),
        );
        Ok(BenchmarkSnapshot {
            settings: settings.clone(),
            selection_confirmed: self.store.node_selection_confirmed(&settings.selected_profile_uids)?,
            profiles: self.store.profile_summaries(&available, &settings)?,
            nodes: node_rows,
            champions,
            traffic,
            reminders,
            changes: self.store.recent_changes(50, &settings)?,
            task: self.task_status.read().clone(),
            generated_at: now,
        })
    }

    /// 启动使用独立内核的手动单节点或批量测速任务。
    pub async fn start_manual_batch(self: &Arc<Self>, request: ManualBatchRequest) -> Result<ManualBatchSnapshot> {
        if request.node_names.is_empty() {
            anyhow::bail!("没有可测速节点")
        }
        let profile_uid = match request.profile_uid.clone() {
            Some(uid) => uid,
            None => current_profile_uid()
                .await
                .context("当前没有选中的订阅，无法建立独立测速配置")?,
        };
        let job_id = nanoid::nanoid!(12);
        let snapshot = ManualBatchSnapshot {
            job_id: job_id.clone(),
            running: true,
            mode: request.mode,
            rows: request
                .node_names
                .iter()
                .map(|name| ManualBatchRow {
                    node_name: name.clone(),
                    status: "queued".to_owned(),
                    error: None,
                    result: None,
                })
                .collect(),
        };
        let token = CancellationToken::new();
        self.manual_jobs.write().insert(job_id.clone(), snapshot.clone());
        self.manual_tokens.lock().insert(job_id.clone(), token.clone());
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager.run_manual_batch(job_id, profile_uid, request, token).await;
        });
        Ok(snapshot)
    }

    /// 返回指定手动测速任务的最新逐节点状态。
    pub fn manual_batch(&self, job_id: &str) -> Result<ManualBatchSnapshot> {
        self.manual_jobs
            .read()
            .get(job_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("找不到手动测速任务"))
    }

    /// 取消正在排队或执行的手动测速任务。
    pub fn cancel_manual_batch(&self, job_id: &str) -> Result<()> {
        let token = self
            .manual_tokens
            .lock()
            .get(job_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("找不到可取消的手动测速任务"))?;
        token.cancel();
        Ok(())
    }

    /// 取消当前一轮长期评选任务，后续是否继续派发由总开关和调度时间决定。
    pub fn cancel_scheduled(&self) {
        if let Some(token) = self.scheduled_latency_cancel.lock().take() {
            token.cancel();
        }
        if let Some(token) = self.scheduled_speed_cancel.lock().take() {
            token.cancel();
        }
    }

    /// 在应用退出或重启前取消所有独立测速任务。
    pub fn shutdown(&self) {
        self.shutdown.cancel();
        self.cancel_scheduled();
        for token in self.manual_tokens.lock().values() {
            token.cancel();
        }
    }

    /// 周期派发扫描、延迟和速度队列；实际网络测试由统一互斥锁串行执行。
    async fn run_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_secs(20));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                _ = interval.tick() => {
                    let scan_manager = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(error) = scan_manager.run_profile_scan_cycle().await {
                            logging!(error, Type::System, "节点评选订阅扫描失败: {error:#}");
                        }
                    });
                    let latency_manager = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(error) = latency_manager.run_latency_cycle().await {
                            logging!(error, Type::System, "节点评选延迟队列失败: {error:#}");
                            latency_manager.finish_worker(
                                ScheduledWorker::Latency,
                                0,
                                0,
                                "failed",
                                Some(&error.to_string()),
                            );
                        }
                    });
                    let speed_manager = Arc::clone(&self);
                    tokio::spawn(async move {
                        if let Err(error) = speed_manager.run_speed_cycle().await {
                            logging!(error, Type::System, "节点评选速度队列失败: {error:#}");
                            speed_manager.finish_worker(
                                ScheduledWorker::Speed,
                                0,
                                0,
                                "failed",
                                Some(&error.to_string()),
                            );
                        }
                    });
                }
            }
        }
    }

    /// 在统一配置锁空闲时执行一次半小时订阅变化扫描。
    async fn run_profile_scan_cycle(&self) -> Result<()> {
        let Ok(_guard) = self.operation_lock.try_lock() else {
            return Ok(());
        };
        let settings = self.store.load_settings()?;
        if !settings.enabled {
            return Ok(());
        }
        let now = unix_now();
        if now - self.last_scan_at.load(Ordering::Acquire) >= 30 * 60 {
            self.scan_selected_profiles(&settings).await?;
        }
        Ok(())
    }

    /// 执行一轮到期延迟任务；该队列不会被长时间的上下行测速占用。
    async fn run_latency_cycle(&self) -> Result<()> {
        let Ok(_cycle_guard) = self.latency_cycle_lock.try_lock() else {
            return Ok(());
        };
        let settings = self.store.load_settings()?;
        if !settings.enabled {
            return Ok(());
        }
        let now = unix_now();
        let selected = settings
            .selected_profile_uids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let due_nodes = self
            .store
            .due_latency_nodes(now)?
            .into_iter()
            .filter(|node| selected.contains(node.profile_uid.as_str()))
            .collect::<Vec<_>>();
        if due_nodes.is_empty() {
            return Ok(());
        }

        let token = CancellationToken::new();
        *self.scheduled_latency_cancel.lock() = Some(token.clone());
        self.set_worker_status(
            ScheduledWorker::Latency,
            "preparing",
            None,
            None,
            0,
            due_nodes.len(),
            None,
        );
        let result = async {
            let mut grouped = BTreeMap::<String, Vec<_>>::new();
            for node in due_nodes {
                grouped.entry(node.profile_uid.clone()).or_default().push(node);
            }
            let total = grouped.values().map(Vec::len).sum();
            let round = self.measurement_round.fetch_add(1, Ordering::AcqRel);
            let mut grouped = grouped.into_iter().collect::<Vec<_>>();
            rotate_for_round(&mut grouped, round);
            for (index, (_, nodes)) in grouped.iter_mut().enumerate() {
                rotate_for_round(nodes, round.saturating_add(index as u64));
            }
            let mut completed = 0;
            for (profile_uid, nodes) in grouped {
                if token.is_cancelled() {
                    break;
                }
                let prepared = match prepare_profile(&profile_uid, 1).await {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let message = format!("无法准备订阅延迟测试：{error:#}");
                        for node in &nodes {
                            self.store.defer_latency(&node.node_id, now + 10 * 60)?;
                        }
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Latency,
                            "startingCore",
                            nodes.first().map(|node| node.profile_name.as_str()),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                };
                self.set_worker_status(
                    ScheduledWorker::Latency,
                    "startingCore",
                    Some(&prepared.profile_name),
                    None,
                    completed,
                    total,
                    None,
                );
                let core = match BenchmarkCore::start(&prepared).await {
                    Ok(core) => core,
                    Err(error) => {
                        let message = format!("独立延迟内核启动失败：{error:#}");
                        for node in &nodes {
                            self.store.defer_latency(&node.node_id, now + 10 * 60)?;
                        }
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Latency,
                            "startingCore",
                            Some(&prepared.profile_name),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                };
                let available_names = match core.list_nodes().await {
                    Ok(nodes) => nodes,
                    Err(error) => {
                        let message = format!("无法读取独立延迟节点：{error:#}");
                        for node in &nodes {
                            self.store.defer_latency(&node.node_id, now + 10 * 60)?;
                        }
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Latency,
                            "startingCore",
                            Some(&prepared.profile_name),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                }
                .into_iter()
                .map(|(name, _)| name)
                .collect::<HashSet<_>>();
                for node in nodes {
                    if token.is_cancelled() {
                        break;
                    }
                    if !available_names.contains(&node.node_name) {
                        self.store.defer_latency(&node.node_id, now + 10 * 60)?;
                        completed += 1;
                        continue;
                    }
                    self.run_latency(&core, &node, &settings, &token, completed, total)
                        .await?;
                    completed += 1;
                }
            }
            Result::<(usize, usize)>::Ok((completed, total))
        }
        .await;

        *self.scheduled_latency_cancel.lock() = None;
        let was_cancelled = token.is_cancelled();
        match result {
            Ok((completed, total)) => {
                self.finish_worker(
                    ScheduledWorker::Latency,
                    completed,
                    total,
                    if was_cancelled { "cancelled" } else { "idle" },
                    None,
                );
                Ok(())
            }
            Err(error) => {
                self.finish_worker(ScheduledWorker::Latency, 0, 0, "failed", Some(&error.to_string()));
                Err(error)
            }
        }
    }

    /// 执行一轮到期下载和上传任务；两个方向分别决定成功推进或短时重试。
    async fn run_speed_cycle(&self) -> Result<()> {
        let Ok(_cycle_guard) = self.speed_cycle_lock.try_lock() else {
            return Ok(());
        };
        let Ok(_operation_guard) = self.operation_lock.try_lock() else {
            return Ok(());
        };
        let settings = self.store.load_settings()?;
        if !settings.enabled {
            return Ok(());
        }
        let now = unix_now();
        let selected = settings
            .selected_profile_uids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let due_nodes = self
            .store
            .due_speed_nodes(now)?
            .into_iter()
            .filter(|node| selected.contains(node.profile_uid.as_str()))
            .collect::<Vec<_>>();
        if due_nodes.is_empty() {
            return Ok(());
        }

        let token = CancellationToken::new();
        *self.scheduled_speed_cancel.lock() = Some(token.clone());
        self.set_worker_status(
            ScheduledWorker::Speed,
            "checkingNetwork",
            None,
            None,
            0,
            due_nodes.len(),
            None,
        );
        let result = async {
            if main_network_is_busy(&token).await {
                for node in &due_nodes {
                    if node.next_download_at <= now {
                        self.store
                            .defer_speed_direction(&node.node_id, MeasurementKind::Download, now + 10 * 60)?;
                    }
                    if node.next_upload_at <= now {
                        self.store
                            .defer_speed_direction(&node.node_id, MeasurementKind::Upload, now + 10 * 60)?;
                    }
                }
                return Ok((due_nodes.len(), due_nodes.len()));
            }
            if token.is_cancelled() {
                return Ok((0, due_nodes.len()));
            }

            let mut grouped = BTreeMap::<String, Vec<_>>::new();
            for node in due_nodes {
                grouped.entry(node.profile_uid.clone()).or_default().push(node);
            }
            let total = grouped.values().map(Vec::len).sum();
            let round = self.measurement_round.fetch_add(1, Ordering::AcqRel);
            let mut grouped = grouped.into_iter().collect::<Vec<_>>();
            rotate_for_round(&mut grouped, round);
            for (index, (_, nodes)) in grouped.iter_mut().enumerate() {
                rotate_for_round(nodes, round.saturating_add(index as u64));
            }
            let mut completed = 0;
            for (profile_uid, nodes) in grouped {
                if token.is_cancelled() {
                    break;
                }
                let prepared = match prepare_profile(&profile_uid, 1).await {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        let message = format!("无法准备订阅速度测试：{error:#}");
                        self.defer_speed_nodes(&nodes, now, now + 10 * 60)?;
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Speed,
                            "startingCore",
                            nodes.first().map(|node| node.profile_name.as_str()),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                };
                self.set_worker_status(
                    ScheduledWorker::Speed,
                    "startingCore",
                    Some(&prepared.profile_name),
                    None,
                    completed,
                    total,
                    None,
                );
                let core = match BenchmarkCore::start(&prepared).await {
                    Ok(core) => core,
                    Err(error) => {
                        let message = format!("独立速度内核启动失败：{error:#}");
                        self.defer_speed_nodes(&nodes, now, now + 10 * 60)?;
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Speed,
                            "startingCore",
                            Some(&prepared.profile_name),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                };
                let available_names = match core.list_nodes().await {
                    Ok(nodes) => nodes,
                    Err(error) => {
                        let message = format!("无法读取独立速度节点：{error:#}");
                        self.defer_speed_nodes(&nodes, now, now + 10 * 60)?;
                        completed += nodes.len();
                        self.set_worker_status(
                            ScheduledWorker::Speed,
                            "startingCore",
                            Some(&prepared.profile_name),
                            None,
                            completed,
                            total,
                            Some(&message),
                        );
                        continue;
                    }
                }
                .into_iter()
                .map(|(name, _)| name)
                .collect::<HashSet<_>>();
                let mut download_environment = None;
                let mut upload_environment = None;
                for node in nodes {
                    if token.is_cancelled() {
                        break;
                    }
                    if !available_names.contains(&node.node_name) {
                        self.record_unavailable_speed_directions(&node, now)?;
                        completed += 1;
                        continue;
                    }
                    if let Err(error) = core.select_node(&node.node_name).await {
                        self.record_node_selection_failure(&node, now, &error.to_string())?;
                        completed += 1;
                        continue;
                    }
                    self.run_speed_node(
                        &core,
                        &node,
                        &settings,
                        &token,
                        node.next_download_at <= now,
                        node.next_upload_at <= now,
                        &mut download_environment,
                        &mut upload_environment,
                        completed,
                        total,
                    )
                    .await?;
                    completed += 1;
                }
            }
            Result::<(usize, usize)>::Ok((completed, total))
        }
        .await;

        *self.scheduled_speed_cancel.lock() = None;
        let was_cancelled = token.is_cancelled();
        match result {
            Ok((completed, total)) => {
                self.finish_worker(
                    ScheduledWorker::Speed,
                    completed,
                    total,
                    if was_cancelled { "cancelled" } else { "idle" },
                    None,
                );
                Ok(())
            }
            Err(error) => {
                self.finish_worker(ScheduledWorker::Speed, 0, 0, "failed", Some(&error.to_string()));
                Err(error)
            }
        }
    }

    /// 扫描全部已选订阅并把变化写入 SQLite。
    async fn scan_selected_profiles(&self, settings: &BenchmarkSettings) -> Result<()> {
        let now = unix_now();
        let profiles = available_profiles().await.into_iter().collect::<HashMap<_, _>>();
        for profile_uid in &settings.selected_profile_uids {
            if !profiles.contains_key(profile_uid) {
                self.store.mark_profile_missing(profile_uid, profile_uid, now)?;
                continue;
            }
            let profile_name = profiles
                .get(profile_uid)
                .cloned()
                .unwrap_or_else(|| profile_uid.clone());
            let result = async {
                let discovery = Self::discover_profile(profile_uid).await?;
                self.store
                    .reconcile_nodes(profile_uid, &discovery.profile_name, &discovery.nodes, settings, now)?;
                self.store.save_profile_scan(
                    profile_uid,
                    &discovery.profile_name,
                    Some(&discovery.source_hash),
                    now,
                    None,
                )?;
                Result::<()>::Ok(())
            }
            .await;
            if let Err(error) = result {
                self.store
                    .save_profile_scan(profile_uid, &profile_name, None, now, Some(&error.to_string()))?;
            }
        }
        self.last_scan_at.store(now, Ordering::Release);
        Ok(())
    }

    /// 从一个订阅配置中发现全部代理节点，不写数据库也不执行网络测速。
    async fn discover_profile(profile_uid: &str) -> Result<ProfileDiscovery> {
        let prepared = prepare_profile(profile_uid, 1).await?;
        let nodes = if prepared.has_proxy_providers() {
            let core = BenchmarkCore::start(&prepared).await?;
            prepared.discovered_nodes(&core.list_nodes().await?)
        } else {
            prepared.direct_nodes()
        };
        Ok(ProfileDiscovery {
            profile_name: prepared.profile_name,
            source_hash: prepared.source_hash,
            nodes,
        })
    }

    /// 为单个仅延迟到期的节点排队，静置后执行一次低流量延迟测试。
    async fn run_latency(
        &self,
        core: &BenchmarkCore,
        node: &super::models::StoredNode,
        settings: &BenchmarkSettings,
        token: &CancellationToken,
        completed: usize,
        total: usize,
    ) -> Result<()> {
        self.set_worker_status(
            ScheduledWorker::Latency,
            "waitingForOtherQueue",
            Some(&node.profile_name),
            Some(&node.node_name),
            completed,
            total,
            None,
        );
        let _measurement_guard = tokio::select! {
            _ = token.cancelled() => return Ok(()),
            guard = self.measurement_lock.lock() => guard,
        };
        if !self
            .wait_scheduled_cooldown(ScheduledWorker::Latency, node, token, completed, total)
            .await
        {
            return Ok(());
        }
        self.run_latency_measurement(core, node, settings, token, ScheduledWorker::Latency, completed, total)
            .await
    }

    /// 在已经取得统一测速锁的情况下记录一次延迟，并独立安排下次延迟时间。
    #[allow(clippy::too_many_arguments)]
    async fn run_latency_measurement(
        &self,
        core: &BenchmarkCore,
        node: &super::models::StoredNode,
        settings: &BenchmarkSettings,
        token: &CancellationToken,
        worker: ScheduledWorker,
        completed: usize,
        total: usize,
    ) -> Result<()> {
        self.set_worker_status(
            worker,
            "latency",
            Some(&node.profile_name),
            Some(&node.node_name),
            completed,
            total,
            None,
        );
        let started = std::time::Instant::now();
        let result = tokio::select! {
            _ = token.cancelled() => return Ok(()),
            result = core.test_delay(&node.node_name, &settings.latency_url, 10_000) => result,
        };
        let now = unix_now();
        let environment_available = if result.is_err() {
            let Some(available) = probe_test_environment(core, &settings.latency_url, token).await else {
                return Ok(());
            };
            available
        } else {
            true
        };
        let value = result.as_ref().ok().map(|delay| f64::from(*delay));
        let error = result.err().map(|error| {
            if environment_available {
                error.to_string()
            } else {
                format!("测试环境异常，不计入节点稳定率：{error}")
            }
        });
        self.store.insert_measurement(&MeasurementRecord {
            node_id: node.node_id.clone(),
            kind: MeasurementKind::Latency,
            success: error.is_none(),
            value,
            bytes_down: LATENCY_ESTIMATED_DOWNLOAD_BYTES.saturating_mul(if environment_available { 1 } else { 2 }),
            bytes_up: LATENCY_ESTIMATED_UPLOAD_BYTES.saturating_mul(if environment_available { 1 } else { 2 }),
            duration_ms: started.elapsed().as_millis() as u64,
            trigger: if environment_available {
                "scheduled"
            } else {
                "environment"
            }
            .to_owned(),
            rank_eligible: environment_available,
            error,
            created_at: now,
        })?;
        if !environment_available {
            self.store.defer_latency(&node.node_id, now + 10 * 60)?;
            return Ok(());
        }
        self.store
            .schedule_next_latency(&node.node_id, now + settings.latency_interval_minutes as i64 * 60)?;
        Ok(())
    }

    /// 把单个节点的延迟、下载和上传锁成不可被其他队列插入的完整测试单元。
    #[allow(clippy::too_many_arguments)]
    async fn run_speed_node(
        &self,
        core: &BenchmarkCore,
        node: &super::models::StoredNode,
        settings: &BenchmarkSettings,
        token: &CancellationToken,
        download_due: bool,
        upload_due: bool,
        download_environment: &mut Option<bool>,
        upload_environment: &mut Option<bool>,
        completed: usize,
        total: usize,
    ) -> Result<()> {
        self.set_worker_status(
            ScheduledWorker::Speed,
            "waitingForOtherQueue",
            Some(&node.profile_name),
            Some(&node.node_name),
            completed,
            total,
            None,
        );
        let _measurement_guard = tokio::select! {
            _ = token.cancelled() => return Ok(()),
            guard = self.measurement_lock.lock() => guard,
        };
        if !self
            .wait_scheduled_cooldown(ScheduledWorker::Speed, node, token, completed, total)
            .await
        {
            return Ok(());
        }

        self.run_latency_measurement(core, node, settings, token, ScheduledWorker::Speed, completed, total)
            .await?;
        if token.is_cancelled() {
            return Ok(());
        }
        if download_due {
            self.run_speed_direction(
                core,
                node,
                settings,
                token,
                MeasurementKind::Download,
                download_environment,
                completed,
                total,
            )
            .await?;
        }
        if download_due
            && upload_due
            && !token.is_cancelled()
            && !self
                .wait_scheduled_cooldown(ScheduledWorker::Speed, node, token, completed, total)
                .await
        {
            return Ok(());
        }
        if upload_due && !token.is_cancelled() {
            self.run_speed_direction(
                core,
                node,
                settings,
                token,
                MeasurementKind::Upload,
                upload_environment,
                completed,
                total,
            )
            .await?;
        }
        if !token.is_cancelled() {
            self.wait_scheduled_cooldown(ScheduledWorker::Speed, node, token, completed, total)
                .await;
        }
        Ok(())
    }

    /// 显示后台静置阶段并等待统一的两秒恢复时间。
    async fn wait_scheduled_cooldown(
        &self,
        worker: ScheduledWorker,
        node: &super::models::StoredNode,
        token: &CancellationToken,
        completed: usize,
        total: usize,
    ) -> bool {
        self.set_worker_status(
            worker,
            "cooldown",
            Some(&node.profile_name),
            Some(&node.node_name),
            completed,
            total,
            None,
        );
        wait_measurement_cooldown(token).await
    }

    /// 在已经取得统一测速锁的情况下执行单个速度方向，失败时保持阶段并短时重试。
    async fn run_speed_direction(
        &self,
        core: &BenchmarkCore,
        node: &super::models::StoredNode,
        settings: &BenchmarkSettings,
        token: &CancellationToken,
        kind: MeasurementKind,
        environment_available: &mut Option<bool>,
        completed: usize,
        total: usize,
    ) -> Result<()> {
        let (actual_download, actual_upload) = self.store.actual_traffic_since(local_day_start())?;
        let (phase, url, actual_bytes, daily_limit, bootstrap_stage) = match kind {
            MeasurementKind::Download => (
                "download",
                settings.download_url.as_str(),
                actual_download,
                settings.download_daily_limit_bytes,
                node.download_bootstrap_stage,
            ),
            MeasurementKind::Upload => (
                "upload",
                settings.upload_url.as_str(),
                actual_upload,
                settings.upload_daily_limit_bytes,
                node.upload_bootstrap_stage,
            ),
            MeasurementKind::Latency => anyhow::bail!("延迟任务不能进入速度队列"),
        };
        if actual_bytes.saturating_add(settings.speed_max_bytes) > daily_limit {
            self.store
                .schedule_next_speed_direction(&node.node_id, kind, bootstrap_stage, local_next_day_start())?;
            return Ok(());
        }
        if environment_available.is_some_and(|available| !available) {
            self.store.insert_measurement(&MeasurementRecord {
                node_id: node.node_id.clone(),
                kind,
                success: false,
                value: None,
                bytes_down: 0,
                bytes_up: 0,
                duration_ms: 0,
                trigger: "environment".to_owned(),
                rank_eligible: false,
                error: Some("本轮测速源直连检查失败，已自动顺延且不计入节点结果".to_owned()),
                created_at: unix_now(),
            })?;
            self.store
                .defer_speed_direction(&node.node_id, kind, unix_now() + 10 * 60)?;
            return Ok(());
        }

        self.set_worker_status(
            ScheduledWorker::Speed,
            phase,
            Some(&node.profile_name),
            Some(&node.node_name),
            completed,
            total,
            None,
        );
        let options = speed_options(url, settings);
        let started = std::time::Instant::now();
        let result = match kind {
            MeasurementKind::Download => tokio::select! {
                _ = token.cancelled() => return Ok(()),
                result = test_download_speed_with_port(options, core.mixed_port()) => {
                    result.map(|value| (value.average_bytes_per_second, value.bytes_read, 0, value.elapsed_ms))
                }
            },
            MeasurementKind::Upload => tokio::select! {
                _ = token.cancelled() => return Ok(()),
                result = test_upload_speed_with_port(options, core.mixed_port()) => {
                    result.map(|value| (value.average_bytes_per_second, 0, value.bytes_sent, value.elapsed_ms))
                }
            },
            MeasurementKind::Latency => unreachable!("延迟方向已经在上方拒绝"),
        };
        let now = unix_now();
        match result {
            Ok((speed, bytes_down, bytes_up, duration_ms)) => {
                self.store.insert_measurement(&MeasurementRecord {
                    node_id: node.node_id.clone(),
                    kind,
                    success: true,
                    value: Some(speed as f64),
                    bytes_down,
                    bytes_up,
                    duration_ms,
                    trigger: speed_trigger(bootstrap_stage).to_owned(),
                    rank_eligible: true,
                    error: None,
                    created_at: now,
                })?;
                let (next_stage, next_at) = next_speed_schedule(bootstrap_stage, now, settings.speed_interval_hours);
                self.store
                    .schedule_next_speed_direction(&node.node_id, kind, next_stage, next_at)?;
            }
            Err(error) => {
                let mut record =
                    failed_speed_record(node, kind, bootstrap_stage, started.elapsed().as_millis() as u64, error);
                let source_available = match *environment_available {
                    Some(available) => available,
                    None => {
                        let available = probe_speed_environment(core, node, kind, url, settings, token).await;
                        *environment_available = Some(available);
                        available
                    }
                };
                if !source_available {
                    record.rank_eligible = false;
                    record.trigger = "environment".to_owned();
                    record.error = record
                        .error
                        .map(|error| format!("测速源或本机直连检查失败，不计入节点结果：{error}"));
                }
                self.store.insert_measurement(&record)?;
                let retry_after = if source_available { 30 * 60 } else { 10 * 60 };
                self.store
                    .defer_speed_direction(&node.node_id, kind, now + retry_after)?;
            }
        }
        Ok(())
    }

    /// 顺延一组节点当前真正到期的速度方向。
    fn defer_speed_nodes(&self, nodes: &[super::models::StoredNode], now: i64, retry_at: i64) -> Result<()> {
        for node in nodes {
            if node.next_download_at <= now {
                self.store
                    .defer_speed_direction(&node.node_id, MeasurementKind::Download, retry_at)?;
            }
            if node.next_upload_at <= now {
                self.store
                    .defer_speed_direction(&node.node_id, MeasurementKind::Upload, retry_at)?;
            }
        }
        Ok(())
    }

    /// 记录独立配置缺少节点时两个到期方向的诊断信息并短时重试。
    fn record_unavailable_speed_directions(&self, node: &super::models::StoredNode, now: i64) -> Result<()> {
        self.record_scheduled_speed_failure(node, now, "独立测速配置中找不到该节点")
    }

    /// 记录临时内核无法选中节点时两个到期方向的诊断信息并短时重试。
    fn record_node_selection_failure(&self, node: &super::models::StoredNode, now: i64, error: &str) -> Result<()> {
        self.record_scheduled_speed_failure(node, now, &format!("临时内核无法选中节点：{error}"))
    }

    /// 为没有真正开始传输的调度错误登记失败原因，且不把它计入公平排名。
    fn record_scheduled_speed_failure(&self, node: &super::models::StoredNode, now: i64, error: &str) -> Result<()> {
        for (kind, due, stage) in [
            (
                MeasurementKind::Download,
                node.next_download_at <= now,
                node.download_bootstrap_stage,
            ),
            (
                MeasurementKind::Upload,
                node.next_upload_at <= now,
                node.upload_bootstrap_stage,
            ),
        ] {
            if !due {
                continue;
            }
            self.store.insert_measurement(&MeasurementRecord {
                node_id: node.node_id.clone(),
                kind,
                success: false,
                value: None,
                bytes_down: 0,
                bytes_up: 0,
                duration_ms: 0,
                trigger: speed_trigger(stage).to_owned(),
                rank_eligible: false,
                error: Some(error.to_owned()),
                created_at: now,
            })?;
            self.store.defer_speed_direction(&node.node_id, kind, now + 10 * 60)?;
        }
        Ok(())
    }

    /// 在共享队列中执行一次手动任务并持续更新前端可轮询快照。
    async fn run_manual_batch(
        self: Arc<Self>,
        job_id: String,
        profile_uid: String,
        request: ManualBatchRequest,
        token: CancellationToken,
    ) {
        let result = async {
            let _guard = tokio::select! {
                _ = token.cancelled() => return Ok(()),
                guard = self.operation_lock.lock() => guard,
            };
            let prepared = prepare_profile(&profile_uid, 1).await?;
            let core = BenchmarkCore::start(&prepared).await?;
            let api_nodes = core.list_nodes().await?;
            let discovered = prepared.discovered_nodes(&api_nodes);
            let settings = self.store.load_settings()?;
            self.store.reconcile_nodes(
                &profile_uid,
                &prepared.profile_name,
                &discovered,
                &settings,
                unix_now(),
            )?;
            self.store.save_profile_scan(
                &profile_uid,
                &prepared.profile_name,
                Some(&prepared.source_hash),
                unix_now(),
                None,
            )?;
            let available = api_nodes
                .into_iter()
                .map(|(name, _)| name)
                .collect::<HashSet<_>>();
            let rank_eligible = manual_matches_ranking(&request, &settings);
            for node_name in &request.node_names {
                if token.is_cancelled() {
                    break;
                }
                if !available.contains(node_name) {
                    self.update_manual_row(&job_id, node_name, "failed", Some("独立配置中找不到该节点"), None);
                    continue;
                }
                self.update_manual_row(&job_id, node_name, "testing", None, None);
                let (actual_download, actual_upload) = self.store.actual_traffic_since(local_day_start())?;
                let planned_bytes = request.options.max_bytes.unwrap_or(32 * 1024 * 1024);
                let within_budget = match request.mode {
                    ManualSpeedMode::Download => actual_download.saturating_add(planned_bytes)
                        <= settings.download_daily_limit_bytes,
                    ManualSpeedMode::Upload => actual_upload.saturating_add(planned_bytes)
                        <= settings.upload_daily_limit_bytes,
                };
                if !within_budget {
                    self.update_manual_row(
                        &job_id,
                        node_name,
                        "failed",
                        Some("今日对应方向的测试流量上限已达到"),
                        None,
                    );
                    continue;
                }
                if let Err(error) = core.select_node(node_name).await {
                    self.update_manual_row(
                        &job_id,
                        node_name,
                        "failed",
                        Some(&format!("临时内核无法选中节点：{error}")),
                        None,
                    );
                    continue;
                }
                let _measurement_guard = tokio::select! {
                    _ = token.cancelled() => break,
                    guard = self.measurement_lock.lock() => guard,
                };
                if !wait_measurement_cooldown(&token).await {
                    break;
                }
                let measurement_kind = match request.mode {
                    ManualSpeedMode::Download => MeasurementKind::Download,
                    ManualSpeedMode::Upload => MeasurementKind::Upload,
                };
                let test_url = request.options.url.as_str();
                let started = std::time::Instant::now();
                let result = match request.mode {
                    ManualSpeedMode::Download => tokio::select! {
                        _ = token.cancelled() => break,
                        result = test_download_speed_with_port(request.options.clone(), core.mixed_port()) => {
                            result.and_then(|value| Ok::<_, anyhow::Error>((serde_json::to_value(&value)?, MeasurementKind::Download, value.average_bytes_per_second, value.bytes_read, 0, value.elapsed_ms)))
                        }
                    },
                    ManualSpeedMode::Upload => tokio::select! {
                        _ = token.cancelled() => break,
                        result = test_upload_speed_with_port(request.options.clone(), core.mixed_port()) => {
                            result.and_then(|value| Ok::<_, anyhow::Error>((serde_json::to_value(&value)?, MeasurementKind::Upload, value.average_bytes_per_second, 0, value.bytes_sent, value.elapsed_ms)))
                        }
                    },
                };
                match result {
                    Ok((json, kind, speed, bytes_down, bytes_up, duration_ms)) => {
                        self.update_manual_row(&job_id, node_name, "success", None, Some(json.clone()));
                        if let Some(node) = self.store.node_by_profile_and_name(&profile_uid, node_name)? {
                            self.store.insert_measurement(&MeasurementRecord {
                                node_id: node.node_id,
                                kind,
                                success: true,
                                value: Some(speed as f64),
                                bytes_down,
                                bytes_up,
                                duration_ms,
                                trigger: "manual".to_owned(),
                                rank_eligible: rank_eligible
                                    && node.manual_override
                                    && node.state == ParticipationState::Participating,
                                error: None,
                                created_at: unix_now(),
                            })?;
                        }
                    }
                    Err(error) => {
                        let progress = error.downcast_ref::<SpeedTestFailure>();
                        let bytes_down = progress.map_or(0, |failure| failure.bytes_down);
                        let bytes_up = progress.map_or(0, |failure| failure.bytes_up);
                        let duration_ms = progress
                            .map_or_else(|| started.elapsed().as_millis() as u64, |failure| failure.elapsed_ms);
                        let stored_node = self.store.node_by_profile_and_name(&profile_uid, node_name)?;
                        let environment_available = if let Some(node) = stored_node.as_ref() {
                            probe_speed_environment(
                                &core,
                                node,
                                measurement_kind,
                                test_url,
                                &settings,
                                &token,
                            )
                            .await
                        } else {
                            probe_test_environment(&core, test_url, &token).await.unwrap_or(false)
                        };
                        let error_message = if environment_available {
                            error.to_string()
                        } else {
                            format!("测试环境异常，不计入节点结果：{error}")
                        };
                        self.update_manual_row(&job_id, node_name, "failed", Some(&error_message), None);
                        if let Some(node) = stored_node {
                            self.store.insert_measurement(&MeasurementRecord {
                                node_id: node.node_id,
                                kind: measurement_kind,
                                success: false,
                                value: None,
                                bytes_down,
                                bytes_up,
                                duration_ms,
                                trigger: if environment_available { "manual" } else { "environment" }.to_owned(),
                                rank_eligible: rank_eligible
                                    && environment_available
                                    && node.manual_override
                                    && node.state == ParticipationState::Participating,
                                error: Some(error_message),
                                created_at: unix_now(),
                            })?;
                        }
                    }
                }
                if !token.is_cancelled() {
                    wait_measurement_cooldown(&token).await;
                }
            }
            Result::<()>::Ok(())
        }
        .await;
        if let Err(error) = result {
            self.mark_manual_job_error(&job_id, &error.to_string());
        }
        if token.is_cancelled() {
            self.mark_queued_rows_cancelled(&job_id);
        }
        if let Some(job) = self.manual_jobs.write().get_mut(&job_id) {
            job.running = false;
        }
        self.manual_tokens.lock().remove(&job_id);
    }

    /// 更新一个手动测速节点的状态、错误和结果。
    fn update_manual_row(
        &self,
        job_id: &str,
        node_name: &str,
        status: &str,
        error: Option<&str>,
        result: Option<serde_json::Value>,
    ) {
        if let Some(job) = self.manual_jobs.write().get_mut(job_id)
            && let Some(row) = job.rows.iter_mut().find(|row| row.node_name == node_name)
        {
            row.status = status.to_owned();
            row.error = error.map(ToOwned::to_owned);
            row.result = result;
        }
    }

    /// 把任务级启动错误写到尚未执行的全部行。
    fn mark_manual_job_error(&self, job_id: &str, error: &str) {
        if let Some(job) = self.manual_jobs.write().get_mut(job_id) {
            for row in job
                .rows
                .iter_mut()
                .filter(|row| row.status == "queued" || row.status == "testing")
            {
                row.status = "failed".to_owned();
                row.error = Some(error.to_owned());
            }
        }
    }

    /// 把用户取消后尚未执行的行标记为已取消。
    fn mark_queued_rows_cancelled(&self, job_id: &str) {
        if let Some(job) = self.manual_jobs.write().get_mut(job_id) {
            for row in job
                .rows
                .iter_mut()
                .filter(|row| row.status == "queued" || row.status == "testing")
            {
                row.status = "cancelled".to_owned();
                row.error = None;
            }
        }
    }

    /// 原子更新一个后台队列，并同步兼容旧界面的汇总状态。
    fn set_worker_status(
        &self,
        worker: ScheduledWorker,
        phase: &str,
        profile_name: Option<&str>,
        node_name: Option<&str>,
        completed: usize,
        total: usize,
        error: Option<&str>,
    ) {
        let worker_status = BenchmarkWorkerStatus {
            running: true,
            phase: Some(phase.to_owned()),
            profile_name: profile_name.map(ToOwned::to_owned),
            node_name: node_name.map(ToOwned::to_owned),
            completed,
            total,
            last_error: error.map(ToOwned::to_owned),
        };
        let mut status = self.task_status.write();
        match worker {
            ScheduledWorker::Latency => status.latency = worker_status,
            ScheduledWorker::Speed => status.speed = worker_status,
        }
        status.running = true;
        status.cancellable = true;
        status.phase = Some(phase.to_owned());
        status.profile_name = profile_name.map(ToOwned::to_owned);
        status.node_name = node_name.map(ToOwned::to_owned);
        status.completed = completed;
        status.total = total;
        status.last_error = error
            .map(ToOwned::to_owned)
            .or_else(|| status.latency.last_error.clone())
            .or_else(|| status.speed.last_error.clone());
    }

    /// 结束一个后台队列；另一个队列仍运行时保持整体状态为运行中。
    fn finish_worker(&self, worker: ScheduledWorker, completed: usize, total: usize, phase: &str, error: Option<&str>) {
        let mut status = self.task_status.write();
        {
            let worker_status = match worker {
                ScheduledWorker::Latency => &mut status.latency,
                ScheduledWorker::Speed => &mut status.speed,
            };
            worker_status.running = false;
            worker_status.phase = Some(phase.to_owned());
            worker_status.completed = completed;
            worker_status.total = total;
            if let Some(error) = error {
                worker_status.last_error = Some(error.to_owned());
            }
        }
        let active = if status.latency.running {
            Some(status.latency.clone())
        } else if status.speed.running {
            Some(status.speed.clone())
        } else {
            None
        };
        status.running = active.is_some();
        status.cancellable = active.is_some();
        if let Some(active) = active {
            status.phase = active.phase;
            status.profile_name = active.profile_name;
            status.node_name = active.node_name;
            status.completed = active.completed;
            status.total = active.total;
        } else {
            status.phase = Some(phase.to_owned());
            status.profile_name = None;
            status.node_name = None;
            status.completed = completed;
            status.total = total;
        }
        status.last_error = error
            .map(ToOwned::to_owned)
            .or_else(|| status.latency.last_error.clone())
            .or_else(|| status.speed.last_error.clone());
    }
}

/// 按轮次循环移动队列起点，避免固定排在前面的订阅或节点长期占据网络时段优势。
const fn rotate_for_round<T>(items: &mut [T], round: u64) {
    if items.len() > 1 {
        let offset = round as usize % items.len();
        items.rotate_left(offset);
    }
}

/// 等待可取消的统一两秒静置时间。
async fn wait_measurement_cooldown(token: &CancellationToken) -> bool {
    tokio::select! {
        _ = token.cancelled() => false,
        _ = tokio::time::sleep(MEASUREMENT_COOLDOWN) => true,
    }
}

/// 构造长期评选统一使用的真实传输参数。
fn speed_options(url: &str, settings: &BenchmarkSettings) -> SpeedTestOptions {
    SpeedTestOptions {
        url: url.to_owned().into(),
        duration_ms: Some(settings.speed_duration_ms),
        max_bytes: Some(settings.speed_max_bytes),
        connect_timeout_ms: Some(8_000),
        read_idle_timeout_ms: Some(3_000),
    }
}

/// 为失败的上下行测试生成保留实际传输进度、但不伪造速度的数据记录。
fn failed_speed_record(
    node: &super::models::StoredNode,
    kind: MeasurementKind,
    bootstrap_stage: u8,
    duration_ms: u64,
    error: anyhow::Error,
) -> MeasurementRecord {
    let progress = error.downcast_ref::<SpeedTestFailure>();
    MeasurementRecord {
        node_id: node.node_id.clone(),
        kind,
        success: false,
        value: None,
        bytes_down: progress.map_or(0, |failure| failure.bytes_down),
        bytes_up: progress.map_or(0, |failure| failure.bytes_up),
        duration_ms: progress.map_or(duration_ms, |failure| failure.elapsed_ms),
        trigger: speed_trigger(bootstrap_stage).to_owned(),
        rank_eligible: true,
        error: Some(error.to_string()),
        created_at: unix_now(),
    }
}

/// 通过直连出口执行一次小流量真实传输，确认测速源本身能够完成相同方向的请求。
async fn probe_speed_environment(
    core: &BenchmarkCore,
    node: &super::models::StoredNode,
    kind: MeasurementKind,
    url: &str,
    settings: &BenchmarkSettings,
    token: &CancellationToken,
) -> bool {
    if token.is_cancelled() || core.select_node("DIRECT").await.is_err() {
        return false;
    }
    let mut options = speed_options(url, settings);
    options.duration_ms = Some(3_000);
    options.max_bytes = Some(1024 * 1024);
    let available = match kind {
        MeasurementKind::Download => tokio::select! {
            _ = token.cancelled() => false,
            result = test_download_speed_with_port(options, core.mixed_port()) => result.is_ok(),
        },
        MeasurementKind::Upload => tokio::select! {
            _ = token.cancelled() => false,
            result = test_upload_speed_with_port(options, core.mixed_port()) => result.is_ok(),
        },
        MeasurementKind::Latency => false,
    };
    let restored = core.select_node(&node.node_name).await.is_ok();
    available && restored
}

/// 使用独立内核的直连出口探测同一目标，区分节点失败与本地网络或测速源异常。
async fn probe_test_environment(core: &BenchmarkCore, url: &str, token: &CancellationToken) -> Option<bool> {
    tokio::select! {
        _ = token.cancelled() => None,
        result = core.test_delay("DIRECT", url, 5_000) => Some(result.is_ok()),
    }
}

/// 读取主 Mihomo 当前一秒总速率，网络繁忙或用户正在大流量传输时暂缓计划测速。
async fn main_network_is_busy(token: &CancellationToken) -> bool {
    let Ok(mut stream) = connections_stream::connect_traffic_stream().await else {
        return false;
    };
    let connection_id = stream.connection_id;
    let state = tokio::select! {
        _ = token.cancelled() => connections_stream::StreamConsumeState::ExitRequested,
        state = stream.next_event(Duration::from_millis(200), Duration::from_secs(2), || false) => state,
    };
    connections_stream::disconnect_connection(connection_id).await;
    matches!(
        state,
        connections_stream::StreamConsumeState::Event(speed)
            if speed.up.saturating_add(speed.down) >= BUSY_NETWORK_BYTES_PER_SECOND
    )
}

/// 返回快速评测或长期巡航对应的记录来源标签。
fn speed_trigger(bootstrap_stage: u8) -> &'static str {
    if bootstrap_stage < 3 { "bootstrap" } else { "scheduled" }
}

/// 按立即、一小时、累计四小时、之后固定周期计算下一次速度任务。
fn next_speed_schedule(bootstrap_stage: u8, now: i64, interval_hours: u64) -> (u8, i64) {
    match bootstrap_stage {
        0 => (1, now + 60 * 60),
        1 => (2, now + 3 * 60 * 60),
        2 => (3, now + interval_hours as i64 * 60 * 60),
        _ => (3, now + interval_hours as i64 * 60 * 60),
    }
}

/// 判断手动参数是否与正式评选完全一致，只有一致时才进入公平排名样本。
fn manual_matches_ranking(request: &ManualBatchRequest, settings: &BenchmarkSettings) -> bool {
    let expected_url = match request.mode {
        ManualSpeedMode::Download => &settings.download_url,
        ManualSpeedMode::Upload => &settings.upload_url,
    };
    request.include_in_ranking
        && request.options.url.trim() == expected_url
        && request.options.duration_ms.unwrap_or(5_000) == settings.speed_duration_ms
        && request.options.max_bytes.unwrap_or(32 * 1024 * 1024) == settings.speed_max_bytes
}

/// 返回当前 Unix 秒时间戳。
fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// 返回本地自然日开始时对应的 Unix 秒时间戳。
fn local_day_start() -> i64 {
    let now = Local::now();
    now.date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|value| Local.from_local_datetime(&value).earliest())
        .map(|value| value.timestamp())
        .unwrap_or_else(unix_now)
}

/// 返回下一个本地自然日开始时对应的 Unix 秒时间戳。
fn local_next_day_start() -> i64 {
    let now = Local::now();
    now.date_naive()
        .checked_add_days(Days::new(1))
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .and_then(|value| Local.from_local_datetime(&value).earliest())
        .map(|value| value.timestamp())
        .unwrap_or_else(|| local_day_start() + 24 * 60 * 60)
}

/// 返回距离下一个本地自然日的保守剩余秒数。
fn seconds_until_local_day_end() -> u64 {
    local_next_day_start().saturating_sub(unix_now()) as u64
}

#[cfg(test)]
mod tests {
    use super::{MEASUREMENT_COOLDOWN, next_speed_schedule, rotate_for_round};

    #[test]
    fn bootstrap_schedule_is_immediate_then_one_and_four_hours() {
        assert_eq!(next_speed_schedule(0, 100, 12), (1, 3700));
        assert_eq!(next_speed_schedule(1, 100, 12), (2, 10_900));
        assert_eq!(next_speed_schedule(2, 100, 12), (3, 43_300));
    }

    /// 每轮应循环移动节点起点，并保持用户确认的两秒静置时间。
    #[test]
    fn benchmark_round_rotates_starting_node_and_uses_two_second_cooldown() {
        let mut nodes = ["节点A", "节点B", "节点C"];
        rotate_for_round(&mut nodes, 1);

        assert_eq!(nodes, ["节点B", "节点C", "节点A"]);
        assert_eq!(MEASUREMENT_COOLDOWN.as_secs(), 2);
    }
}
