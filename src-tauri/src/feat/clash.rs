use crate::{
    config::{Config, MixedPort},
    core::{CoreManager, handle, tray},
    feat::clean_async,
    process::AsyncHandler,
    utils,
};
use bytes::BytesMut;
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

/// 代理下载测速参数。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadSpeedTestOptions {
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

/// 将原始测速参数规范化为安全范围内的配置。
fn normalize_download_speed_test_options(
    options: DownloadSpeedTestOptions,
) -> anyhow::Result<DownloadSpeedTestOptions> {
    let url = options.url.trim().to_owned();
    if url.is_empty() {
        anyhow::bail!("测速链接不能为空");
    }

    Ok(DownloadSpeedTestOptions {
        url: url.into(),
        duration_ms: Some(options.duration_ms.unwrap_or(5_000).clamp(1_000, 30_000)),
        max_bytes: Some(
            options
                .max_bytes
                .unwrap_or(32 * 1024 * 1024)
                .clamp(1024 * 1024, 512 * 1024 * 1024),
        ),
        connect_timeout_ms: Some(options.connect_timeout_ms.unwrap_or(8_000).clamp(1_000, 30_000)),
        read_idle_timeout_ms: Some(options.read_idle_timeout_ms.unwrap_or(3_000).clamp(1_000, 15_000)),
    })
}

/// 通过本地 mixed-port 代理执行一次真实下载测速。
pub async fn test_download_speed(options: DownloadSpeedTestOptions) -> anyhow::Result<DownloadSpeedTestResult> {
    use reqwest::header::{ACCEPT_ENCODING, CONNECTION};
    use std::time::{Duration, Instant};

    let options = normalize_download_speed_test_options(options)?;
    let duration_ms = options.duration_ms.unwrap_or(5_000);
    let max_bytes = options.max_bytes.unwrap_or(32 * 1024 * 1024);
    let connect_timeout_ms = options.connect_timeout_ms.unwrap_or(8_000);
    let read_idle_timeout_ms = options.read_idle_timeout_ms.unwrap_or(3_000);

    let proxy_port = Config::clash().await.data_arc().get_mixed_port();
    let proxy_url = format!("http://127.0.0.1:{proxy_port}");

    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy_url)?)
        .connect_timeout(Duration::from_millis(connect_timeout_ms))
        .build()?;

    let start = Instant::now();
    let mut response = client
        .get(options.url.as_str())
        .header(ACCEPT_ENCODING, "identity")
        .header(CONNECTION, "close")
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("测速源返回异常状态码: {}", response.status());
    }

    let status_code = response.status().as_u16();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned);
    let content_length = response.content_length();
    let final_url = response.url().to_string();
    let deadline = start + Duration::from_millis(duration_ms);
    let mut bytes_read = 0_u64;

    while Instant::now() < deadline && bytes_read < max_bytes {
        let chunk_result = tokio::time::timeout(Duration::from_millis(read_idle_timeout_ms), response.chunk()).await;

        match chunk_result {
            Ok(Ok(Some(chunk))) => {
                if chunk.is_empty() {
                    continue;
                }

                bytes_read = bytes_read.saturating_add(chunk.len() as u64);
            }
            Ok(Ok(None)) => break,
            Ok(Err(error)) => return Err(error.into()),
            Err(_) if bytes_read > 0 => break,
            Err(_) => anyhow::bail!("测速读取超时"),
        }
    }

    let elapsed_ms = start.elapsed().as_millis() as u64;
    let average_bytes_per_second = bytes_read.saturating_mul(1000).checked_div(elapsed_ms).unwrap_or(0);

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
