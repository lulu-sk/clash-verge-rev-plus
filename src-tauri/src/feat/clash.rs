use crate::{
    config::{Config, MixedPort},
    core::{CoreManager, handle, tray},
    feat::clean_async,
    process::AsyncHandler,
    utils,
};
use bytes::{Bytes, BytesMut};
use clash_verge_logging::{Type, logging};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value};
use smartstring::alias::String;
use std::sync::Arc;

#[allow(clippy::expect_used)]
static TLS_CONFIG: Lazy<Arc<rustls::ClientConfig>> = Lazy::new(|| {
    let root_store = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("Failed to set TLS versions")
        .with_root_certificates(root_store)
        .with_no_client_auth();
    Arc::new(config)
});

/// Restart the Clash core
pub async fn restart_clash_core() {
    match CoreManager::global().restart_core().await {
        Ok(_) => {
            handle::Handle::refresh_clash();
            handle::Handle::notice_message("set_config::ok", "ok");
        }
        Err(err) => {
            handle::Handle::notice_message("set_config::error", format!("{err}"));
            logging!(error, Type::Core, "{err}");
        }
    }
}

/// Restart the application
pub async fn restart_app() {
    logging!(debug, Type::System, "启动重启应用流程");
    // 设置退出标志
    handle::Handle::global().set_is_exiting();

    Config::apply_all_and_save_file().await;

    logging!(info, Type::System, "开始异步清理资源");
    let cleanup_result = clean_async().await;

    logging!(
        info,
        Type::System,
        "资源清理完成，退出代码: {}",
        if cleanup_result.all_success { 0 } else { 1 }
    );

    if !cleanup_result.core_stopped {
        handle::Handle::global().clear_is_exiting();
        handle::Handle::notice_message("app_restart::core_stop_failed", "");
        return;
    }

    utils::server::shutdown_embedded_server();
    let app_handle = handle::Handle::app_handle();
    app_handle.restart();
}

fn after_change_clash_mode() {
    AsyncHandler::spawn(move || async {
        let mihomo = handle::Handle::mihomo();
        match mihomo.get_connections().await {
            Ok(connections) => {
                if let Some(connections_array) = connections.connections {
                    for connection in connections_array {
                        let _ = mihomo.close_connection(&connection.id).await;
                    }
                }
            }
            Err(err) => {
                logging!(error, Type::Core, "Failed to get connections: {err}");
            }
        }
    });
}

/// Change Clash mode (rule/global/direct/script)
///
/// mihomo `/configs` PATCH 失败时返回 `Err`，以便命令层把失败上抛给前端。
/// （此前该函数吞掉错误并始终视为成功，导致 UI 误判"切换成功"、看似"切不动"。）
pub async fn change_clash_mode(mode: String) -> Result<(), String> {
    let mut mapping = Mapping::new();
    mapping.insert(Value::from("mode"), Value::from(mode.as_str()));
    // Convert YAML mapping to JSON Value
    let json_value = serde_json::json!({
        "mode": mode
    });
    logging!(debug, Type::Core, "change clash mode to {mode}");
    if let Err(err) = handle::Handle::mihomo().patch_base_config(&json_value).await {
        logging!(error, Type::Core, "{err}");
        return Err(err.to_string().into());
    }

    // 更新订阅
    let clash = Config::clash().await;
    clash.edit_draft(|d| d.patch_config(&mapping));
    clash.apply();

    // 分离数据获取和异步调用
    let clash_data = clash.data_arc();
    if clash_data.save_config().await.is_ok() {
        handle::Handle::refresh_clash();
        tray::Tray::global().update_menu_and_icon().await;
    }

    let is_auto_close_connection = Config::verge().await.data_arc().auto_close_connection.unwrap_or(false);
    if is_auto_close_connection {
        after_change_clash_mode();
    }

    Ok(())
}

