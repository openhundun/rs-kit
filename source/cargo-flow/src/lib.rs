//! `cargo-flow` —— `cargo flow` 子命令，对 Cargo.toml 清单做结构归一；解析与回写全程使用 `toml_edit`，未触碰的字节原样保留。
//!
//! ## 功能
//! 点键展开为核心，始终执行；`--features`、`--default-features`、`--normalize-versions` 可选。
//!
//! ### 点键展开
//! 白名单表内的所有点键按第一段分组，合并为一个内联表条目：子键保持原出现顺序，值原样平移；被折叠行上方的整行注释与行尾注释按原顺序移到新条目上方，注释文本不丢弃。
//!
//! ```toml
//! [dependencies]
//! # inherit from workspace
//! serde.workspace = true
//! serde.features = ["derive"]
//! ```
//!
//! 展开为：
//!
//! ```toml
//! [dependencies]
//! # inherit from workspace
//! serde = { workspace = true, features = ["derive"] }
//! ```
//!
//! `workspace = false` 不视为继承，值照抄并入内联表；同一根键的点键与显式条目并存属于非法 TOML，解析期即失败。
//!
//! ### --features
//! 成员侧依赖表（`[dependencies]`、`[dev-dependencies]`、`[build-dependencies]`、`[target.*.*dependencies]`）中缺 `features` 键的条目补 `features = []`，继承条目（`workspace = true`）同样补；`[workspace.dependencies]` 不补。
//!
//! ### --default-features
//! 成员侧依赖表中显式声明裸数字版本（`x` / `x.y` / `x.y.z`）的条目，查询 `https://crates.io/api/v1/crates/{name}/{version}`，取 `version.features.default` 列表，将 `default-features` 置为 `false`，并把缺失的 default 特性按序追加进 `features`。
//!
//! cargo 语义约束：继承条目（`workspace = true`）声明 `default-features` 无效（继承只合并 features，default-features 无法覆盖），故一律跳过；`[workspace.dependencies]` 是特性的定义来源，不处理。带操作符或 pre-release 的版本、404 与网络失败告警后跳过；default 列表缺失或为空静默跳过。单依赖失败只告警，进程仍以 0 退出；网络层抽象为 [`RegistryFetch`]，测试注入 fake，不发真实请求。
//!
//! ### --normalize-versions
//! 依赖表条目的 `version`（含 `[workspace.dependencies]`，不含 `[package].version`）去掉尾部 `.0` 至最短 caret 等价形；`^0.0.4` 与 `^0.0` 不等价，故 `0.0.x` 形态保持不变；带操作符或 pre-release 的要求不动。
//!
//! ```text
//! 1.2.0 → 1.2    1.0.0 → 1    0.2.0 → 0.2    3.27.0 → 3.27
//! 1.2.3 / 0.0.4 / 0.0.0 / 0.0 / 0 保持不变
//! ```
//!
//! ## 白名单表
//! 表头路径最后一段命中下列集合才处理；`[lints]`、`[[bin]]`、`[lib]`、`[features]` 一律不碰：
//!
//! ```text
//! package | dependencies | dev-dependencies | build-dependencies
//! ```
//!
//! ## 管线与退出码
//! 每个文件依次执行 解析 → 展开 → --features → --default-features → --normalize-versions，有变化才写回，全程幂等；fatal（解析失败、IO 失败）打印 stderr 并以 1 退出。
//!
//! ```text
//! 0  成功（单依赖网络失败仅告警）
//! 1  fatal
//! ```

pub mod expand;
pub mod features;
pub mod registry;
pub mod versions;

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

pub use crate::registry::{CratesIoRegistry, RegistryError, RegistryFetch};

/// 白名单中可被点键展开命中的表头末段；`package` 表仅展开会处理。
pub(crate) const EXPANDABLE_SEGMENT_LIST: [&str; 4] = ["package", "dependencies", "dev-dependencies", "build-dependencies"];

/// 白名单中的依赖表末段；`--features`、`--default-features`、`--normalize-versions` 的公共判定基础。
pub(crate) const DEPENDENCY_SEGMENT_LIST: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// 表头路径最后一段是否命中展开白名单（含 `[workspace.package]`、`[target.'cfg(..)'.dependencies]` 等）。
pub(crate) fn is_expandable_table(path: &[String]) -> bool {
    path.last().is_some_and(|segment| EXPANDABLE_SEGMENT_LIST.contains(&segment.as_str()))
}

