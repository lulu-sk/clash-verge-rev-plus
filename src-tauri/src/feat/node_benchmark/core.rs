use super::profile::{BENCHMARK_GROUP_NAME, PreparedProfile};
use crate::{
    config::Config,
    core::handle,
    utils::{dirs, yaml_emitter},
};
use anyhow::{Context as _, Result};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value as JsonValue;
use serde_yaml_ng::Value;
use std::{
    net::{Ipv4Addr, SocketAddrV4, TcpListener},
    path::PathBuf,
    time::Duration,
};
use tauri_plugin_shell::{ShellExt as _, process::CommandChild};

/// 独立 Mihomo 测速实例及其本地接口。
pub struct BenchmarkCore {
    mixed_port: u16,
    controller_url: String,
    controller_secret: String,
    client: reqwest::Client,
    child: Option<CommandChild>,
    runtime_dir: PathBuf,
}

impl BenchmarkCore {
    /// 使用指定订阅配置启动一个不影响主代理线路的临时内核。
    pub async fn start(prepared: &PreparedProfile) -> Result<Self> {
        let (mixed_port, controller_port) = reserve_port_pair()?;
        let controller_secret = nanoid::nanoid!(24);
        let runtime_dir = dirs::app_home_dir()?
            .join("node-benchmark")
            .join("runtime")
            .join(nanoid::nanoid!(12));
        std::fs::create_dir_all(&runtime_dir)
            .with_context(|| format!("无法创建临时测速目录: {}", runtime_dir.display()))?;

        let mut config = prepared.config.clone();
        config.insert(yaml_key("mixed-port"), Value::Number(mixed_port.into()));
        config.insert(
            yaml_key("external-controller"),
            Value::String(format!("127.0.0.1:{controller_port}")),
        );
        config.insert(yaml_key("secret"), Value::String(controller_secret.clone()));
        config.insert(yaml_key("unified-delay"), Value::Bool(true));

        let config_path = runtime_dir.join("config.yaml");
        let config_text = yaml_emitter::to_mihomo_config_string(&config)?;
        tokio::fs::write(&config_path, config_text)
            .await
            .with_context(|| format!("无法写入临时测速配置: {}", config_path.display()))?;

        let app_handle = handle::Handle::app_handle();
        let clash_core = Config::verge().await.latest_arc().get_valid_clash_core();
        let command = app_handle
            .shell()
            .sidecar(clash_core.as_str())
            .map_err(|error| anyhow::anyhow!("无法创建独立 Mihomo 命令: {error}"))?
            .args([
                "-d",
                dirs::path_to_str(&runtime_dir)?,
                "-f",
                dirs::path_to_str(&config_path)?,
            ]);
        let (mut events, child) = command
            .spawn()
            .map_err(|error| anyhow::anyhow!("独立 Mihomo 启动失败: {error}"))?;
        let client = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(15))
            .build()?;

        tokio::spawn(async move { while events.recv().await.is_some() {} });

