use super::models::{
    BenchmarkNodeRow, BenchmarkSettings, BenchmarkWindow, ChampionCollection, ChampionSummary, ExclusionReminder,
    LATENCY_ESTIMATED_DOWNLOAD_BYTES, LATENCY_ESTIMATED_UPLOAD_BYTES, MeasurementKind, MeasurementRecord,
    MetricSummary, ParticipationState, ProfileTrafficSummary, StoredNode, TrafficSummary,
};
use std::{cmp::Ordering, collections::HashMap};

/// 根据指定统计窗口生成主表、榜首卡片和延迟落后提醒。
pub fn build_rankings(
    nodes: &[StoredNode],
    measurements: &[MeasurementRecord],
    window: BenchmarkWindow,
    settings: &BenchmarkSettings,
    now: i64,
) -> (Vec<BenchmarkNodeRow>, ChampionCollection, Vec<ExclusionReminder>) {
    let start_at = window.start_timestamp(now);
    let rows = build_ranked_rows(nodes, measurements, start_at, settings, now);
    let champions = build_champions(&rows);
    let reminder_rows = if window == BenchmarkWindow::OneDay {
        rows.clone()
    } else {
        build_ranked_rows(
            nodes,
            measurements,
            BenchmarkWindow::OneDay.start_timestamp(now),
            settings,
            now,
        )
    };
    let reminders = build_reminders(nodes, &reminder_rows, measurements, settings, now);
    (rows, champions, reminders)
}

/// 为指定起点生成三项名次和综合名次分，供页面窗口与固定24小时提醒复用。
fn build_ranked_rows(
    nodes: &[StoredNode],
    measurements: &[MeasurementRecord],
    start_at: i64,
    settings: &BenchmarkSettings,
    now: i64,
) -> Vec<BenchmarkNodeRow> {
    let mut by_node: HashMap<&str, Vec<&MeasurementRecord>> = HashMap::new();
    for measurement in measurements
        .iter()
        .filter(|measurement| measurement.created_at >= start_at)
    {
        by_node.entry(&measurement.node_id).or_default().push(measurement);
    }

    let mut rows = nodes
        .iter()
        .map(|node| build_node_row(node, by_node.get(node.node_id.as_str()), settings, now))
        .collect::<Vec<_>>();
    assign_metric_ranks(&mut rows, MetricKind::Latency, false);
    assign_metric_ranks(&mut rows, MetricKind::Download, true);
    assign_metric_ranks(&mut rows, MetricKind::Upload, true);
    for row in &mut rows {
        row.total_rank_score = match (row.download.rank, row.upload.rank, row.latency.rank) {
            (Some(download), Some(upload), Some(latency)) => Some(download + upload + latency),
            _ => None,
        };
    }
    rows.sort_by(|left, right| {
        overall_row_order(left, right)
            .then_with(|| left.profile_name.cmp(&right.profile_name))
            .then_with(|| left.node_name.cmp(&right.node_name))
    });

    rows
}