/// Test delay to a URL through proxy.
/// HTTPS: measures TLS handshake time. HTTP: measures HEAD round-trip time.
pub async fn test_delay(url: String) -> anyhow::Result<u32> {
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpStream;
    use tokio::time::Instant;

    let parsed = tauri::Url::parse(&url)?;
    let is_https = parsed.scheme() == "https";
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid URL: no host"))?
        .to_string();
    let port = parsed.port().unwrap_or(if is_https { 443 } else { 80 });

    let verge = Config::verge().await.latest_arc();
    let proxy_enabled = verge.enable_system_proxy.unwrap_or(false) || verge.enable_tun_mode.unwrap_or(false);
    let proxy_port = if proxy_enabled {
        Some(MixedPort::desired().await)
    } else {
        None
    };

    tokio::time::timeout(Duration::from_secs(10), async {
        let start = Instant::now();
        let mut buf = BytesMut::with_capacity(1024);

        if is_https {
            let stream = match proxy_port {
                Some(pp) => {
                    let mut s = TcpStream::connect(format!("127.0.0.1:{pp}")).await?;
                    s.write_all(format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n\r\n").as_bytes())
                        .await?;
                    s.read_buf(&mut buf).await?;
                    if !buf.windows(3).any(|w| w == b"200") {
                        return Err(anyhow::anyhow!("Proxy CONNECT failed"));
                    }
                    s
                }
                None => TcpStream::connect(format!("{host}:{port}")).await?,
            };
            let connector = tokio_rustls::TlsConnector::from(Arc::clone(&TLS_CONFIG));
            let server_name = rustls::pki_types::ServerName::try_from(host.as_str())
                .map_err(|_| anyhow::anyhow!("Invalid DNS name: {host}"))?
                .to_owned();
            connector.connect(server_name, stream).await?;
        } else {
            let (mut stream, req) = match proxy_port {
                Some(pp) => (
                    TcpStream::connect(format!("127.0.0.1:{pp}")).await?,
                    format!("HEAD {url} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
                ),
                None => (
                    TcpStream::connect(format!("{host}:{port}")).await?,
                    format!("HEAD / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"),
                ),
            };
            stream.write_all(req.as_bytes()).await?;
            let _ = stream.read(&mut buf).await?;
        }

        // frontend treats 0 as timeout
        Ok((start.elapsed().as_millis() as u32).max(1))
    })
    .await
    .unwrap_or(Ok(10000u32))
}

/// 代理传输测速参数。
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedTestOptions {
    pub url: String,
    pub duration_ms: Option<u64>,
    pub max_bytes: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub read_idle_timeout_ms: Option<u64>,
}

/// 代理下载测速结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSpeedTestResult {
    pub final_url: String,
    pub status_code: u16,
    pub content_type: Option<String>,
    pub content_length: Option<u64>,
    pub bytes_read: u64,
    pub elapsed_ms: u64,
    pub average_bytes_per_second: u64,
}

/// 代理上传测速结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSpeedTestResult {
    pub final_url: String,
    pub status_code: u16,
    pub content_type: Option<String>,
    pub bytes_sent: u64,
    pub elapsed_ms: u64,
    pub average_bytes_per_second: u64,
}

/// 保留失败前已经产生的真实传输量，避免流量统计把部分失败误记为零。
#[derive(Debug)]
pub(crate) struct SpeedTestFailure {
    message: String,
    pub bytes_down: u64,
    pub bytes_up: u64,
    pub elapsed_ms: u64,
}

impl SpeedTestFailure {
    /// 创建一条带实际传输进度的测速错误。
    fn new(message: impl Into<String>, bytes_down: u64, bytes_up: u64, elapsed_ms: u64) -> Self {
        Self {
            message: message.into(),
            bytes_down,
            bytes_up,
            elapsed_ms,
        }
    }
}