        let mut core = Self {
            mixed_port,
            controller_url: format!("http://127.0.0.1:{controller_port}"),
            controller_secret,
            client,
            child: Some(child),
            runtime_dir,
        };
        if let Err(error) = core.wait_until_ready().await {
            core.stop_process();
            return Err(error);
        }
        Ok(core)
    }

    /// 返回独立实例提供给真实上下行请求使用的混合代理端口。
    pub const fn mixed_port(&self) -> u16 {
        self.mixed_port
    }

    /// 读取临时选择组中的全部实际节点及 Mihomo 类型。
    pub async fn list_nodes(&self) -> Result<Vec<(String, String)>> {
        let all_response = self
            .authorized(self.client.get(format!("{}/proxies", self.controller_url)))
            .send()
            .await?
            .error_for_status()?
            .json::<JsonValue>()
            .await?;
        let group_response = self
            .authorized(self.client.get(self.proxy_endpoint(BENCHMARK_GROUP_NAME)))
            .send()
            .await?
            .error_for_status()?
            .json::<JsonValue>()
            .await?;
        let proxy_types = all_response
            .get("proxies")
            .and_then(JsonValue::as_object)
            .cloned()
            .unwrap_or_default();
        let names = group_response
            .get("all")
            .and_then(JsonValue::as_array)
            .ok_or_else(|| anyhow::anyhow!("独立 Mihomo 未返回测速节点列表"))?;

        Ok(names
            .iter()
            .filter_map(JsonValue::as_str)
            .map(|name| {
                let node_type = proxy_types
                    .get(name)
                    .and_then(|value| value.get("type"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Unknown")
                    .to_owned();
                (name.to_owned(), node_type)
            })
            .collect())
    }

    /// 只在临时实例中选中节点，用户主代理组不会发生变化。
    pub async fn select_node(&self, node_name: &str) -> Result<()> {
        self.authorized(
            self.client
                .put(self.proxy_endpoint(BENCHMARK_GROUP_NAME))
                .json(&serde_json::json!({ "name": node_name })),
        )
        .send()
        .await?
        .error_for_status()
        .with_context(|| format!("临时内核无法选择节点“{node_name}”"))?;
        Ok(())
    }

    /// 通过 Mihomo 节点延迟接口执行一次低流量延迟测试。
    pub async fn test_delay(&self, node_name: &str, url: &str, timeout_ms: u64) -> Result<u32> {
        let timeout = timeout_ms.to_string();
        let response = self
            .authorized(
                self.client
                    .get(format!("{}/delay", self.proxy_endpoint(node_name)))
                    .query(&[("url", url), ("timeout", timeout.as_str())]),
            )
            .send()
            .await?
            .error_for_status()
            .with_context(|| format!("节点“{node_name}”延迟测试失败"))?
            .json::<JsonValue>()
            .await?;
        response
            .get("delay")
            .and_then(JsonValue::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow::anyhow!("节点“{node_name}”未返回有效延迟"))
    }

    /// 等待临时内核控制接口在限定时间内就绪。
    async fn wait_until_ready(&self) -> Result<()> {
        let mut last_error = None;
        for _ in 0..40 {
            match self
                .authorized(self.client.get(format!("{}/version", self.controller_url)))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => return Ok(()),
                Ok(response) => last_error = Some(anyhow::anyhow!("控制接口状态码 {}", response.status())),
                Err(error) => last_error = Some(error.into()),
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("未知启动错误"))).context("独立 Mihomo 在 8 秒内未就绪")
    }

    /// 为本地控制接口请求附加仅本进程知道的访问令牌。
    fn authorized(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder.bearer_auth(&self.controller_secret)
    }

    /// 构造已正确编码的节点控制接口地址。
    fn proxy_endpoint(&self, proxy_name: &str) -> String {
        format!(
            "{}/proxies/{}",
            self.controller_url,
            utf8_percent_encode(proxy_name, NON_ALPHANUMERIC)
        )
    }

    /// 终止子进程并清理本次运行目录。
    fn stop_process(&mut self) {
        if let Some(child) = self.child.take() {
            let _ = child.kill();
        }
        let _ = std::fs::remove_dir_all(&self.runtime_dir);
    }
}

impl Drop for BenchmarkCore {
    /// 确保取消、报错或应用退出时都不会遗留测速内核。
    fn drop(&mut self) {
        self.stop_process();
    }
}

/// 同时预留两个不同的本地 TCP 端口。
fn reserve_port_pair() -> Result<(u16, u16)> {
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0);
    let mixed_listener = TcpListener::bind(address).context("无法预留独立测速代理端口")?;
    let controller_listener = TcpListener::bind(address).context("无法预留独立测速控制端口")?;
    let mixed_port = mixed_listener.local_addr()?.port();
    let controller_port = controller_listener.local_addr()?.port();
    Ok((mixed_port, controller_port))
}

/// 返回字符串形式的 YAML 键。
fn yaml_key(value: &str) -> Value {
    Value::String(value.to_owned())
}