/// 根据今日实耗、剩余时间和历史单次流量生成可解释的流量预估。
pub fn build_traffic_summary(
    nodes: &[StoredNode],
    measurements: &[MeasurementRecord],
    actual_by_profile: &[(String, String, u64, u64)],
    settings: &BenchmarkSettings,
    now: i64,
    seconds_until_day_end: u64,
) -> TrafficSummary {
    let mut profiles = actual_by_profile
        .iter()
        .map(|(uid, name, download, upload)| {
            (
                uid.clone(),
                ProfileTrafficSummary {
                    profile_uid: uid.clone(),
                    profile_name: name.clone(),
                    estimated_download_bytes: *download,
                    estimated_upload_bytes: *upload,
                    actual_download_bytes: *download,
                    actual_upload_bytes: *upload,
                    multiplier: settings.profile_traffic_multipliers.get(uid).copied().unwrap_or(1.0),
                    estimated_billed_bytes: 0,
                    actual_billed_bytes: 0,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let day_end = now.saturating_add(seconds_until_day_end as i64);

    for node in nodes.iter().filter(|node| {
        node.manual_override
            && matches!(
                node.state,
                ParticipationState::Participating | ParticipationState::Observe
            )
    }) {
        let remaining_latency_runs = planned_interval_runs(
            node.next_latency_at,
            settings.latency_interval_minutes.saturating_mul(60).max(1),
            now,
            day_end,
        );
        let profile = profiles
            .entry(node.profile_uid.clone())
            .or_insert_with(|| ProfileTrafficSummary {
                profile_uid: node.profile_uid.clone(),
                profile_name: node.profile_name.clone(),
                estimated_download_bytes: 0,
                estimated_upload_bytes: 0,
                actual_download_bytes: 0,
                actual_upload_bytes: 0,
                multiplier: settings
                    .profile_traffic_multipliers
                    .get(&node.profile_uid)
                    .copied()
                    .unwrap_or(1.0),
                estimated_billed_bytes: 0,
                actual_billed_bytes: 0,
            });
        profile.estimated_download_bytes = profile
            .estimated_download_bytes
            .saturating_add(LATENCY_ESTIMATED_DOWNLOAD_BYTES.saturating_mul(remaining_latency_runs));
        profile.estimated_upload_bytes = profile
            .estimated_upload_bytes
            .saturating_add(LATENCY_ESTIMATED_UPLOAD_BYTES.saturating_mul(remaining_latency_runs));

        if node.state == ParticipationState::Participating {
            let remaining_download_runs = planned_speed_runs(
                node.download_bootstrap_stage,
                node.next_download_at,
                settings,
                now,
                day_end,
            );
            let remaining_upload_runs =
                planned_speed_runs(node.upload_bootstrap_stage, node.next_upload_at, settings, now, day_end);
            let download_bytes = median_u64(
                measurements
                    .iter()
                    .filter(|measurement| {
                        measurement.node_id == node.node_id
                            && measurement.kind == MeasurementKind::Download
                            && measurement.success
                    })
                    .map(|measurement| measurement.bytes_down)
                    .filter(|bytes| *bytes > 0)
                    .collect(),
            )
            .unwrap_or(settings.speed_max_bytes);
            let upload_bytes = median_u64(
                measurements
                    .iter()
                    .filter(|measurement| {
                        measurement.node_id == node.node_id
                            && measurement.kind == MeasurementKind::Upload
                            && measurement.success
                    })
                    .map(|measurement| measurement.bytes_up)
                    .filter(|bytes| *bytes > 0)
                    .collect(),
            )
            .unwrap_or(settings.speed_max_bytes);
            profile.estimated_download_bytes = profile
                .estimated_download_bytes
                .saturating_add(download_bytes.saturating_mul(remaining_download_runs));
            profile.estimated_upload_bytes = profile
                .estimated_upload_bytes
                .saturating_add(upload_bytes.saturating_mul(remaining_upload_runs));
        }
    }

    let mut profile_rows = profiles.into_values().collect::<Vec<_>>();
    for profile in &mut profile_rows {
        let future_download = profile
            .estimated_download_bytes
            .saturating_sub(profile.actual_download_bytes);
        let future_upload = profile
            .estimated_upload_bytes
            .saturating_sub(profile.actual_upload_bytes);
        profile.estimated_download_bytes = profile
            .actual_download_bytes
            .saturating_add(add_safety_margin(future_download));
        profile.estimated_upload_bytes = profile
            .actual_upload_bytes
            .saturating_add(add_safety_margin(future_upload));
        profile.estimated_billed_bytes =
            ((profile.estimated_download_bytes + profile.estimated_upload_bytes) as f64 * profile.multiplier) as u64;
        profile.actual_billed_bytes =
            ((profile.actual_download_bytes + profile.actual_upload_bytes) as f64 * profile.multiplier) as u64;
    }
    profile_rows.sort_by(|left, right| left.profile_name.cmp(&right.profile_name));
    let actual_download_bytes = profile_rows.iter().map(|profile| profile.actual_download_bytes).sum();
    let actual_upload_bytes = profile_rows.iter().map(|profile| profile.actual_upload_bytes).sum();
    let estimated_download_bytes = profile_rows
        .iter()
        .map(|profile| profile.estimated_download_bytes)
        .sum();
    let estimated_upload_bytes = profile_rows.iter().map(|profile| profile.estimated_upload_bytes).sum();

    TrafficSummary {
        estimated_download_bytes,
        estimated_upload_bytes,
        actual_download_bytes,
        actual_upload_bytes,
        download_limit_bytes: settings.download_daily_limit_bytes,
        upload_limit_bytes: settings.upload_daily_limit_bytes,
        profiles: profile_rows,
    }
}

/// 计算一个固定周期任务在今天剩余时间内还会实际触发几次。
fn planned_interval_runs(next_at: i64, interval_seconds: u64, now: i64, day_end: i64) -> u64 {
    let first_at = next_at.max(now);
    if first_at >= day_end {
        return 0;
    }
    1 + day_end.saturating_sub(first_at + 1) as u64 / interval_seconds.max(1)
}

/// 计算单个速度方向包含立即、一小时和累计四小时快速阶段的剩余次数。
fn planned_speed_runs(
    bootstrap_stage: u8,
    scheduled_at: i64,
    settings: &BenchmarkSettings,
    now: i64,
    day_end: i64,
) -> u64 {
    let mut stage = bootstrap_stage;
    let mut next_at = scheduled_at.max(now);
    let mut runs = 0_u64;
    while next_at < day_end {
        runs += 1;
        let delay_seconds = match stage {
            0 => 60 * 60,
            1 => 3 * 60 * 60,
            _ => settings.speed_interval_hours.max(1) as i64 * 60 * 60,
        };
        stage = (stage + 1).min(3);
        next_at = next_at.saturating_add(delay_seconds);
    }
    runs
}

/// 根据单个节点在窗口内的记录生成一行透明统计数据。
fn build_node_row(
    node: &StoredNode,
    records: Option<&Vec<&MeasurementRecord>>,
    settings: &BenchmarkSettings,
    now: i64,
) -> BenchmarkNodeRow {
    let records = records.map(Vec::as_slice).unwrap_or_default();
    let latency_records = records
        .iter()
        .copied()
        .filter(|record| record.kind == MeasurementKind::Latency)
        .collect::<Vec<_>>();
    let eligible_latency_records = latency_records
        .iter()
        .copied()
        .filter(|record| record.rank_eligible)
        .collect::<Vec<_>>();
    let latency_attempts = eligible_latency_records.len();
    let latency_successes = eligible_latency_records.iter().filter(|record| record.success).count();
    let stability_percent =
        (latency_attempts > 0).then_some(latency_successes as f64 * 100.0 / latency_attempts as f64);
    let latency = metric_summary(&latency_records, false);
    let download_records = records
        .iter()
        .copied()
        .filter(|record| record.kind == MeasurementKind::Download)
        .collect::<Vec<_>>();
    let upload_records = records
        .iter()
        .copied()
        .filter(|record| record.kind == MeasurementKind::Upload)
        .collect::<Vec<_>>();
    let download = metric_summary(&download_records, true);
    let upload = metric_summary(&upload_records, true);
    let last_tested_at = records.iter().map(|record| record.created_at).max();
    let latest_latency = latency_records
        .iter()
        .filter(|record| record.rank_eligible && record.success)
        .map(|record| record.created_at)
        .max();
    let latest_download = download_records
        .iter()
        .filter(|record| record.rank_eligible && record.success)
        .map(|record| record.created_at)
        .max();
    let latest_upload = upload_records
        .iter()
        .filter(|record| record.rank_eligible && record.success)
        .map(|record| record.created_at)
        .max();
    let latency_fresh = latest_latency.is_some_and(|timestamp| {
        now - timestamp <= (settings.latency_interval_minutes.saturating_mul(120) as i64).max(60 * 60)
    });
    let download_fresh = latest_download.is_some_and(|timestamp| {
        now - timestamp <= (settings.speed_interval_hours.saturating_mul(7200) as i64).max(24 * 60 * 60)
    });
    let upload_fresh = latest_upload.is_some_and(|timestamp| {
        now - timestamp <= (settings.speed_interval_hours.saturating_mul(7200) as i64).max(24 * 60 * 60)
    });
    let official_eligible = node.state == ParticipationState::Participating
        && stability_percent.is_some_and(|value| value >= 95.0)
        && latency_successes >= 20
        && download.samples >= 2
        && upload.samples >= 2
        && latency_fresh
        && download_fresh
        && upload_fresh;

    BenchmarkNodeRow {
        node_id: node.node_id.clone(),
        profile_uid: node.profile_uid.clone(),
        profile_name: node.profile_name.clone(),
        node_name: node.node_name.clone(),
        node_type: node.node_type.clone(),
        fingerprint: node.fingerprint.clone(),
        last_seen_at: node.last_seen_at,
        state: node.state,
        stability_percent,
        latency_attempts,
        latency_successes,
        latency,
        download,
        upload,
        total_rank_score: None,
        official_eligible,
        confidence: confidence_label(node.first_seen_at, records, now),
        last_tested_at,
        bootstrap_stage: node.bootstrap_stage,
        download_bootstrap_stage: node.download_bootstrap_stage,
        upload_bootstrap_stage: node.upload_bootstrap_stage,
        next_latency_at: node.next_latency_at,
        next_speed_at: node.next_speed_at,
        next_download_at: node.next_download_at,
        next_upload_at: node.next_upload_at,
    }
}

/// 将成功记录转换为中位数、平均值、P95、峰值和样本数。
fn metric_summary(records: &[&MeasurementRecord], higher_is_better: bool) -> MetricSummary {
    let attempts = records.len();
    let failures = records.iter().filter(|record| !record.success).count();
    let last_attempted_at = records.iter().map(|record| record.created_at).max();
    let last_success_at = records
        .iter()
        .filter(|record| record.rank_eligible && record.success)
        .map(|record| record.created_at)
        .max();
    let latest_error = records
        .iter()
        .filter(|record| record.error.is_some())
        .max_by_key(|record| record.created_at);
    let unresolved_error =
        latest_error.filter(|record| last_success_at.is_none_or(|last_success| record.created_at >= last_success));
    let mut values = records
        .iter()
        .filter(|record| record.rank_eligible)
        .filter(|record| record.success)
        .filter_map(|record| record.value)
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return MetricSummary {
            attempts,
            failures,
            last_attempted_at,
            last_success_at,
            last_error: unresolved_error.and_then(|record| record.error.clone()),
            last_error_at: unresolved_error.map(|record| record.created_at),
            ..MetricSummary::default()
        };
    }
    values.sort_by(f64::total_cmp);
    let samples = values.len();
    let average = values.iter().sum::<f64>() / samples as f64;
    let peak = if higher_is_better {
        values.last().copied()
    } else {
        values.first().copied()
    };
    MetricSummary {
        median: percentile(&values, 0.5),
        average: Some(average),
        p95: percentile(&values, 0.95),
        peak,
        samples,
        attempts,
        failures,
        last_attempted_at,
        last_success_at,
        last_error: unresolved_error.and_then(|record| record.error.clone()),
        last_error_at: unresolved_error.map(|record| record.created_at),
        rank: None,
    }
}

/// 读取排序数组中的线性插值百分位数。
fn percentile(sorted: &[f64], ratio: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    let position = ratio.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        return sorted.get(lower).copied();
    }
    let weight = position - lower as f64;
    Some(sorted[lower] * (1.0 - weight) + sorted[upper] * weight)
}

/// 为指定指标按方向分配直观的顺序名次。
fn assign_metric_ranks(rows: &mut [BenchmarkNodeRow], kind: MetricKind, descending: bool) {
    let mut values = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| row.state == ParticipationState::Participating)
        .filter_map(|(index, row)| metric_for(row, kind).median.map(|value| (index, value)))
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        if descending {
            right.1.total_cmp(&left.1)
        } else {
            left.1.total_cmp(&right.1)
        }
    });
    for (position, (index, _)) in values.into_iter().enumerate() {
        metric_for_mut(&mut rows[index], kind).rank = Some(position + 1);
    }
}