impl std::fmt::Display for SpeedTestFailure {
    /// 输出给日志和界面的简明失败原因。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SpeedTestFailure {}

/// 将原始测速参数规范化为安全范围内的配置。
fn normalize_speed_test_options(options: SpeedTestOptions) -> anyhow::Result<SpeedTestOptions> {
    let url = options.url.trim().to_owned();
    if url.is_empty() {
        anyhow::bail!("测速链接不能为空");
    }

    Ok(SpeedTestOptions {
        url: url.into(),
        duration_ms: Some(options.duration_ms.unwrap_or(5_000).clamp(1_000, 30_000)),
        max_bytes: Some(
            options
                .max_bytes
                .unwrap_or(32 * 1024 * 1024)
                .clamp(1024 * 1024, 2 * 1024 * 1024 * 1024),
        ),
        connect_timeout_ms: Some(options.connect_timeout_ms.unwrap_or(8_000).clamp(1_000, 30_000)),
        read_idle_timeout_ms: Some(options.read_idle_timeout_ms.unwrap_or(3_000).clamp(1_000, 15_000)),
    })
}

/// 创建统一经过本地 mixed-port 的测速客户端。
fn create_speed_test_client(connect_timeout_ms: u64, mixed_port: u16) -> anyhow::Result<reqwest::Client> {
    use std::time::Duration;

    let proxy_url = format!("http://127.0.0.1:{mixed_port}");
    Ok(reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url)?)
        .connect_timeout(Duration::from_millis(connect_timeout_ms))
        .user_agent("Clash-Verge-Rev-Plus/Node-Benchmark")
        .build()?)
}

/// 根据传输字节数与耗时计算平均速度。
fn calculate_average_speed(bytes: u64, elapsed_ms: u64) -> u64 {
    bytes.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(0)
}

/// 根据单次最长时间计算预热结束和允许稳定提前结束的时间点。
fn adaptive_speed_timing(duration_ms: u64) -> (u64, u64) {
    if duration_ms < 8_000 {
        return (duration_ms / 4, duration_ms);
    }
    (2_000, 8_000.min(duration_ms))
}

/// 判断最近三个一秒吞吐样本是否已进入波动不超过8%的平台期。
fn recent_speed_samples_stable(samples: &[u64]) -> bool {
    let recent = samples.iter().rev().take(3).copied().collect::<Vec<_>>();
    if recent.len() < 3 || recent.contains(&0) {
        return false;
    }
    let minimum = recent.iter().copied().min().unwrap_or_default();
    let maximum = recent.iter().copied().max().unwrap_or_default();
    let average = recent.iter().sum::<u64>() / recent.len() as u64;
    average > 0 && maximum.saturating_sub(minimum) as f64 / average as f64 <= 0.08
}

/// 通过本地 mixed-port 代理执行一次真实下载测速。
pub async fn test_download_speed(options: SpeedTestOptions) -> anyhow::Result<DownloadSpeedTestResult> {
    let mixed_port = Config::clash().await.data_arc().get_mixed_port();
    test_download_speed_with_port(options, mixed_port).await
}

