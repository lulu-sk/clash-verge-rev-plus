use super::models::{
    BenchmarkChangeEvent, BenchmarkProfileSummary, BenchmarkSettings, DiscoveredNode, MeasurementKind,
    MeasurementRecord, ParticipationState, ReminderAction, StoredNode,
};
use anyhow::{Context as _, Result};
use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension as _, ToSql, params};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

const MISSING_PROFILE_ERROR: &str = "所选订阅已从配置列表中移除";

/// 节点评选 SQLite 存储。
pub struct BenchmarkStore {
    connection: Mutex<Connection>,
}

impl BenchmarkStore {
    /// 打开数据库并完成幂等结构迁移。
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建节点评选数据库目录: {}", parent.display()))?;
        }

        let connection =
            Connection::open(path).with_context(|| format!("无法打开节点评选数据库: {}", path.display()))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            r"
            CREATE TABLE IF NOT EXISTS benchmark_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                json TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS benchmark_profiles (
                profile_uid TEXT PRIMARY KEY,
                profile_name TEXT NOT NULL,
                source_hash TEXT,
                last_scanned_at INTEGER,
                scan_error TEXT
            );

            CREATE TABLE IF NOT EXISTS benchmark_nodes (
                node_id TEXT PRIMARY KEY,
                profile_uid TEXT NOT NULL,
                profile_name TEXT NOT NULL,
                node_name TEXT NOT NULL,
                node_type TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                state TEXT NOT NULL,
                manual_override INTEGER NOT NULL DEFAULT 0,
                active INTEGER NOT NULL DEFAULT 1,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                bootstrap_stage INTEGER NOT NULL DEFAULT 0,
                next_latency_at INTEGER NOT NULL,
                next_speed_at INTEGER NOT NULL,
                download_bootstrap_stage INTEGER NOT NULL DEFAULT 0,
                upload_bootstrap_stage INTEGER NOT NULL DEFAULT 0,
                next_download_at INTEGER NOT NULL DEFAULT 0,
                next_upload_at INTEGER NOT NULL DEFAULT 0,
                snoozed_until INTEGER,
                ignore_reminder INTEGER NOT NULL DEFAULT 0
            );

            CREATE INDEX IF NOT EXISTS idx_benchmark_nodes_profile
                ON benchmark_nodes(profile_uid, active);
            CREATE INDEX IF NOT EXISTS idx_benchmark_nodes_schedule
                ON benchmark_nodes(active, next_latency_at, next_speed_at);

            CREATE TABLE IF NOT EXISTS benchmark_measurements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                node_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                success INTEGER NOT NULL,
                value REAL,
                bytes_down INTEGER NOT NULL DEFAULT 0,
                bytes_up INTEGER NOT NULL DEFAULT 0,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                trigger_kind TEXT NOT NULL,
                rank_eligible INTEGER NOT NULL DEFAULT 1,
                error TEXT,
                created_at INTEGER NOT NULL,
                FOREIGN KEY(node_id) REFERENCES benchmark_nodes(node_id)
            );

            CREATE INDEX IF NOT EXISTS idx_benchmark_measurements_window
                ON benchmark_measurements(created_at, node_id, kind);
            CREATE INDEX IF NOT EXISTS idx_benchmark_measurements_node
                ON benchmark_measurements(node_id, kind, created_at);

            CREATE TABLE IF NOT EXISTS benchmark_changes (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                profile_uid TEXT NOT NULL,
                profile_name TEXT NOT NULL,
                node_name TEXT,
                change_type TEXT NOT NULL,
                relevant INTEGER NOT NULL DEFAULT 1,
                created_at INTEGER NOT NULL
            );
            ",
        )?;

        let has_rank_eligible = connection
            .prepare("PRAGMA table_info(benchmark_measurements)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .iter()
            .any(|column| column == "rank_eligible");
        if !has_rank_eligible {
            connection.execute(
                "ALTER TABLE benchmark_measurements ADD COLUMN rank_eligible INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }

        let node_columns = connection
            .prepare("PRAGMA table_info(benchmark_nodes)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        if !node_columns.contains("download_bootstrap_stage") {
            connection.execute(
                "ALTER TABLE benchmark_nodes ADD COLUMN download_bootstrap_stage INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            connection.execute(
                "UPDATE benchmark_nodes SET download_bootstrap_stage = bootstrap_stage",
                [],
            )?;
        }
        if !node_columns.contains("upload_bootstrap_stage") {
            connection.execute(
                "ALTER TABLE benchmark_nodes ADD COLUMN upload_bootstrap_stage INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            connection.execute(
                "UPDATE benchmark_nodes SET upload_bootstrap_stage = bootstrap_stage",
                [],
            )?;
        }
        if !node_columns.contains("next_download_at") {
            connection.execute(
                "ALTER TABLE benchmark_nodes ADD COLUMN next_download_at INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            connection.execute("UPDATE benchmark_nodes SET next_download_at = next_speed_at", [])?;
        }
        if !node_columns.contains("next_upload_at") {
            connection.execute(
                "ALTER TABLE benchmark_nodes ADD COLUMN next_upload_at INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
            connection.execute("UPDATE benchmark_nodes SET next_upload_at = next_speed_at", [])?;
        }
        let change_columns = connection
            .prepare("PRAGMA table_info(benchmark_changes)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        if !change_columns.contains("relevant") {
            connection.execute(
                "ALTER TABLE benchmark_changes ADD COLUMN relevant INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
            connection.execute(
                "UPDATE benchmark_changes SET relevant = 0 WHERE node_name IS NOT NULL AND EXISTS (\
                 SELECT 1 FROM benchmark_nodes n WHERE n.profile_uid = benchmark_changes.profile_uid \
                 AND n.node_name = benchmark_changes.node_name AND n.state = 'excluded')",
                [],
            )?;
        }
        connection.execute(
            "CREATE INDEX IF NOT EXISTS idx_benchmark_nodes_direction_schedule ON benchmark_nodes(\
             active, manual_override, next_latency_at, next_download_at, next_upload_at)",
            [],
        )?;

        Ok(Self {
            connection: Mutex::new(connection),
        })
    }

    /// 读取设置；数据库尚未保存时返回默认设置。
    pub fn load_settings(&self) -> Result<BenchmarkSettings> {
        let connection = self.connection.lock();
        let json = connection
            .query_row("SELECT json FROM benchmark_settings WHERE id = 1", [], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        drop(connection);
        json.map_or_else(
            || Ok(BenchmarkSettings::default()),
            |value| {
                serde_json::from_str::<BenchmarkSettings>(&value)
                    .context("节点评选设置无法解析")?
                    .normalized()
            },
        )
    }

    /// 原子保存完整设置。
    pub fn save_settings(&self, settings: &BenchmarkSettings) -> Result<()> {
        let json = serde_json::to_string(settings)?;
        self.connection.lock().execute(
            "INSERT INTO benchmark_settings(id, json) VALUES(1, ?1) \
             ON CONFLICT(id) DO UPDATE SET json = excluded.json",
            [json],
        )?;
        Ok(())
    }

    /// 保存订阅扫描状态和错误信息。
    pub fn save_profile_scan(
        &self,
        profile_uid: &str,
        profile_name: &str,
        source_hash: Option<&str>,
        scanned_at: i64,
        scan_error: Option<&str>,
    ) -> Result<()> {
        let mut connection = self.connection.lock();
        let previous_name = connection
            .query_row(
                "SELECT profile_name FROM benchmark_profiles WHERE profile_uid = ?1",
                [profile_uid],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            r"
            INSERT INTO benchmark_profiles(profile_uid, profile_name, source_hash, last_scanned_at, scan_error)
            VALUES(?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(profile_uid) DO UPDATE SET
                profile_name = excluded.profile_name,
                source_hash = COALESCE(excluded.source_hash, benchmark_profiles.source_hash),
                last_scanned_at = excluded.last_scanned_at,
                scan_error = excluded.scan_error
            ",
            params![profile_uid, profile_name, source_hash, scanned_at, scan_error],
        )?;
        if let Some(previous_name) = previous_name
            && previous_name != profile_name
        {
            insert_change(
                &transaction,
                profile_uid,
                profile_name,
                None,
                "profileRenamed",
                true,
                scanned_at,
            )?;
        }
        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    /// 将仍在设置中但已从配置列表消失的订阅节点标记为移除。
    pub fn mark_profile_missing(&self, profile_uid: &str, fallback_name: &str, now: i64) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let previous = transaction
            .query_row(
                "SELECT profile_name, scan_error FROM benchmark_profiles WHERE profile_uid = ?1",
                [profile_uid],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .optional()?;
        let profile_name = previous
            .as_ref()
            .map(|value| value.0.clone())
            .unwrap_or_else(|| fallback_name.to_owned());
        let was_missing = previous
            .as_ref()
            .and_then(|value| value.1.as_deref())
            .is_some_and(|error| error == MISSING_PROFILE_ERROR);
        transaction.execute(
            r"
            INSERT INTO benchmark_profiles(profile_uid, profile_name, last_scanned_at, scan_error)
            VALUES(?1, ?2, ?3, ?4)
            ON CONFLICT(profile_uid) DO UPDATE SET
                last_scanned_at = excluded.last_scanned_at,
                scan_error = excluded.scan_error
            ",
            params![profile_uid, profile_name, now, MISSING_PROFILE_ERROR],
        )?;
        let changed = transaction.execute(
            "UPDATE benchmark_nodes SET active = 0, state = 'removed', last_seen_at = ?2 \
             WHERE profile_uid = ?1 AND active = 1",
            params![profile_uid, now],
        )?;
        if !was_missing || changed > 0 {
            insert_change(
                &transaction,
                profile_uid,
                &profile_name,
                None,
                "profileRemoved",
                true,
                now,
            )?;
        }
        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    /// 将一次订阅发现结果与现有节点状态进行差异合并。
    pub fn reconcile_nodes(
        &self,
        profile_uid: &str,
        profile_name: &str,
        discovered: &[DiscoveredNode],
        settings: &BenchmarkSettings,
        now: i64,
    ) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let existing = {
            let mut statement = transaction.prepare(
                "SELECT node_id, node_name, state, manual_override FROM benchmark_nodes \
                 WHERE profile_uid = ?1 AND active = 1",
            )?;
            statement
                .query_map([profile_uid], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)? != 0,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let discovered_ids = discovered
            .iter()
            .map(|node| node.node_id.as_str())
            .collect::<HashSet<_>>();

        for node in discovered {
            let same_id = existing.iter().find(|entry| entry.0 == node.node_id);
            let same_name = existing.iter().find(|entry| entry.1 == node.node_name);
            let (state, manual_override) = if let Some(entry) = same_id {
                (ParticipationState::from_db(&entry.2), entry.3)
            } else if let Some(entry) = same_name {
                let previous_state = ParticipationState::from_db(&entry.2);
                if previous_state == ParticipationState::Excluded && entry.3 {
                    (ParticipationState::Excluded, true)
                } else {
                    (ParticipationState::Changed, false)
                }
            } else {
                let default_state = default_state_for_name(&node.node_name, &node.profile_name, settings);
                if default_state == ParticipationState::Excluded {
                    (ParticipationState::Excluded, true)
                } else {
                    (ParticipationState::Changed, false)
                }
            };

            transaction.execute(
                r"
                INSERT INTO benchmark_nodes(
                    node_id, profile_uid, profile_name, node_name, node_type, fingerprint,
                    state, manual_override, active, first_seen_at, last_seen_at,
                    bootstrap_stage, next_latency_at, next_speed_at,
                    download_bootstrap_stage, upload_bootstrap_stage, next_download_at, next_upload_at
                ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1, ?9, ?9, 0, ?9, ?9, 0, 0, ?9, ?9)
                ON CONFLICT(node_id) DO UPDATE SET
                    profile_name = excluded.profile_name,
                    node_name = excluded.node_name,
                    node_type = excluded.node_type,
                    fingerprint = excluded.fingerprint,
                    state = excluded.state,
                    manual_override = excluded.manual_override,
                    active = 1,
                    last_seen_at = excluded.last_seen_at
                ",
                params![
                    node.node_id,
                    node.profile_uid,
                    node.profile_name,
                    node.node_name,
                    node.node_type,
                    node.fingerprint,
                    state.as_str(),
                    i64::from(manual_override),
                    now,
                ],
            )?;

            if let Some(entry) = same_id {
                if entry.1 != node.node_name {
                    insert_change(
                        &transaction,
                        profile_uid,
                        profile_name,
                        Some(&node.node_name),
                        "renamed",
                        state != ParticipationState::Excluded,
                        now,
                    )?;
                }
            } else if same_name.is_some() {
                insert_change(
                    &transaction,
                    profile_uid,
                    profile_name,
                    Some(&node.node_name),
                    "configurationChanged",
                    state != ParticipationState::Excluded,
                    now,
                )?;
            } else {
                insert_change(
                    &transaction,
                    profile_uid,
                    profile_name,
                    Some(&node.node_name),
                    "added",
                    state != ParticipationState::Excluded,
                    now,
                )?;
            }
        }

        for (node_id, node_name, state, _) in existing {
            if discovered_ids.contains(node_id.as_str()) {
                continue;
            }
            transaction.execute(
                "UPDATE benchmark_nodes SET active = 0, state = 'removed', last_seen_at = ?2 WHERE node_id = ?1",
                params![node_id, now],
            )?;
            if !discovered.iter().any(|node| node.node_name == node_name) {
                insert_change(
                    &transaction,
                    profile_uid,
                    profile_name,
                    Some(&node_name),
                    "removed",
                    ParticipationState::from_db(&state) != ParticipationState::Excluded,
                    now,
                )?;
            }
        }

        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    /// 返回面板显示的所有活跃节点。
    pub fn active_nodes(&self) -> Result<Vec<StoredNode>> {
        self.query_nodes(
            "SELECT node_id, profile_uid, profile_name, node_name, node_type, fingerprint, state, manual_override, \
             first_seen_at, last_seen_at, bootstrap_stage, next_latency_at, next_speed_at, \
             download_bootstrap_stage, upload_bootstrap_stage, next_download_at, next_upload_at, \
             snoozed_until, ignore_reminder FROM benchmark_nodes WHERE active = 1 ORDER BY profile_name, node_name",
            [],
        )
    }

    /// 返回当前到期且已经人工确认的低流量延迟节点。
    pub fn due_latency_nodes(&self, now: i64) -> Result<Vec<StoredNode>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT node_id, profile_uid, profile_name, node_name, node_type, fingerprint, state, manual_override, \
             first_seen_at, last_seen_at, bootstrap_stage, next_latency_at, next_speed_at, \
             download_bootstrap_stage, upload_bootstrap_stage, next_download_at, next_upload_at, \
             snoozed_until, ignore_reminder FROM benchmark_nodes \
             WHERE active = 1 AND manual_override = 1 AND state IN ('participating', 'observe') \
             AND next_latency_at <= ?1 ORDER BY next_latency_at, profile_uid, node_name LIMIT 500",
        )?;
        let nodes = statement
            .query_map([now], stored_node_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(nodes)
    }

    /// 返回当前至少一个上下行方向到期且已经人工确认的参赛节点。
    pub fn due_speed_nodes(&self, now: i64) -> Result<Vec<StoredNode>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT node_id, profile_uid, profile_name, node_name, node_type, fingerprint, state, manual_override, \
             first_seen_at, last_seen_at, bootstrap_stage, next_latency_at, next_speed_at, \
             download_bootstrap_stage, upload_bootstrap_stage, next_download_at, next_upload_at, \
             snoozed_until, ignore_reminder FROM benchmark_nodes \
             WHERE active = 1 AND manual_override = 1 AND state = 'participating' \
             AND (next_download_at <= ?1 OR next_upload_at <= ?1) \
             ORDER BY MIN(next_download_at, next_upload_at), profile_uid, node_name LIMIT 500",
        )?;
        let nodes = statement
            .query_map([now], stored_node_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(nodes)
    }

    /// 按订阅和节点显示名称查找当前记录，供手动批量测速登记流量。
    pub fn node_by_profile_and_name(&self, profile_uid: &str, node_name: &str) -> Result<Option<StoredNode>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT node_id, profile_uid, profile_name, node_name, node_type, fingerprint, state, manual_override, \
             first_seen_at, last_seen_at, bootstrap_stage, next_latency_at, next_speed_at, \
             download_bootstrap_stage, upload_bootstrap_stage, next_download_at, next_upload_at, \
             snoozed_until, ignore_reminder FROM benchmark_nodes \
             WHERE profile_uid = ?1 AND node_name = ?2 AND active = 1 ORDER BY last_seen_at DESC LIMIT 1",
        )?;
        let node = statement
            .query_row(params![profile_uid, node_name], stored_node_from_row)
            .optional()?;
        drop(statement);
        drop(connection);
        Ok(node)
    }

    /// 更新节点状态并标记为用户人工决定。
    pub fn update_node_state(&self, node_id: &str, state: ParticipationState, now: i64) -> Result<()> {
        let changed = self.connection.lock().execute(
            "UPDATE benchmark_nodes SET state = ?2, manual_override = 1, \
             next_latency_at = CASE WHEN ?2 = 'participating' OR ?2 = 'observe' THEN ?3 ELSE next_latency_at END, \
             next_speed_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_speed_at END, \
             next_download_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_download_at END, \
             next_upload_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_upload_at END \
             WHERE node_id = ?1 AND active = 1",
            params![node_id, state.as_str(), now],
        )?;
        if changed == 0 {
            anyhow::bail!("找不到需要更新的节点")
        }
        if state == ParticipationState::Excluded {
            self.connection.lock().execute(
                "UPDATE benchmark_changes SET relevant = 0 WHERE EXISTS (\
                 SELECT 1 FROM benchmark_nodes n WHERE n.node_id = ?1 \
                 AND n.profile_uid = benchmark_changes.profile_uid \
                 AND n.node_name = benchmark_changes.node_name)",
                [node_id],
            )?;
        }
        Ok(())
    }

    /// 用一次事务保存所选订阅下最终确认的参赛节点，未勾选节点明确设为排除。
    pub fn confirm_participating_nodes(
        &self,
        profile_uids: &[String],
        participating_node_ids: &[String],
        now: i64,
    ) -> Result<()> {
        if profile_uids.is_empty() {
            anyhow::bail!("请先选择至少一个订阅")
        }

        let selected_profiles = profile_uids.iter().map(String::as_str).collect::<HashSet<_>>();
        let participating = participating_node_ids
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let candidate_ids = {
            let mut statement = transaction.prepare(
                "SELECT node_id, profile_uid FROM benchmark_nodes WHERE active = 1 ORDER BY profile_uid, node_name",
            )?;
            statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?
                .filter_map(|row| match row {
                    Ok((node_id, profile_uid)) if selected_profiles.contains(profile_uid.as_str()) => Some(Ok(node_id)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        if candidate_ids.is_empty() {
            anyhow::bail!("当前范围内没有可确认的节点，请先保存并预览")
        }

        let known_ids = candidate_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        if participating.iter().any(|node_id| !known_ids.contains(node_id)) {
            anyhow::bail!("参赛节点列表已经变化，请重新预览后再确认")
        }

        for node_id in candidate_ids {
            let state = if participating.contains(node_id.as_str()) {
                ParticipationState::Participating
            } else {
                ParticipationState::Excluded
            };
            transaction.execute(
                "UPDATE benchmark_nodes SET state = ?2, manual_override = 1, \
                 next_latency_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_latency_at END, \
                 next_speed_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_speed_at END, \
                 next_download_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_download_at END, \
                 next_upload_at = CASE WHEN ?2 = 'participating' THEN ?3 ELSE next_upload_at END \
                 WHERE node_id = ?1 AND active = 1",
                params![node_id, state.as_str(), now],
            )?;
        }
        transaction.execute(
            "UPDATE benchmark_changes SET relevant = 0 WHERE node_name IS NOT NULL AND EXISTS (\
             SELECT 1 FROM benchmark_nodes n WHERE n.active = 1 AND n.state = 'excluded' \
             AND n.profile_uid = benchmark_changes.profile_uid \
             AND n.node_name = benchmark_changes.node_name)",
            [],
        )?;
        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    /// 判断所选订阅下是否已有候选节点且每个节点都经过用户二次确认。
    pub fn node_selection_confirmed(&self, profile_uids: &[String]) -> Result<bool> {
        if profile_uids.is_empty() {
            return Ok(false);
        }
        let selected_profiles = profile_uids.iter().map(String::as_str).collect::<HashSet<_>>();
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT profile_uid, manual_override FROM benchmark_nodes WHERE active = 1 ORDER BY profile_uid, node_name",
        )?;
        let confirmations = statement
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? != 0)))?
            .filter_map(|row| match row {
                Ok((profile_uid, confirmed)) if selected_profiles.contains(profile_uid.as_str()) => Some(Ok(confirmed)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(!confirmations.is_empty() && confirmations.into_iter().all(|confirmed| confirmed))
    }

    /// 更新节点的提醒延后或忽略策略。
    pub fn update_reminder(&self, node_id: &str, action: ReminderAction, now: i64) -> Result<()> {
        let (snoozed_until, ignore) = match action {
            ReminderAction::SnoozeSevenDays => (Some(now + 7 * 24 * 60 * 60), false),
            ReminderAction::Ignore => (None, true),
            ReminderAction::Reset => (None, false),
        };
        self.connection.lock().execute(
            "UPDATE benchmark_nodes SET snoozed_until = ?2, ignore_reminder = ?3 WHERE node_id = ?1",
            params![node_id, snoozed_until, i64::from(ignore)],
        )?;
        Ok(())
    }

    /// 将指定节点的延迟和速度计划都提前到现在。
    pub fn force_due(&self, node_ids: &[String], now: i64) -> Result<()> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        for node_id in node_ids {
            transaction.execute(
                "UPDATE benchmark_nodes SET next_latency_at = ?2, next_speed_at = ?2, \
                 next_download_at = ?2, next_upload_at = ?2 WHERE node_id = ?1 AND active = 1",
                params![node_id, now],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        Ok(())
    }

    /// 删除全部历史测量并把节点调度恢复到新一轮快速评测起点。
    pub fn clear_measurements(&self, now: i64) -> Result<usize> {
        let mut connection = self.connection.lock();
        let transaction = connection.transaction()?;
        let deleted = transaction.execute("DELETE FROM benchmark_measurements", [])?;
        transaction.execute(
            "UPDATE benchmark_nodes SET bootstrap_stage = 0, download_bootstrap_stage = 0, \
             upload_bootstrap_stage = 0, next_latency_at = ?1, next_speed_at = ?1, \
             next_download_at = ?1, next_upload_at = ?1, snoozed_until = NULL, ignore_reminder = 0 \
             WHERE active = 1",
            [now],
        )?;
        transaction.commit()?;
        drop(connection);
        Ok(deleted)
    }

    /// 更新节点下一次延迟测试时间。
    pub fn schedule_next_latency(&self, node_id: &str, next_at: i64) -> Result<()> {
        self.connection.lock().execute(
            "UPDATE benchmark_nodes SET next_latency_at = ?2 WHERE node_id = ?1",
            params![node_id, next_at],
        )?;
        Ok(())
    }

    /// 更新一个上下行方向的快速评测阶段和下次测试时间。
    pub fn schedule_next_speed_direction(
        &self,
        node_id: &str,
        kind: MeasurementKind,
        bootstrap_stage: u8,
        next_at: i64,
    ) -> Result<()> {
        let sql = match kind {
            MeasurementKind::Download => {
                "UPDATE benchmark_nodes SET download_bootstrap_stage = ?2, next_download_at = ?3 WHERE node_id = ?1"
            }
            MeasurementKind::Upload => {
                "UPDATE benchmark_nodes SET upload_bootstrap_stage = ?2, next_upload_at = ?3 WHERE node_id = ?1"
            }
            MeasurementKind::Latency => anyhow::bail!("延迟任务不能写入速度调度"),
        };
        let connection = self.connection.lock();
        connection.execute(sql, params![node_id, i64::from(bootstrap_stage), next_at])?;
        connection.execute(
            "UPDATE benchmark_nodes SET \
             bootstrap_stage = MIN(download_bootstrap_stage, upload_bootstrap_stage), \
             next_speed_at = MIN(next_download_at, next_upload_at) WHERE node_id = ?1",
            [node_id],
        )?;
        drop(connection);
        Ok(())
    }

    /// 在测试环境异常时只把节点延迟任务延后到短暂重试时间。
    pub fn defer_latency(&self, node_id: &str, retry_at: i64) -> Result<()> {
        self.connection.lock().execute(
            "UPDATE benchmark_nodes SET next_latency_at = MAX(next_latency_at, ?2) WHERE node_id = ?1",
            params![node_id, retry_at],
        )?;
        Ok(())
    }

    /// 只顺延一个高流量测速方向，不影响延迟和另一个速度方向。
    pub fn defer_speed_direction(&self, node_id: &str, kind: MeasurementKind, retry_at: i64) -> Result<()> {
        let sql = match kind {
            MeasurementKind::Download => {
                "UPDATE benchmark_nodes SET next_download_at = MAX(next_download_at, ?2) WHERE node_id = ?1"
            }
            MeasurementKind::Upload => {
                "UPDATE benchmark_nodes SET next_upload_at = MAX(next_upload_at, ?2) WHERE node_id = ?1"
            }
            MeasurementKind::Latency => anyhow::bail!("延迟任务不能写入速度调度"),
        };
        let connection = self.connection.lock();
        connection.execute(sql, params![node_id, retry_at])?;
        connection.execute(
            "UPDATE benchmark_nodes SET next_speed_at = MIN(next_download_at, next_upload_at) WHERE node_id = ?1",
            [node_id],
        )?;
        drop(connection);
        Ok(())
    }

    /// 保存一条延迟或传输测速记录。
    pub fn insert_measurement(&self, record: &MeasurementRecord) -> Result<()> {
        self.connection.lock().execute(
            r"
            INSERT INTO benchmark_measurements(
                node_id, kind, success, value, bytes_down, bytes_up,
                duration_ms, trigger_kind, rank_eligible, error, created_at
            ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            ",
            params![
                record.node_id,
                record.kind.as_str(),
                i64::from(record.success),
                record.value,
                i64::try_from(record.bytes_down).unwrap_or(i64::MAX),
                i64::try_from(record.bytes_up).unwrap_or(i64::MAX),
                i64::try_from(record.duration_ms).unwrap_or(i64::MAX),
                record.trigger,
                i64::from(record.rank_eligible),
                record.error,
                record.created_at,
            ],
        )?;
        Ok(())
    }

    /// 只读取当前所选节点在指定时间后的记录，避免普通页面刷新扫描全部长期历史。
    pub fn measurements_for_nodes_since(&self, node_ids: &[String], start_at: i64) -> Result<Vec<MeasurementRecord>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let placeholders = (2..node_ids.len() + 2)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT node_id, kind, success, value, bytes_down, bytes_up, duration_ms, \
             trigger_kind, rank_eligible, error, created_at FROM benchmark_measurements \
             WHERE created_at >= ?1 AND node_id IN ({placeholders}) ORDER BY created_at"
        );
        let connection = self.connection.lock();
        let mut statement = connection.prepare(&sql)?;
        let mut parameters = Vec::<&dyn ToSql>::with_capacity(node_ids.len() + 1);
        parameters.push(&start_at);
        parameters.extend(node_ids.iter().map(|node_id| node_id as &dyn ToSql));
        let records = statement
            .query_map(parameters.as_slice(), |row| {
                Ok(MeasurementRecord {
                    node_id: row.get(0)?,
                    kind: MeasurementKind::from_db(&row.get::<_, String>(1)?),
                    success: row.get::<_, i64>(2)? != 0,
                    value: row.get(3)?,
                    bytes_down: row.get::<_, i64>(4)?.max(0) as u64,
                    bytes_up: row.get::<_, i64>(5)?.max(0) as u64,
                    duration_ms: row.get::<_, i64>(6)?.max(0) as u64,
                    trigger: row.get(7)?,
                    rank_eligible: row.get::<_, i64>(8)? != 0,
                    error: row.get(9)?,
                    created_at: row.get(10)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(records)
    }

    /// 汇总指定时间之后的实际下载和上传字节数。
    pub fn actual_traffic_since(&self, start_at: i64) -> Result<(u64, u64)> {
        let connection = self.connection.lock();
        let (download, upload) = connection.query_row(
            "SELECT COALESCE(SUM(bytes_down), 0), COALESCE(SUM(bytes_up), 0) \
             FROM benchmark_measurements WHERE created_at >= ?1",
            [start_at],
            |row| Ok((row.get::<_, i64>(0)?.max(0) as u64, row.get::<_, i64>(1)?.max(0) as u64)),
        )?;
        drop(connection);
        Ok((download, upload))
    }

    /// 按订阅汇总指定时间之后的实际流量。
    pub fn actual_traffic_by_profile_since(&self, start_at: i64) -> Result<Vec<(String, String, u64, u64)>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT n.profile_uid, n.profile_name, COALESCE(SUM(m.bytes_down), 0), \
             COALESCE(SUM(m.bytes_up), 0) FROM benchmark_nodes n \
             LEFT JOIN benchmark_measurements m ON m.node_id = n.node_id AND m.created_at >= ?1 \
             WHERE n.active = 1 GROUP BY n.profile_uid, n.profile_name ORDER BY n.profile_name",
        )?;
        let totals = statement
            .query_map([start_at], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get::<_, i64>(2)?.max(0) as u64,
                    row.get::<_, i64>(3)?.max(0) as u64,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(totals)
    }

    /// 返回最近的订阅节点变化记录。
    pub fn recent_changes(&self, limit: usize, settings: &BenchmarkSettings) -> Result<Vec<BenchmarkChangeEvent>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(
            "SELECT id, profile_uid, profile_name, node_name, change_type, created_at \
             FROM benchmark_changes WHERE relevant = 1 ORDER BY id DESC LIMIT ?1",
        )?;
        let mut changes = statement
            .query_map([(limit.saturating_mul(10).max(limit)) as i64], |row| {
                Ok(BenchmarkChangeEvent {
                    id: row.get(0)?,
                    profile_uid: row.get(1)?,
                    profile_name: row.get(2)?,
                    node_name: row.get(3)?,
                    change_type: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        changes.retain(|change| {
            change.node_name.as_deref().is_none_or(|node_name| {
                default_state_for_name(node_name, &change.profile_name, settings) != ParticipationState::Excluded
            })
        });
        changes.truncate(limit);
        Ok(changes)
    }

    /// 将配置中的订阅列表与数据库扫描状态组合成前端摘要。
    pub fn profile_summaries(
        &self,
        profiles: &[(String, String)],
        settings: &BenchmarkSettings,
    ) -> Result<Vec<BenchmarkProfileSummary>> {
        let connection = self.connection.lock();
        let available = profiles.iter().cloned().collect::<HashMap<_, _>>();
        let profile_uids = profiles.iter().map(|(uid, _)| uid.clone()).chain(
            settings
                .selected_profile_uids
                .iter()
                .filter(|uid| !available.contains_key(*uid))
                .cloned(),
        );

        let mut summaries = profile_uids
            .map(|uid| {
                let scan = connection
                    .query_row(
                        "SELECT profile_name, last_scanned_at, scan_error FROM benchmark_profiles WHERE profile_uid = ?1",
                        [&uid],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<i64>>(1)?,
                                row.get::<_, Option<String>>(2)?,
                            ))
                        },
                    )
                    .optional()?;
                let node_count = connection
                    .query_row(
                        "SELECT COUNT(*) FROM benchmark_nodes WHERE profile_uid = ?1 AND active = 1",
                        [&uid],
                        |row| row.get::<_, i64>(0),
                    )?
                    .max(0) as usize;
                let is_available = available.contains_key(&uid);
                let name = available
                    .get(&uid)
                    .cloned()
                    .or_else(|| scan.as_ref().map(|value| value.0.clone()))
                    .unwrap_or_else(|| uid.clone());
                Ok(BenchmarkProfileSummary {
                    uid: uid.clone(),
                    name,
                    available: is_available,
                    selected: settings.selected_profile_uids.contains(&uid),
                    node_count,
                    scan_error: scan.as_ref().and_then(|value| value.2.clone()),
                    last_scanned_at: scan.and_then(|value| value.1),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        drop(connection);
        summaries.sort_by(|left, right| {
            right
                .available
                .cmp(&left.available)
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(summaries)
    }

    /// 使用统一查询读取节点列表。
    fn query_nodes<const N: usize>(&self, sql: &str, parameters: [&str; N]) -> Result<Vec<StoredNode>> {
        let connection = self.connection.lock();
        let mut statement = connection.prepare(sql)?;
        let nodes = statement
            .query_map(rusqlite::params_from_iter(parameters), stored_node_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        drop(connection);
        Ok(nodes)
    }
}

/// 将数据库行转换为节点调度记录。
fn stored_node_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredNode> {
    Ok(StoredNode {
        node_id: row.get(0)?,
        profile_uid: row.get(1)?,
        profile_name: row.get(2)?,
        node_name: row.get(3)?,
        node_type: row.get(4)?,
        fingerprint: row.get(5)?,
        state: ParticipationState::from_db(&row.get::<_, String>(6)?),
        manual_override: row.get::<_, i64>(7)? != 0,
        first_seen_at: row.get(8)?,
        last_seen_at: row.get(9)?,
        bootstrap_stage: row.get::<_, u8>(10)?,
        next_latency_at: row.get(11)?,
        next_speed_at: row.get(12)?,
        download_bootstrap_stage: row.get::<_, u8>(13)?,
        upload_bootstrap_stage: row.get::<_, u8>(14)?,
        next_download_at: row.get(15)?,
        next_upload_at: row.get(16)?,
        snoozed_until: row.get(17)?,
        ignore_reminder: row.get::<_, i64>(18)? != 0,
    })
}

/// 写入一条订阅节点变化记录。
fn insert_change(
    transaction: &rusqlite::Transaction<'_>,
    profile_uid: &str,
    profile_name: &str,
    node_name: Option<&str>,
    change_type: &str,
    relevant: bool,
    now: i64,
) -> Result<()> {
    transaction.execute(
        "INSERT INTO benchmark_changes(profile_uid, profile_name, node_name, change_type, relevant, created_at) \
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            profile_uid,
            profile_name,
            node_name,
            change_type,
            i64::from(relevant),
            now
        ],
    )?;
    Ok(())
}

/// 根据关键字设置决定新节点的默认状态。
fn default_state_for_name(node_name: &str, profile_name: &str, settings: &BenchmarkSettings) -> ParticipationState {
    let searchable_text = format!("{profile_name} {node_name}");
    let normalized_name = if settings.case_sensitive {
        searchable_text
    } else {
        searchable_text.to_lowercase()
    };
    let normalize = |value: &str| {
        if settings.case_sensitive {
            value.to_owned()
        } else {
            value.to_lowercase()
        }
    };
    let split = |value: &str| {
        value
            .split(['|', ',', ';', '\n'])
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(normalize)
            .collect::<Vec<_>>()
    };
    let includes = split(&settings.include_keywords);
    let excludes = split(&settings.exclude_keywords);
    let included = includes.is_empty() || includes.iter().any(|keyword| normalized_name.contains(keyword));
    let excluded = excludes.iter().any(|keyword| normalized_name.contains(keyword));
    if included && !excluded {
        ParticipationState::Participating
    } else {
        ParticipationState::Excluded
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, reason = "测试通过失败即终止来表达断言")]
mod tests {
    use super::{BenchmarkStore, default_state_for_name};
    use crate::feat::node_benchmark::models::{
        BenchmarkSettings, DiscoveredNode, MeasurementKind, MeasurementRecord, ParticipationState,
    };

    #[test]
    fn keyword_filter_prefers_exclusion_and_ignores_case_by_default() {
        let settings = BenchmarkSettings {
            include_keywords: "香港|日本".to_owned(),
            exclude_keywords: "倍率".to_owned(),
            ..BenchmarkSettings::default()
        };

        assert_eq!(
            default_state_for_name("香港 01", "机场A", &settings),
            ParticipationState::Participating
        );
        assert_eq!(
            default_state_for_name("香港 2倍率", "机场A", &settings),
            ParticipationState::Excluded
        );
        assert_eq!(
            default_state_for_name("Singapore", "机场A", &settings),
            ParticipationState::Excluded
        );
    }

    #[test]
    fn new_matching_node_waits_for_confirmation_before_it_can_be_due() {
        let database_path = std::env::temp_dir().join(format!("node-benchmark-{}.sqlite3", nanoid::nanoid!(10)));
        let store = BenchmarkStore::open(&database_path).expect("测试数据库应可创建");
        let settings = BenchmarkSettings {
            selected_profile_uids: vec!["profile-a".to_owned()],
            include_keywords: "香港".to_owned(),
            ..BenchmarkSettings::default()
        };
        let node = DiscoveredNode {
            node_id: "node-a".to_owned(),
            profile_uid: "profile-a".to_owned(),
            profile_name: "订阅A".to_owned(),
            node_name: "香港 01".to_owned(),
            node_type: "VLESS".to_owned(),
            fingerprint: "fingerprint-a".to_owned(),
        };

        store
            .reconcile_nodes("profile-a", "订阅A", &[node], &settings, 100)
            .expect("新节点应可写入");
        let stored = store.active_nodes().expect("应可读取节点");
        assert_eq!(stored[0].state, ParticipationState::Changed);
        assert!(!stored[0].manual_override);
        assert!(!store.node_selection_confirmed(&settings.selected_profile_uids).unwrap());
        assert!(store.due_latency_nodes(100).unwrap().is_empty());
        assert!(store.due_speed_nodes(100).unwrap().is_empty());

        store
            .confirm_participating_nodes(&settings.selected_profile_uids, &["node-a".to_owned()], 100)
            .expect("人工确认应成功");
        assert!(store.node_selection_confirmed(&settings.selected_profile_uids).unwrap());
        assert_eq!(store.due_latency_nodes(100).unwrap().len(), 1);
        assert_eq!(store.due_speed_nodes(100).unwrap().len(), 1);

        drop(store);
        let _ = std::fs::remove_file(&database_path);
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-shm"));
    }

    /// 清理历史应删除成绩、重置快速评测，并让今日流量统计按用户确认归零。
    #[test]
    fn clearing_measurements_starts_a_fresh_benchmark_cycle() {
        let database_path = std::env::temp_dir().join(format!("node-benchmark-{}.sqlite3", nanoid::nanoid!(10)));
        let store = BenchmarkStore::open(&database_path).expect("测试数据库应可创建");
        let settings = BenchmarkSettings {
            selected_profile_uids: vec!["profile-a".to_owned()],
            ..BenchmarkSettings::default()
        };
        let node = DiscoveredNode {
            node_id: "node-a".to_owned(),
            profile_uid: "profile-a".to_owned(),
            profile_name: "订阅A".to_owned(),
            node_name: "香港 01".to_owned(),
            node_type: "VLESS".to_owned(),
            fingerprint: "fingerprint-a".to_owned(),
        };
        store
            .reconcile_nodes("profile-a", "订阅A", &[node], &settings, 100)
            .expect("节点应可写入");
        store
            .confirm_participating_nodes(&settings.selected_profile_uids, &["node-a".to_owned()], 100)
            .expect("节点应可确认参赛");
        store
            .insert_measurement(&MeasurementRecord {
                node_id: "node-a".to_owned(),
                kind: MeasurementKind::Download,
                success: true,
                value: Some(1024.0),
                bytes_down: 4096,
                bytes_up: 0,
                duration_ms: 1000,
                trigger: "scheduled".to_owned(),
                rank_eligible: true,
                error: None,
                created_at: 150,
            })
            .expect("测试记录应可写入");

        assert_eq!(store.clear_measurements(200).expect("历史应可清理"), 1);
        assert!(
            store
                .measurements_for_nodes_since(&["node-a".to_owned()], 0)
                .expect("应可读取清理后的历史")
                .is_empty()
        );
        assert_eq!(store.actual_traffic_since(0).expect("应可读取清理后的流量"), (0, 0));
        let stored = store.active_nodes().expect("应可读取重置后的节点");
        assert_eq!(stored[0].bootstrap_stage, 0);
        assert_eq!(stored[0].download_bootstrap_stage, 0);
        assert_eq!(stored[0].upload_bootstrap_stage, 0);
        assert_eq!(stored[0].next_latency_at, 200);
        assert_eq!(stored[0].next_download_at, 200);
        assert_eq!(stored[0].next_upload_at, 200);

        drop(store);
        let _ = std::fs::remove_file(&database_path);
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-shm"));
    }

    /// 关键词未命中的新增节点不应出现在最近订阅变化中。
    #[test]
    fn irrelevant_unselected_node_change_is_hidden() {
        let database_path = std::env::temp_dir().join(format!("node-benchmark-{}.sqlite3", nanoid::nanoid!(10)));
        let store = BenchmarkStore::open(&database_path).expect("测试数据库应可创建");
        let settings = BenchmarkSettings {
            selected_profile_uids: vec!["profile-a".to_owned()],
            include_keywords: "香港".to_owned(),
            ..BenchmarkSettings::default()
        };
        let node = DiscoveredNode {
            node_id: "node-meta".to_owned(),
            profile_uid: "profile-a".to_owned(),
            profile_name: "订阅A".to_owned(),
            node_name: "剩余流量 100 GB".to_owned(),
            node_type: "VLESS".to_owned(),
            fingerprint: "fingerprint-meta".to_owned(),
        };

        store
            .reconcile_nodes("profile-a", "订阅A", &[node], &settings, 100)
            .expect("非候选节点也应保留在本地状态中");

        assert!(store.recent_changes(50, &settings).expect("应可读取变化").is_empty());

        drop(store);
        let _ = std::fs::remove_file(&database_path);
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-shm"));
    }

    /// 用户已经排除的节点连接配置变化后仍保持排除且不重复提醒。
    #[test]
    fn configuration_change_preserves_manual_exclusion() {
        let database_path = std::env::temp_dir().join(format!("node-benchmark-{}.sqlite3", nanoid::nanoid!(10)));
        let store = BenchmarkStore::open(&database_path).expect("测试数据库应可创建");
        let settings = BenchmarkSettings {
            selected_profile_uids: vec!["profile-a".to_owned()],
            include_keywords: "香港".to_owned(),
            ..BenchmarkSettings::default()
        };
        let first = DiscoveredNode {
            node_id: "node-a".to_owned(),
            profile_uid: "profile-a".to_owned(),
            profile_name: "订阅A".to_owned(),
            node_name: "香港 01".to_owned(),
            node_type: "VLESS".to_owned(),
            fingerprint: "fingerprint-a".to_owned(),
        };
        store
            .reconcile_nodes("profile-a", "订阅A", &[first], &settings, 100)
            .expect("首次节点应可写入");
        store
            .confirm_participating_nodes(&settings.selected_profile_uids, &[], 100)
            .expect("未勾选节点应可确认为排除");
        let relevant_before = store.recent_changes(50, &settings).expect("应可读取首次变化").len();
        let changed = DiscoveredNode {
            node_id: "node-b".to_owned(),
            profile_uid: "profile-a".to_owned(),
            profile_name: "订阅A".to_owned(),
            node_name: "香港 01".to_owned(),
            node_type: "VLESS".to_owned(),
            fingerprint: "fingerprint-b".to_owned(),
        };

        store
            .reconcile_nodes("profile-a", "订阅A", &[changed], &settings, 200)
            .expect("配置变化应可合并");

        let active = store.active_nodes().expect("应可读取变化后的节点");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].state, ParticipationState::Excluded);
        assert!(active[0].manual_override);
        assert_eq!(
            store.recent_changes(50, &settings).expect("应可读取过滤后的变化").len(),
            relevant_before
        );

        drop(store);
        let _ = std::fs::remove_file(&database_path);
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-wal"));
        let _ = std::fs::remove_file(database_path.with_extension("sqlite3-shm"));
    }
}