/// 从一行数据读取指定指标。
const fn metric_for(row: &BenchmarkNodeRow, kind: MetricKind) -> &MetricSummary {
    match kind {
        MetricKind::Latency => &row.latency,
        MetricKind::Download => &row.download,
        MetricKind::Upload => &row.upload,
    }
}

/// 从一行数据可变读取指定指标。
const fn metric_for_mut(row: &mut BenchmarkNodeRow, kind: MetricKind) -> &mut MetricSummary {
    match kind {
        MetricKind::Latency => &mut row.latency,
        MetricKind::Download => &mut row.download,
        MetricKind::Upload => &mut row.upload,
    }
}

/// 生成综合、下载、上传和延迟四张榜首卡片。
fn build_champions(rows: &[BenchmarkNodeRow]) -> ChampionCollection {
    let overall_row = rows
        .iter()
        .filter(|row| row.state == ParticipationState::Participating && row.total_rank_score.is_some())
        .min_by(|left, right| {
            right
                .official_eligible
                .cmp(&left.official_eligible)
                .then_with(|| overall_row_order(left, right))
        });
    ChampionCollection {
        overall: overall_row.map(|row| champion(row, row.total_rank_score.unwrap_or_default() as f64)),
        download: best_metric_row(rows, MetricKind::Download, true)
            .and_then(|row| row.download.median.map(|value| champion(row, value))),
        upload: best_metric_row(rows, MetricKind::Upload, true)
            .and_then(|row| row.upload.median.map(|value| champion(row, value))),
        latency: best_metric_row(rows, MetricKind::Latency, false)
            .and_then(|row| row.latency.median.map(|value| champion(row, value))),
    }
}