/// 通过明确指定的 mixed-port 执行下载测速，供主内核和独立测速内核共用。
pub(crate) async fn test_download_speed_with_port(
    options: SpeedTestOptions,
    mixed_port: u16,
) -> anyhow::Result<DownloadSpeedTestResult> {
    use reqwest::header::{ACCEPT_ENCODING, CACHE_CONTROL};
    use std::time::{Duration, Instant};

    let options = normalize_speed_test_options(options)?;
    let duration_ms = options.duration_ms.unwrap_or(5_000);
    let max_bytes = options.max_bytes.unwrap_or(32 * 1024 * 1024);
    let connect_timeout_ms = options.connect_timeout_ms.unwrap_or(8_000);
    let read_idle_timeout_ms = options.read_idle_timeout_ms.unwrap_or(3_000);

    let client = create_speed_test_client(connect_timeout_ms, mixed_port)?;

    let request_started = Instant::now();
    let response = tokio::time::timeout(
        Duration::from_millis(connect_timeout_ms.saturating_add(read_idle_timeout_ms)),
        client
            .get(options.url.as_str())
            .header(ACCEPT_ENCODING, "identity")
            .header(CACHE_CONTROL, "no-store")
            .send(),
    )
    .await;
    let mut response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return Err(
                SpeedTestFailure::new(error.to_string(), 0, 0, request_started.elapsed().as_millis() as u64).into(),
            );
        }
        Err(_) => {
            return Err(
                SpeedTestFailure::new("测速请求超时", 0, 0, request_started.elapsed().as_millis() as u64).into(),
            );
        }
    };

    if !response.status().is_success() {
        return Err(SpeedTestFailure::new(
            format!("测速源返回异常状态码: {}", response.status()),
            0,
            0,
            request_started.elapsed().as_millis() as u64,
        )
        .into());
    }

    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_length = response.content_length();
    let final_url = response.url().to_string();
    let start = Instant::now();
    let deadline = start + Duration::from_millis(duration_ms);
    let (warmup_ms, minimum_duration_ms) = adaptive_speed_timing(duration_ms);
    let warmup_deadline = start + Duration::from_millis(warmup_ms);
    let minimum_deadline = start + Duration::from_millis(minimum_duration_ms);
    let mut next_sample_at = start + Duration::from_secs(1);
    let mut bytes_read = 0_u64;
    let mut warmup_bytes = 0_u64;
    let mut previous_sample_bytes = 0_u64;
    let mut previous_sample_at = start;
    let mut speed_samples = Vec::new();

    while Instant::now() < deadline && bytes_read < max_bytes {
        let chunk_result = tokio::time::timeout(Duration::from_millis(read_idle_timeout_ms), response.chunk()).await;

        match chunk_result {
            Ok(Ok(Some(chunk))) => {
                if chunk.is_empty() {
                    continue;
                }

                bytes_read = bytes_read.saturating_add(chunk.len() as u64);
                let now = Instant::now();
                if warmup_bytes == 0 && now >= warmup_deadline {
                    warmup_bytes = bytes_read;
                }
                if now >= next_sample_at {
                    let sample_elapsed = now.duration_since(previous_sample_at).as_millis() as u64;
                    speed_samples.push(calculate_average_speed(
                        bytes_read.saturating_sub(previous_sample_bytes),
                        sample_elapsed,
                    ));
                    previous_sample_bytes = bytes_read;
                    previous_sample_at = now;
                    next_sample_at = now + Duration::from_secs(1);
                }
                if now >= minimum_deadline && recent_speed_samples_stable(&speed_samples) {
                    break;
                }
            }
            Ok(Ok(None)) => {
                let now = Instant::now();
                if now >= deadline || bytes_read >= max_bytes {
                    break;
                }

                let remaining = deadline.saturating_duration_since(now);
                let repeat_timeout =
                    Duration::from_millis(connect_timeout_ms.saturating_add(read_idle_timeout_ms)).min(remaining);
                let next_response = tokio::time::timeout(
                    repeat_timeout,
                    client
                        .get(options.url.as_str())
                        .header(ACCEPT_ENCODING, "identity")
                        .header(CACHE_CONTROL, "no-store")
                        .send(),
                )
                .await;
                match next_response {
                    Ok(Ok(next_response)) if next_response.status().is_success() => {
                        response = next_response;
                    }
                    Ok(Ok(next_response)) if bytes_read == 0 => {
                        return Err(SpeedTestFailure::new(
                            format!("测速源返回异常状态码: {}", next_response.status()),
                            0,
                            0,
                            start.elapsed().as_millis() as u64,
                        )
                        .into());
                    }
                    Ok(Err(error)) if bytes_read == 0 => {
                        return Err(
                            SpeedTestFailure::new(error.to_string(), 0, 0, start.elapsed().as_millis() as u64).into(),
                        );
                    }
                    Err(_) if bytes_read == 0 => {
                        return Err(
                            SpeedTestFailure::new("测速请求超时", 0, 0, start.elapsed().as_millis() as u64).into(),
                        );
                    }
                    _ => break,
                }
            }
            Ok(Err(error)) => {
                return Err(SpeedTestFailure::new(
                    error.to_string(),
                    bytes_read,
                    0,
                    start.elapsed().as_millis() as u64,
                )
                .into());
            }
            Err(_) if bytes_read > 0 => break,
            Err(_) => anyhow::bail!("测速读取超时"),
        }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    if bytes_read == 0 {
        return Err(SpeedTestFailure::new("未接收任何测速数据", 0, 0, elapsed_ms).into());
    }
    let effective_bytes = if elapsed_ms > warmup_ms && bytes_read > warmup_bytes {
        bytes_read - warmup_bytes
    } else {
        bytes_read
    };
    let effective_elapsed_ms = if elapsed_ms > warmup_ms {
        elapsed_ms - warmup_ms
    } else {
        elapsed_ms
    };
    let average_bytes_per_second = calculate_average_speed(effective_bytes, effective_elapsed_ms);

    Ok(DownloadSpeedTestResult {
        final_url: final_url.into(),
        status_code,
        content_type: content_type.map(Into::into),
        content_length,
        bytes_read,
        elapsed_ms,
        average_bytes_per_second,
    })
}

