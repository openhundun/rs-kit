//! crates.io 网络层：[`RegistryFetch`] 抽象、生产实现与 JSON 解析。
//!
//! ## 抽象
//! 生产实现 [`CratesIoRegistry`] 用 `ureq` 请求 crates.io API；单测注入 fake 返回 canned JSON，不发真实请求。
//!
//! ```text
//! GET https://crates.io/api/v1/crates/{name}/{version}   （带 user-agent）
//! 200 → Ok(Some(json))    404 → Ok(None)    其他 → Err
//! ```

#[cfg(test)]
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use serde::Deserialize;

/// crates.io 查询失败（传输错误、非 200/404 状态等）。
#[derive(Clone, Debug)]
pub struct RegistryError {
    /// 人读的错误描述。
    pub message: String,
}

impl fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RegistryError {}

/// 版本特性查询抽象；`Ok(None)` 表示该 crate/version 不存在（404）。
pub trait RegistryFetch {
    /// 拉取 `{name}/{version}` 的版本 JSON 原文。
    fn version_features_json(&self, name: &str, version: &str) -> Result<Option<String>, RegistryError>;
}

/// 生产实现：`ureq` + crates.io API，`user-agent: cargo-flow/0.1.0`。
#[derive(Debug)]
pub struct CratesIoRegistry;

impl RegistryFetch for CratesIoRegistry {
    fn version_features_json(&self, name: &str, version: &str) -> Result<Option<String>, RegistryError> {
        let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
        match ureq::get(&url).set("user-agent", "cargo-flow/0.1.0").call() {
            Ok(response) => response.into_string().map(Some).map_err(|error| RegistryError { message: error.to_string() }),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(RegistryError { message: error.to_string() }),
        }
    }
}

/// crates.io 版本 JSON 形态：`{"version": {"features": {"default": [...], ...}}}`。
#[derive(Debug, Deserialize)]
struct VersionResponse {
    /// 版本记录。
    version: VersionRecord,
}

#[derive(Debug, Deserialize)]
struct VersionRecord {
    /// 特性名 → 特性列表；缺失时按空表处理。
    #[serde(default)]
    features: HashMap<String, Vec<String>>,
}

/// 从版本 JSON 取 `version.features.default`；JSON 非法或字段缺失返回 `None`。
pub(crate) fn default_features_from_json(json_text: &str) -> Option<Vec<String>> {
    let response = serde_json::from_str::<VersionResponse>(json_text).ok()?;
    response.version.features.get("default").cloned()
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct FakeRegistry {
    /// 查询键 → canned 响应；缺省键按 404 处理。
    pub response_map: HashMap<(String, String), Result<Option<String>, RegistryError>>,
    /// 已发生的查询记录，供断言不误查。
    pub query_list: RefCell<Vec<(String, String)>>,
}

#[cfg(test)]
impl RegistryFetch for FakeRegistry {
    fn version_features_json(&self, name: &str, version: &str) -> Result<Option<String>, RegistryError> {
        self.query_list.borrow_mut().push((name.to_string(), version.to_string()));
        match self.response_map.get(&(name.to_string(), version.to_string())) {
            Some(response) => response.clone(),
            None => Ok(None),
        }
    }
}