/// 按综合名次分、稳定率、延迟波动、样本量和新鲜度依次比较两行。
fn overall_row_order(left: &BenchmarkNodeRow, right: &BenchmarkNodeRow) -> Ordering {
    let left_jitter = left.latency.p95.unwrap_or(f64::MAX) - left.latency.median.unwrap_or(0.0);
    let right_jitter = right.latency.p95.unwrap_or(f64::MAX) - right.latency.median.unwrap_or(0.0);
    let left_samples = left.latency.samples + left.download.samples + left.upload.samples;
    let right_samples = right.latency.samples + right.download.samples + right.upload.samples;
    left.total_rank_score
        .unwrap_or(usize::MAX)
        .cmp(&right.total_rank_score.unwrap_or(usize::MAX))
        .then_with(|| {
            right
                .stability_percent
                .unwrap_or_default()
                .total_cmp(&left.stability_percent.unwrap_or_default())
        })
        .then_with(|| left_jitter.total_cmp(&right_jitter))
        .then_with(|| right_samples.cmp(&left_samples))
        .then_with(|| {
            right
                .last_tested_at
                .unwrap_or_default()
                .cmp(&left.last_tested_at.unwrap_or_default())
        })
}

/// 选择指定指标的第一名节点。
fn best_metric_row(rows: &[BenchmarkNodeRow], kind: MetricKind, descending: bool) -> Option<&BenchmarkNodeRow> {
    rows.iter()
        .filter(|row| row.state == ParticipationState::Participating && metric_for(row, kind).median.is_some())
        .min_by(|left, right| {
            let left_value = metric_for(left, kind).median.unwrap_or_default();
            let right_value = metric_for(right, kind).median.unwrap_or_default();
            if descending {
                right_value.total_cmp(&left_value)
            } else {
                left_value.total_cmp(&right_value)
            }
        })
}