/// 表头路径最后一段是否为依赖表（含 `[workspace.dependencies]`，不含 `package`）。
pub(crate) fn is_dependency_table(path: &[String]) -> bool {
    path.last().is_some_and(|segment| DEPENDENCY_SEGMENT_LIST.contains(&segment.as_str()))
}

/// 是否为成员侧依赖表（依赖表且不以 `[workspace.*]` 为根，供 `--features` 与 `--default-features` 收窄作用域）。
pub(crate) fn is_member_dependency_table(path: &[String]) -> bool {
    is_dependency_table(path) && !path.first().is_some_and(|segment| segment == "workspace")
}

/// 单次处理的改写选项；点键展开始终执行，不受选项控制。
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// 补全成员侧依赖的 `features = []`。
    pub features: bool,
    /// 按 crates.io 的 default 特性改写 `default-features` 与 `features`。
    pub default_features: bool,
    /// 归一化裸数字版本号。
    pub normalize_versions: bool,
}

/// 单个文件的处理计数；为零的项不打摘要行。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChangeSummary {
    /// 被折叠点键的条数（每个点键路径计一）。
    pub expanded_key_count: usize,
    /// 补上 `features` 键的条目数。
    pub features_filled_count: usize,
    /// 改写 `default-features` / `features` 的条目数。
    pub default_features_applied_count: usize,
    /// 被归一化的版本号个数。
    pub versions_normalized_count: usize,
}

/// 处理失败：IO 失败或解析失败（含点键与显式条目并存的非法清单）。
#[derive(Debug)]
pub enum FlowError {
    /// 读或写目标文件失败。
    Io {
        /// 目标文件路径。
        path: PathBuf,
        /// 底层 IO 错误。
        source: std::io::Error,
    },
    /// 清单解析失败。
    Parse(toml_edit::TomlError),
}

impl fmt::Display for FlowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(formatter, "io error on {}: {}", path.display(), source),
            Self::Parse(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for FlowError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Parse(error) => Some(error),
        }
    }
}

impl From<toml_edit::TomlError> for FlowError {
    fn from(error: toml_edit::TomlError) -> Self {
        Self::Parse(error)
    }
}

/// 处理单个 Cargo.toml：读入、解析、按 [`Options`] 执行管线，渲染结果与原文不同才写回；告警经 `warn` 输出。
pub fn process_path(path: &Path, options: &Options, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> Result<ChangeSummary, FlowError> {
    let original_text = fs::read_to_string(path).map_err(|source| FlowError::Io { path: path.to_path_buf(), source })?;
    let mut document = original_text.parse::<DocumentMut>()?;
    let summary = process_document(&mut document, options, registry, warn);
    let rendered_text = document.to_string();
    if rendered_text != original_text {
        fs::write(path, rendered_text).map_err(|source| FlowError::Io { path: path.to_path_buf(), source })?;
    }
    Ok(summary)
}

/// 对已解析文档执行完整管线：展开 → `--features` → `--default-features` → `--normalize-versions`。
pub fn process_document(document: &mut DocumentMut, options: &Options, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> ChangeSummary {
    let expanded_key_count = expand::expand_dotted_keys(document.as_table_mut());
    let features_filled_count = if options.features { features::fill_features(document.as_table_mut()) } else { 0 };
    let default_features_applied_count = if options.default_features {
        features::apply_default_features(document.as_table_mut(), registry, warn)
    } else {
        0
    };
    let versions_normalized_count = if options.normalize_versions { versions::normalize_versions(document.as_table_mut()) } else { 0 };
    ChangeSummary { expanded_key_count, features_filled_count, default_features_applied_count, versions_normalized_count }
}

/// 深度优先遍历文档内所有非点键的显式表，向 `visitor` 提供表与其表头路径（根表不含表头，不访问）。
pub(crate) fn visit_tables(root_table: &mut Table, visitor: &mut dyn FnMut(&mut Table, &[String])) {
    let mut path = Vec::new();
    visit_nested_tables(root_table, &mut path, visitor);
}

fn visit_nested_tables(table: &mut Table, path: &mut Vec<String>, visitor: &mut dyn FnMut(&mut Table, &[String])) {
    for (key, item) in table.iter_mut() {
        if let Item::Table(sub_table) = item {
            if sub_table.is_dotted() {
                continue;
            }
            path.push(key.get().to_string());
            visitor(sub_table, path);
            visit_nested_tables(sub_table, path, visitor);
            path.pop();
        }
    }
}
