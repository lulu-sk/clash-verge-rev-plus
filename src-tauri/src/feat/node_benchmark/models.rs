use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 延迟测试使用的保守下载流量估算值。
pub const LATENCY_ESTIMATED_DOWNLOAD_BYTES: u64 = 16 * 1024;
/// 延迟测试使用的保守上传流量估算值。
pub const LATENCY_ESTIMATED_UPLOAD_BYTES: u64 = 4 * 1024;

/// Cloudflare 单次下载测速允许的稳定请求大小。
pub const DEFAULT_DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=50000000";

/// 早期版本使用但会被 Cloudflare 拒绝的下载测速地址。
const LEGACY_DOWNLOAD_URL: &str = "https://speed.cloudflare.com/__down?bytes=1000000000";

/// 节点评选的全局设置。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BenchmarkSettings {
    pub enabled: bool,
    pub selected_profile_uids: Vec<String>,
    pub include_keywords: String,
    pub exclude_keywords: String,
    pub case_sensitive: bool,
    pub latency_interval_minutes: u64,
    pub speed_interval_hours: u64,
    pub latency_url: String,
    pub download_url: String,
    pub upload_url: String,
    pub download_daily_limit_bytes: u64,
    pub upload_daily_limit_bytes: u64,
    pub profile_traffic_multipliers: HashMap<String, f64>,
    pub speed_duration_ms: u64,
    pub speed_max_bytes: u64,
}

impl Default for BenchmarkSettings {
    /// 返回兼顾准确度和默认流量保护的初始设置。
    fn default() -> Self {
        Self {
            enabled: false,
            selected_profile_uids: Vec::new(),
            include_keywords: String::new(),
            exclude_keywords: String::new(),
            case_sensitive: false,
            latency_interval_minutes: 15,
            speed_interval_hours: 12,
            latency_url: "https://www.gstatic.com/generate_204".to_owned(),
            download_url: DEFAULT_DOWNLOAD_URL.to_owned(),
            upload_url: "https://speed.cloudflare.com/__up".to_owned(),
            download_daily_limit_bytes: 10 * 1024 * 1024 * 1024,
            upload_daily_limit_bytes: 3 * 1024 * 1024 * 1024,
            profile_traffic_multipliers: HashMap::new(),
            speed_duration_ms: 12_000,
            speed_max_bytes: 1024 * 1024 * 1024,
        }
    }
}

impl BenchmarkSettings {
    /// 校验并规范化用户提交的设置。
    pub fn normalized(mut self) -> anyhow::Result<Self> {
        self.latency_interval_minutes = self.latency_interval_minutes.clamp(5, 60);
        self.speed_interval_hours = self.speed_interval_hours.clamp(1, 168);
        self.speed_duration_ms = self.speed_duration_ms.clamp(3_000, 30_000);
        self.speed_max_bytes = self.speed_max_bytes.clamp(8 * 1024 * 1024, 2 * 1024 * 1024 * 1024);
        self.selected_profile_uids.sort();
        self.selected_profile_uids.dedup();
        self.profile_traffic_multipliers
            .retain(|_, multiplier| multiplier.is_finite() && *multiplier > 0.0);
        for multiplier in self.profile_traffic_multipliers.values_mut() {
            *multiplier = multiplier.clamp(0.1, 100.0);
        }

        if self.download_url.trim() == LEGACY_DOWNLOAD_URL {
            self.download_url = DEFAULT_DOWNLOAD_URL.to_owned();
        }

        for (label, url) in [
            ("延迟测试地址", &self.latency_url),
            ("下载测速地址", &self.download_url),
            ("上传测速地址", &self.upload_url),
        ] {
            let parsed = reqwest::Url::parse(url.trim()).map_err(|error| anyhow::anyhow!("{label}无效: {error}"))?;
            if !matches!(parsed.scheme(), "http" | "https") {
                anyhow::bail!("{label}只支持 HTTP 或 HTTPS")
            }
        }

        self.latency_url = self.latency_url.trim().to_owned();
        self.download_url = self.download_url.trim().to_owned();
        self.upload_url = self.upload_url.trim().to_owned();
        Ok(self)
    }
}

/// 节点参与评选的状态。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ParticipationState {
    Participating,
    Observe,
    Excluded,
    Changed,
    Removed,
}

impl ParticipationState {
    /// 返回数据库使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Participating => "participating",
            Self::Observe => "observe",
            Self::Excluded => "excluded",
            Self::Changed => "changed",
            Self::Removed => "removed",
        }
    }

    /// 从数据库字符串恢复节点状态。
    pub fn from_db(value: &str) -> Self {
        match value {
            "participating" => Self::Participating,
            "observe" => Self::Observe,
            "excluded" => Self::Excluded,
            "changed" => Self::Changed,
            _ => Self::Removed,
        }
    }
}

/// 支持的统计时间窗口。
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BenchmarkWindow {
    SixHours,
    TwelveHours,
    OneDay,
    SevenDays,
    LongTerm,
}