/// 将主表行转换为榜首摘要并保留来源订阅。
fn champion(row: &BenchmarkNodeRow, value: f64) -> ChampionSummary {
    ChampionSummary {
        node_id: row.node_id.clone(),
        node_name: row.node_name.clone(),
        profile_name: row.profile_name.clone(),
        value,
        official: row.official_eligible,
    }
}

/// 按24小时观察时长、后25%和明显差距规则生成需人工确认的建议。
fn build_reminders(
    nodes: &[StoredNode],
    rows: &[BenchmarkNodeRow],
    measurements: &[MeasurementRecord],
    settings: &BenchmarkSettings,
    now: i64,
) -> Vec<ExclusionReminder> {
    let eligible = rows
        .iter()
        .filter(|row| row.state == ParticipationState::Participating && row.latency.median.is_some())
        .collect::<Vec<_>>();
    if eligible.len() < 4 {
        return Vec::new();
    }
    let mut latencies = eligible.iter().filter_map(|row| row.latency.median).collect::<Vec<_>>();
    latencies.sort_by(f64::total_cmp);
    let excellent_count = latencies.len().div_ceil(5).max(1);
    let reference_latency = percentile(&latencies[..excellent_count], 0.5).unwrap_or_default();
    let bottom_start = eligible.len().saturating_mul(3).div_ceil(4);
    let total_nodes = eligible.len();

    eligible
        .into_iter()
        .filter_map(|row| {
            let node = nodes.iter().find(|node| node.node_id == row.node_id)?;
            let rank = row.latency.rank?;
            let median_latency = row.latency.median?;
            if now - node.first_seen_at < 24 * 60 * 60
                || row.latency_successes < 20
                || rank <= bottom_start
                || node.ignore_reminder
                || node.snoozed_until.is_some_and(|until| until > now)
                || (median_latency < reference_latency + 80.0 && median_latency < reference_latency * 2.0)
            {
                return None;
            }
            let speed_runs = 24_u64.div_ceil(settings.speed_interval_hours.max(1));
            let saved_download_bytes_per_day = median_u64(
                measurements
                    .iter()
                    .filter(|measurement| {
                        measurement.node_id == row.node_id
                            && measurement.kind == MeasurementKind::Download
                            && measurement.success
                    })
                    .map(|measurement| measurement.bytes_down)
                    .collect(),
            )
            .unwrap_or(settings.speed_max_bytes)
            .saturating_mul(speed_runs);
            let saved_upload_bytes_per_day = median_u64(
                measurements
                    .iter()
                    .filter(|measurement| {
                        measurement.node_id == row.node_id
                            && measurement.kind == MeasurementKind::Upload
                            && measurement.success
                    })
                    .map(|measurement| measurement.bytes_up)
                    .collect(),
            )
            .unwrap_or(settings.speed_max_bytes)
            .saturating_mul(speed_runs);
            Some(ExclusionReminder {
                node_id: row.node_id.clone(),
                node_name: row.node_name.clone(),
                profile_name: row.profile_name.clone(),
                latency_rank: rank,
                total_nodes,
                median_latency,
                reference_latency,
                download_rank: row.download.rank,
                upload_rank: row.upload.rank,
                stability_percent: row.stability_percent,
                saved_download_bytes_per_day,
                saved_upload_bytes_per_day,
            })
        })
        .collect()
}

