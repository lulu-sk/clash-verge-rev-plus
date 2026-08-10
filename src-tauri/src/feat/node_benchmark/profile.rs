use super::models::DiscoveredNode;
use crate::{config::Config, enhance, utils::dirs};
use anyhow::{Context as _, Result};
use serde_yaml_ng::{Mapping, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

/// 临时 Mihomo 内部唯一的测速选择组名称。
pub const BENCHMARK_GROUP_NAME: &str = "__CVR_NODE_BENCHMARK__";

/// 订阅增强配置和来源信息。
pub struct PreparedProfile {
    pub profile_uid: String,
    pub profile_name: String,
    pub config: Mapping,
    pub source_hash: String,
    direct_fingerprints: HashMap<String, (String, String)>,
}

impl PreparedProfile {
    /// 将 Mihomo API 返回的节点目录转换为带订阅来源的稳定节点记录。
    pub fn discovered_nodes(&self, api_nodes: &[(String, String)]) -> Vec<DiscoveredNode> {
        let mut fingerprints = HashSet::new();
        api_nodes
            .iter()
            .filter(|(name, node_type)| !is_special_proxy(name, node_type))
            .map(|(name, node_type)| {
                let base_fingerprint = self
                    .direct_fingerprints
                    .get(name)
                    .map(|value| value.0.clone())
                    .unwrap_or_else(|| stable_hash(format!("{}\n{name}\n{node_type}", self.source_hash).as_bytes()));
                let fingerprint = if fingerprints.insert(base_fingerprint.clone()) {
                    base_fingerprint
                } else {
                    format!("{}-{}", base_fingerprint, stable_hash(name.as_bytes()))
                };
                DiscoveredNode {
                    node_id: format!("{}:{fingerprint}", self.profile_uid),
                    profile_uid: self.profile_uid.clone(),
                    profile_name: self.profile_name.clone(),
                    node_name: name.clone(),
                    node_type: self
                        .direct_fingerprints
                        .get(name)
                        .map(|value| value.1.clone())
                        .unwrap_or_else(|| node_type.clone()),
                    fingerprint,
                }
            })
            .collect()
    }

    /// 在没有代理提供者时直接从配置生成节点目录，不启动临时内核。
    pub fn direct_nodes(&self) -> Vec<DiscoveredNode> {
        let api_nodes = self
            .direct_fingerprints
            .iter()
            .map(|(name, (_, node_type))| (name.clone(), node_type.clone()))
            .collect::<Vec<_>>();
        self.discovered_nodes(&api_nodes)
    }

    /// 判断配置是否包含需要由 Mihomo 实际加载的代理提供者。
    pub fn has_proxy_providers(&self) -> bool {
        self.config
            .get(Value::String("proxy-providers".to_owned()))
            .and_then(Value::as_mapping)
            .is_some_and(|providers| !providers.is_empty())
    }
}

/// 返回所有可以作为评选来源的本地或远程订阅。
pub async fn available_profiles() -> Vec<(String, String)> {
    let profiles = Config::profiles().await.latest_arc();
    profiles
        .get_items()
        .into_iter()
        .flatten()
        .filter(|item| matches!(item.itype.as_deref(), Some("remote" | "local")))
        .filter_map(|item| Some((item.uid.as_ref()?.to_string(), item.name.as_ref()?.to_string())))
        .collect()
}

/// 返回主界面当前选中的订阅标识，供现有批量测速默认使用。
pub async fn current_profile_uid() -> Option<String> {
    Config::profiles()
        .await
        .latest_arc()
        .current
        .as_ref()
        .map(ToString::to_string)
}

/// 为指定订阅生成不会修改主运行配置的独立测速配置。
pub async fn prepare_profile(profile_uid: &str, mixed_port: u16) -> Result<PreparedProfile> {
    let profiles = Config::profiles().await.latest_arc();
    let mut isolated_profiles = (*profiles).clone();
    let item = isolated_profiles
        .get_item(profile_uid)
        .with_context(|| format!("找不到订阅 {profile_uid}"))?;
    if !matches!(item.itype.as_deref(), Some("remote" | "local")) {
        anyhow::bail!("所选配置不是可测速订阅")
    }
    let profile_name = item.name.as_deref().unwrap_or(profile_uid).to_owned();
    isolated_profiles.current = Some(profile_uid.into());
    let (enhanced, _, _) = enhance::enhance(&isolated_profiles)
        .await
        .with_context(|| format!("无法生成订阅“{profile_name}”的独立配置"))?;
    let mut config = build_isolated_config(&enhanced, mixed_port)?;
    absolutize_provider_paths(&mut config, &dirs::app_home_dir()?);
    let source_bytes = serde_json::to_vec(&config)?;
    let source_hash = stable_hash(&source_bytes);
    let mut direct_fingerprints = direct_proxy_fingerprints(&config)?;
    direct_fingerprints.extend(provider_proxy_fingerprints(&config));

    Ok(PreparedProfile {
        profile_uid: profile_uid.to_owned(),
        profile_name,
        config,
        source_hash,
        direct_fingerprints,
    })
}

/// 从增强配置中只提取测速所需的代理与代理提供者，并覆盖所有监听项。
fn build_isolated_config(source: &Mapping, mixed_port: u16) -> Result<Mapping> {
    let mut config = Mapping::new();
    config.insert(yaml_key("mixed-port"), Value::Number(mixed_port.into()));
    config.insert(yaml_key("mode"), Value::String("rule".to_owned()));
    config.insert(yaml_key("log-level"), Value::String("silent".to_owned()));
    config.insert(yaml_key("ipv6"), Value::Bool(false));
    config.insert(yaml_key("allow-lan"), Value::Bool(false));
    config.insert(yaml_key("bind-address"), Value::String("127.0.0.1".to_owned()));

    for key in ["proxies", "proxy-providers"] {
        if let Some(value) = source.get(yaml_key(key)) {
            config.insert(yaml_key(key), value.clone());
        }
    }

    let mut group = Mapping::new();
    group.insert(yaml_key("name"), Value::String(BENCHMARK_GROUP_NAME.to_owned()));
    group.insert(yaml_key("type"), Value::String("select".to_owned()));
    group.insert(yaml_key("include-all"), Value::Bool(true));
    group.insert(
        yaml_key("proxies"),
        Value::Sequence(vec![Value::String("DIRECT".to_owned())]),
    );
    config.insert(yaml_key("proxy-groups"), Value::Sequence(vec![Value::Mapping(group)]));
    config.insert(
        yaml_key("rules"),
        Value::Sequence(vec![Value::String(format!("MATCH,{BENCHMARK_GROUP_NAME}"))]),
    );

    if !config.contains_key(yaml_key("proxies")) && !config.contains_key(yaml_key("proxy-providers")) {
        anyhow::bail!("订阅中没有可用于测速的代理或代理提供者")
    }
    Ok(config)
}

/// 为配置中的直接代理生成忽略显示名称的稳定指纹。
fn direct_proxy_fingerprints(config: &Mapping) -> Result<HashMap<String, (String, String)>> {
    let mut result = HashMap::new();
    let Some(proxies) = config.get(yaml_key("proxies")).and_then(Value::as_sequence) else {
        return Ok(result);
    };

    for proxy in proxies {
        let Some(mapping) = proxy.as_mapping() else {
            continue;
        };
        let Some(name) = mapping.get(yaml_key("name")).and_then(Value::as_str) else {
            continue;
        };
        let node_type = mapping
            .get(yaml_key("type"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_owned();
        let mut connection_mapping = mapping.clone();
        connection_mapping.remove(yaml_key("name"));
        let canonical = serde_json::to_vec(&connection_mapping)?;
        result.insert(name.to_owned(), (stable_hash(&canonical), node_type));
    }
    Ok(result)
}

/// 将相对代理提供者路径转换为主配置目录下的绝对路径，供临时运行目录复用。
fn absolutize_provider_paths(config: &mut Mapping, config_root: &Path) {
    let Some(providers) = config
        .get_mut(yaml_key("proxy-providers"))
        .and_then(Value::as_mapping_mut)
    else {
        return;
    };
    for provider in providers.values_mut().filter_map(Value::as_mapping_mut) {
        let Some(raw_path) = provider.get(yaml_key("path")).and_then(Value::as_str) else {
            continue;
        };
        let path = Path::new(raw_path);
        if path.is_absolute() {
            continue;
        }
        provider.insert(
            yaml_key("path"),
            Value::String(config_root.join(path).to_string_lossy().into_owned()),
        );
    }
}

/// 从本地代理提供者缓存中提取实际连接配置指纹，避免一次订阅更新重置全部节点。
fn provider_proxy_fingerprints(config: &Mapping) -> HashMap<String, (String, String)> {
    let Some(providers) = config.get(yaml_key("proxy-providers")).and_then(Value::as_mapping) else {
        return HashMap::new();
    };
    let mut result = HashMap::new();
    for provider in providers.values().filter_map(Value::as_mapping) {
        let Some(path) = provider.get(yaml_key("path")).and_then(Value::as_str) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_yaml_ng::from_str::<Value>(&source) else {
            continue;
        };
        let Some(mapping) = value.as_mapping() else {
            continue;
        };
        let Ok(fingerprints) = direct_proxy_fingerprints(mapping) else {
            continue;
        };
        result.extend(fingerprints);
    }
    result
}

/// 返回字符串形式的 YAML 键。
fn yaml_key(value: &str) -> Value {
    Value::String(value.to_owned())
}

/// 判断节点是否属于 Mihomo 内置出口而不应参与评选。
fn is_special_proxy(name: &str, node_type: &str) -> bool {
    matches!(name, "DIRECT" | "REJECT" | "REJECT-DROP" | "PASS" | "COMPATIBLE")
        || matches!(
            node_type.to_ascii_lowercase().as_str(),
            "direct" | "reject" | "rejectdrop" | "pass" | "compatible" | "selector"
        )
}

/// 使用固定 FNV-1a 算法生成跨版本可复现的短指纹。
pub fn stable_hash(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "测试通过失败即终止来表达断言")]
mod tests {
    use super::{BENCHMARK_GROUP_NAME, build_isolated_config, direct_proxy_fingerprints};
    use serde_yaml_ng::Mapping;

    /// 解析测试用订阅配置。
    fn mapping(source: &str) -> Mapping {
        serde_yaml_ng::from_str(source).expect("测试 YAML 应可解析")
    }

    #[test]
    fn isolated_config_discards_user_listeners_and_keeps_proxy_sources() {
        let source = mapping(
            r"
mixed-port: 7890
tun:
  enable: true
proxies:
  - name: HK 01
    type: ss
    server: example.com
    port: 443
    cipher: aes-128-gcm
    password: secret
",
        );
        let result = build_isolated_config(&source, 19000).expect("应生成隔离配置");
        assert_eq!(result.get("mixed-port").and_then(|value| value.as_i64()), Some(19000));
        assert!(!result.contains_key("tun"));
        let groups = result
            .get("proxy-groups")
            .and_then(|value| value.as_sequence())
            .expect("应创建测速组");
        assert_eq!(
            groups[0]
                .as_mapping()
                .and_then(|group| group.get("name"))
                .and_then(|value| value.as_str()),
            Some(BENCHMARK_GROUP_NAME)
        );
    }

    #[test]
    fn direct_fingerprint_ignores_display_name() {
        let first = mapping(
            "proxies:\n  - { name: Old, type: ss, server: example.com, port: 443, cipher: aes-128-gcm, password: secret }",
        );
        let second = mapping(
            "proxies:\n  - { name: New, type: ss, server: example.com, port: 443, cipher: aes-128-gcm, password: secret }",
        );
        let first_hash = direct_proxy_fingerprints(&first).expect("应生成指纹")["Old"].0.clone();
        let second_hash = direct_proxy_fingerprints(&second).expect("应生成指纹")["New"].0.clone();
        assert_eq!(first_hash, second_hash);
    }
}