impl BenchmarkWindow {
    /// 返回时间窗口对应的起始时间戳，长期窗口不限制起点。
    pub const fn start_timestamp(self, now: i64) -> i64 {
        match self {
            Self::SixHours => now - 6 * 60 * 60,
            Self::TwelveHours => now - 12 * 60 * 60,
            Self::OneDay => now - 24 * 60 * 60,
            Self::SevenDays => now - 7 * 24 * 60 * 60,
            Self::LongTerm => 0,
        }
    }
}

/// 可供用户选择的订阅摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkProfileSummary {
    pub uid: String,
    pub name: String,
    pub available: bool,
    pub selected: bool,
    pub node_count: usize,
    pub scan_error: Option<String>,
    pub last_scanned_at: Option<i64>,
}

/// 从订阅配置中发现的节点。
#[derive(Debug, Clone)]
pub struct DiscoveredNode {
    pub node_id: String,
    pub profile_uid: String,
    pub profile_name: String,
    pub node_name: String,
    pub node_type: String,
    pub fingerprint: String,
}

/// 数据库中的节点调度记录。
#[derive(Debug, Clone)]
pub struct StoredNode {
    pub node_id: String,
    pub profile_uid: String,
    pub profile_name: String,
    pub node_name: String,
    pub node_type: String,
    pub fingerprint: String,
    pub state: ParticipationState,
    pub manual_override: bool,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub bootstrap_stage: u8,
    pub next_latency_at: i64,
    pub next_speed_at: i64,
    pub download_bootstrap_stage: u8,
    pub upload_bootstrap_stage: u8,
    pub next_download_at: i64,
    pub next_upload_at: i64,
    pub snoozed_until: Option<i64>,
    pub ignore_reminder: bool,
}

/// 单次测量的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementKind {
    Latency,
    Download,
    Upload,
}

impl MeasurementKind {
    /// 返回数据库使用的稳定字符串。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Latency => "latency",
            Self::Download => "download",
            Self::Upload => "upload",
        }
    }

    /// 从数据库字符串恢复测量类型。
    pub fn from_db(value: &str) -> Self {
        match value {
            "download" => Self::Download,
            "upload" => Self::Upload,
            _ => Self::Latency,
        }
    }
}

/// 单次测试结果记录。
#[derive(Debug, Clone)]
pub struct MeasurementRecord {
    pub node_id: String,
    pub kind: MeasurementKind,
    pub success: bool,
    pub value: Option<f64>,
    pub bytes_down: u64,
    pub bytes_up: u64,
    pub duration_ms: u64,
    pub trigger: String,
    pub rank_eligible: bool,
    pub error: Option<String>,
    pub created_at: i64,
}

/// 某项指标在当前窗口内的统计结果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricSummary {
    pub median: Option<f64>,
    pub average: Option<f64>,
    pub p95: Option<f64>,
    pub peak: Option<f64>,
    pub samples: usize,
    pub attempts: usize,
    pub failures: usize,
    pub last_attempted_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub last_error_at: Option<i64>,
    pub rank: Option<usize>,
}

/// 面板主表中的节点行。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkNodeRow {
    pub node_id: String,
    pub profile_uid: String,
    pub profile_name: String,
    pub node_name: String,
    pub node_type: String,
    pub fingerprint: String,
    pub last_seen_at: i64,
    pub state: ParticipationState,
    pub stability_percent: Option<f64>,
    pub latency_attempts: usize,
    pub latency_successes: usize,
    pub latency: MetricSummary,
    pub download: MetricSummary,
    pub upload: MetricSummary,
    pub total_rank_score: Option<usize>,
    pub official_eligible: bool,
    pub confidence: String,
    pub last_tested_at: Option<i64>,
    pub bootstrap_stage: u8,
    pub download_bootstrap_stage: u8,
    pub upload_bootstrap_stage: u8,
    pub next_latency_at: i64,
    pub next_speed_at: i64,
    pub next_download_at: i64,
    pub next_upload_at: i64,
}

/// 榜首卡片使用的节点摘要。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChampionSummary {
    pub node_id: String,
    pub node_name: String,
    pub profile_name: String,
    pub value: f64,
    pub official: bool,
}

/// 四个直观榜首结论。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChampionCollection {
    pub overall: Option<ChampionSummary>,
    pub download: Option<ChampionSummary>,
    pub upload: Option<ChampionSummary>,
    pub latency: Option<ChampionSummary>,
}

/// 单个订阅的流量统计。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTrafficSummary {
    pub profile_uid: String,
    pub profile_name: String,
    pub estimated_download_bytes: u64,
    pub estimated_upload_bytes: u64,
    pub actual_download_bytes: u64,
    pub actual_upload_bytes: u64,
    pub multiplier: f64,
    pub estimated_billed_bytes: u64,
    pub actual_billed_bytes: u64,
}