/// 根据观察时长和速度样本数量返回用户能直接理解的数据状态。
fn confidence_label(first_seen_at: i64, records: &[&MeasurementRecord], now: i64) -> String {
    let download_samples = records
        .iter()
        .filter(|record| record.rank_eligible && record.success && record.kind == MeasurementKind::Download)
        .count();
    let upload_samples = records
        .iter()
        .filter(|record| record.rank_eligible && record.success && record.kind == MeasurementKind::Upload)
        .count();
    let age = now.saturating_sub(first_seen_at);
    if age >= 7 * 24 * 60 * 60 && download_samples >= 4 && upload_samples >= 4 {
        "longTerm".to_owned()
    } else if age >= 24 * 60 * 60 && download_samples >= 2 && upload_samples >= 2 {
        "daily".to_owned()
    } else if age >= 4 * 60 * 60 && download_samples >= 1 && upload_samples >= 1 {
        "initial".to_owned()
    } else {
        "temporary".to_owned()
    }
}

/// 计算无符号整数样本的中位数。
fn median_u64(mut values: Vec<u64>) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        Some(values[middle - 1].saturating_add(values[middle]) / 2)
    } else {
        Some(values[middle])
    }
}

/// 为协议、握手和加密开销增加10%的保守余量。
const fn add_safety_margin(bytes: u64) -> u64 {
    bytes.saturating_add(bytes / 10)
}

#[derive(Clone, Copy)]
enum MetricKind {
    Latency,
    Download,
    Upload,
}

#[cfg(test)]
mod tests {
    use super::{confidence_label, median_u64, percentile};
    use crate::feat::node_benchmark::models::{MeasurementKind, MeasurementRecord};

    #[test]
    fn percentile_interpolates_sorted_samples() {
        assert_eq!(percentile(&[10.0, 20.0, 30.0, 40.0], 0.5), Some(25.0));
        assert_eq!(percentile(&[10.0], 0.95), Some(10.0));
    }

    #[test]
    fn integer_median_handles_even_and_odd_samples() {
        assert_eq!(median_u64(vec![3, 1, 2]), Some(2));
        assert_eq!(median_u64(vec![4, 2, 1, 3]), Some(2));
    }

    #[test]
    fn confidence_requires_successes_from_both_speed_directions() {
        let upload = MeasurementRecord {
            node_id: "node-a".to_owned(),
            kind: MeasurementKind::Upload,
            success: true,
            value: Some(1.0),
            bytes_down: 0,
            bytes_up: 1,
            duration_ms: 1,
            trigger: "bootstrap".to_owned(),
            rank_eligible: true,
            error: None,
            created_at: 100,
        };
        let now = 5 * 60 * 60;
        assert_eq!(confidence_label(0, &[&upload, &upload], now), "temporary");

        let download = MeasurementRecord {
            kind: MeasurementKind::Download,
            bytes_down: 1,
            bytes_up: 0,
            ..upload.clone()
        };
        assert_eq!(confidence_label(0, &[&download, &upload], now), "initial");
    }
}