/// 通过本地 mixed-port 代理执行一次真实上传测速。
pub async fn test_upload_speed(options: SpeedTestOptions) -> anyhow::Result<UploadSpeedTestResult> {
    let mixed_port = Config::clash().await.data_arc().get_mixed_port();
    test_upload_speed_with_port(options, mixed_port).await
}

/// 通过明确指定的 mixed-port 执行上传测速，供主内核和独立测速内核共用。
pub(crate) async fn test_upload_speed_with_port(
    options: SpeedTestOptions,
    mixed_port: u16,
) -> anyhow::Result<UploadSpeedTestResult> {
    use futures::stream;
    use reqwest::header::CONTENT_TYPE;
    use std::{
        io,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::{Duration, Instant},
    };

    const UPLOAD_CHUNK_SIZE: usize = 64 * 1024;

    let options = normalize_speed_test_options(options)?;
    let duration_ms = options.duration_ms.unwrap_or(5_000);
    let max_bytes = options.max_bytes.unwrap_or(32 * 1024 * 1024);
    let connect_timeout_ms = options.connect_timeout_ms.unwrap_or(8_000);
    let read_idle_timeout_ms = options.read_idle_timeout_ms.unwrap_or(3_000);
    let client = create_speed_test_client(connect_timeout_ms, mixed_port)?;

    let start = Instant::now();
    let deadline = start + Duration::from_millis(duration_ms);
    let (warmup_ms, minimum_duration_ms) = adaptive_speed_timing(duration_ms);
    let warmup_deadline = start + Duration::from_millis(warmup_ms);
    let minimum_deadline = start + Duration::from_millis(minimum_duration_ms);
    let bytes_sent_counter = Arc::new(AtomicU64::new(0));
    let warmup_bytes_counter = Arc::new(AtomicU64::new(0));
    let stream_counter = Arc::clone(&bytes_sent_counter);
    let stream_warmup_counter = Arc::clone(&warmup_bytes_counter);
    let upload_chunk = Bytes::from(vec![0_u8; UPLOAD_CHUNK_SIZE]);
    let upload_stream = stream::unfold(
        (
            0_u64,
            upload_chunk,
            start,
            0_u64,
            Vec::<u64>::new(),
            start + Duration::from_secs(1),
        ),
        move |(
            bytes_sent,
            upload_chunk,
            previous_sample_at,
            previous_sample_bytes,
            mut speed_samples,
            next_sample_at,
        )| {
            let stream_counter = Arc::clone(&stream_counter);
            let stream_warmup_counter = Arc::clone(&stream_warmup_counter);
            async move {
                let now = Instant::now();
                if now >= deadline || bytes_sent >= max_bytes {
                    return None;
                }
                let mut previous_sample_at = previous_sample_at;
                let mut previous_sample_bytes = previous_sample_bytes;
                let mut next_sample_at = next_sample_at;
                if stream_warmup_counter.load(Ordering::Relaxed) == 0 && now >= warmup_deadline {
                    stream_warmup_counter.store(bytes_sent, Ordering::Relaxed);
                }
                if now >= next_sample_at {
                    let sample_elapsed = now.duration_since(previous_sample_at).as_millis() as u64;
                    speed_samples.push(calculate_average_speed(
                        bytes_sent.saturating_sub(previous_sample_bytes),
                        sample_elapsed,
                    ));
                    previous_sample_at = now;
                    previous_sample_bytes = bytes_sent;
                    next_sample_at = now + Duration::from_secs(1);
                }
                if now >= minimum_deadline && recent_speed_samples_stable(&speed_samples) {
                    return None;
                }

                let chunk_size = (max_bytes - bytes_sent).min(UPLOAD_CHUNK_SIZE as u64) as usize;
                let next_bytes_sent = bytes_sent + chunk_size as u64;
                stream_counter.store(next_bytes_sent, Ordering::Relaxed);
                Some((
                    Ok::<Bytes, io::Error>(upload_chunk.slice(..chunk_size)),
                    (
                        next_bytes_sent,
                        upload_chunk,
                        previous_sample_at,
                        previous_sample_bytes,
                        speed_samples,
                        next_sample_at,
                    ),
                ))
            }
        },
    );
    let request_timeout_ms = duration_ms
        .saturating_add(connect_timeout_ms)
        .saturating_add(read_idle_timeout_ms);
    let response = tokio::time::timeout(
        Duration::from_millis(request_timeout_ms),
        client
            .post(options.url.as_str())
            .header(CONTENT_TYPE, "application/octet-stream")
            .body(reqwest::Body::wrap_stream(upload_stream))
            .send(),
    )
    .await;
    let response = match response {
        Ok(Ok(response)) => response,
        Ok(Err(error)) => {
            return Err(SpeedTestFailure::new(
                error.to_string(),
                0,
                bytes_sent_counter.load(Ordering::Relaxed),
                start.elapsed().as_millis() as u64,
            )
            .into());
        }
        Err(_) => {
            return Err(SpeedTestFailure::new(
                "测速上传超时",
                0,
                bytes_sent_counter.load(Ordering::Relaxed),
                start.elapsed().as_millis() as u64,
            )
            .into());
        }
    };

    if !response.status().is_success() {
        return Err(SpeedTestFailure::new(
            format!("测速源返回异常状态码: {}", response.status()),
            0,
            bytes_sent_counter.load(Ordering::Relaxed),
            start.elapsed().as_millis() as u64,
        )
        .into());
    }

    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let final_url = response.url().to_string();
    let bytes_sent = bytes_sent_counter.load(Ordering::Relaxed);
    if bytes_sent == 0 {
        return Err(SpeedTestFailure::new("未发送任何测速数据", 0, 0, start.elapsed().as_millis() as u64).into());
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let warmup_bytes = warmup_bytes_counter.load(Ordering::Relaxed);
    let effective_bytes = if elapsed_ms > warmup_ms && bytes_sent > warmup_bytes {
        bytes_sent - warmup_bytes
    } else {
        bytes_sent
    };
    let effective_elapsed_ms = if elapsed_ms > warmup_ms {
        elapsed_ms - warmup_ms
    } else {
        elapsed_ms
    };
    let average_bytes_per_second = calculate_average_speed(effective_bytes, effective_elapsed_ms);

    Ok(UploadSpeedTestResult {
        final_url: final_url.into(),
        status_code,
        content_type: content_type.map(Into::into),
        bytes_sent,
        elapsed_ms,
        average_bytes_per_second,
    })
}

#[cfg(test)]
mod speed_test_tests {
    use super::{adaptive_speed_timing, recent_speed_samples_stable};

    #[test]
    fn long_test_uses_two_second_warmup_and_eight_second_minimum() {
        assert_eq!(adaptive_speed_timing(12_000), (2_000, 8_000));
        assert_eq!(adaptive_speed_timing(3_000), (750, 3_000));
    }

    #[test]
    fn stable_detection_requires_three_close_non_zero_samples() {
        assert!(recent_speed_samples_stable(&[100, 103, 98]));
        assert!(!recent_speed_samples_stable(&[100, 130, 90]));
        assert!(!recent_speed_samples_stable(&[100, 101]));
    }
}