/// 今日流量预估、实耗和硬上限。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrafficSummary {
    pub estimated_download_bytes: u64,
    pub estimated_upload_bytes: u64,
    pub actual_download_bytes: u64,
    pub actual_upload_bytes: u64,
    pub download_limit_bytes: u64,
    pub upload_limit_bytes: u64,
    pub profiles: Vec<ProfileTrafficSummary>,
}

/// 延迟长期落后节点的人工审核建议。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExclusionReminder {
    pub node_id: String,
    pub node_name: String,
    pub profile_name: String,
    pub latency_rank: usize,
    pub total_nodes: usize,
    pub median_latency: f64,
    pub reference_latency: f64,
    pub download_rank: Option<usize>,
    pub upload_rank: Option<usize>,
    pub stability_percent: Option<f64>,
    pub saved_download_bytes_per_day: u64,
    pub saved_upload_bytes_per_day: u64,
}

/// 订阅扫描产生的最近变化。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkChangeEvent {
    pub id: i64,
    pub profile_uid: String,
    pub profile_name: String,
    pub node_name: Option<String>,
    pub change_type: String,
    pub created_at: i64,
}

/// 一个独立后台队列的当前执行状态。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkWorkerStatus {
    pub running: bool,
    pub phase: Option<String>,
    pub profile_name: Option<String>,
    pub node_name: Option<String>,
    pub completed: usize,
    pub total: usize,
    pub last_error: Option<String>,
}

/// 后台当前任务状态，同时保留兼容旧界面的汇总字段。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkTaskStatus {
    pub running: bool,
    pub cancellable: bool,
    pub phase: Option<String>,
    pub profile_name: Option<String>,
    pub node_name: Option<String>,
    pub completed: usize,
    pub total: usize,
    pub last_error: Option<String>,
    pub latency: BenchmarkWorkerStatus,
    pub speed: BenchmarkWorkerStatus,
}

/// 面板一次读取所需的完整快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSnapshot {
    pub settings: BenchmarkSettings,
    pub selection_confirmed: bool,
    pub profiles: Vec<BenchmarkProfileSummary>,
    pub nodes: Vec<BenchmarkNodeRow>,
    pub champions: ChampionCollection,
    pub traffic: TrafficSummary,
    pub reminders: Vec<ExclusionReminder>,
    pub changes: Vec<BenchmarkChangeEvent>,
    pub task: BenchmarkTaskStatus,
    pub generated_at: i64,
}

/// 保存前节点预览中的轻量候选节点。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkCandidateNode {
    pub node_id: String,
    pub profile_uid: String,
    pub profile_name: String,
    pub node_name: String,
    pub node_type: String,
    pub fingerprint: String,
}

/// 保存前预览结果，同时返回后台规范化后的候选范围设置。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkPreview {
    pub settings: BenchmarkSettings,
    pub nodes: Vec<BenchmarkCandidateNode>,
}

/// 一次性保存设置和用户最终勾选的参赛节点。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BenchmarkSaveRequest {
    pub settings: BenchmarkSettings,
    pub participating_node_ids: Vec<String>,
}

/// 节点状态或提醒策略更新请求。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeUpdateRequest {
    pub node_id: String,
    pub state: Option<ParticipationState>,
    pub reminder_action: Option<ReminderAction>,
}

/// 用户对延迟落后提醒采取的操作。
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReminderAction {
    SnoozeSevenDays,
    Ignore,
    Reset,
}

/// 手动批量测速的方向。
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManualSpeedMode {
    Download,
    Upload,
}

/// 手动批量测速启动参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualBatchRequest {
    pub profile_uid: Option<String>,
    pub node_names: Vec<String>,
    pub mode: ManualSpeedMode,
    pub options: crate::feat::SpeedTestOptions,
    #[serde(default = "default_true")]
    pub include_in_ranking: bool,
}

/// 为兼容旧前端请求提供默认启用值。
const fn default_true() -> bool {
    true
}

/// 手动测速单行结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualBatchRow {
    pub node_name: String,
    pub status: String,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

/// 手动批量测速任务快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManualBatchSnapshot {
    pub job_id: String,
    pub running: bool,
    pub mode: ManualSpeedMode,
    pub rows: Vec<ManualBatchRow>,
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "测试通过失败即终止来表达断言")]
mod tests {
    use super::{BenchmarkSettings, DEFAULT_DOWNLOAD_URL, LEGACY_DOWNLOAD_URL};

    #[test]
    fn legacy_cloudflare_download_url_is_migrated() {
        let settings = BenchmarkSettings {
            download_url: LEGACY_DOWNLOAD_URL.to_owned(),
            ..BenchmarkSettings::default()
        };

        let normalized = settings.normalized().expect("旧下载地址应可自动迁移");

        assert_eq!(normalized.download_url, DEFAULT_DOWNLOAD_URL);
    }
}
