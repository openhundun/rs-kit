//! `--features` 与 `--default-features`：成员侧依赖表的特性键补全与 crates.io default 特性改写。
//!
//! ## 作用域
//! 两者都只处理成员侧依赖表（末段为 `dependencies` / `dev-dependencies` / `build-dependencies` 且不以 `[workspace.*]` 为根）；继承条目（`workspace = true`）声明 `default-features` 无效，故 `--default-features` 一律跳过继承条目。
//!
//! ## `--features`
//! 缺 `features` 键的条目补 `features = []`：内联表直接插入，裸字符串条目转 `{ version = ..., features = [] }`（行尾注释移到条目上方），子表插入一行。
//!
//! ## `--default-features`
//! 显式裸数字版本的条目经 [`crate::registry::RegistryFetch`] 查询 `version.features.default`，`default-features` 置 `false`，缺失的 default 特性保序追加进 `features`；非裸数字版本、404、网络失败告警后跳过，default 列表缺失或为空静默跳过。

use toml_edit::{Array, InlineTable, Item, KeyMut, RawString, Table, TableLike, Value};

use crate::registry::{RegistryFetch, default_features_from_json};
use crate::versions::bare_numeric_version_segment_list;
use crate::{is_member_dependency_table, visit_tables};

/// 成员侧依赖条目的版本声明形态。
#[derive(Debug)]
struct DependencyEntry {
    /// 显式版本文本；继承条目没有版本。
    explicit_version: Option<String>,
    /// 是否为继承条目（`workspace = true`）。
    inherited: bool,
}

/// `--features`：为缺 `features` 键的成员侧依赖条目补 `features = []`，返回补全条目数。
pub(crate) fn fill_features(root_table: &mut Table) -> usize {
    let mut filled_entry_count = 0usize;
    visit_tables(root_table, &mut |table, path| {
        if !is_member_dependency_table(path) {
            return;
        }
        for (mut key, item) in table.iter_mut() {
            if entry_has_features_key(item) {
                continue;
            }
            if let Some(relocated_comment) = add_features_key(item) {
                append_relocated_comment(&mut key, &relocated_comment);
            }
            filled_entry_count += 1;
        }
    });
    filled_entry_count
}

/// `--default-features`：按 crates.io 的 default 特性改写条目，返回改写条目数；单依赖失败经 `warn` 告警后继续。
pub(crate) fn apply_default_features(root_table: &mut Table, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> usize {
    let mut applied_entry_count = 0usize;
    visit_tables(root_table, &mut |table, path| {
        if !is_member_dependency_table(path) {
            return;
        }
        for (mut key, item) in table.iter_mut() {
            let Some(entry) = dependency_entry(item) else {
                continue;
            };
            if entry.inherited {
                continue;
            }
            let Some(version_text) = entry.explicit_version else {
                continue;
            };
            let dependency_name = key.get();
            if bare_numeric_version_segment_list(&version_text).is_none() {
                warn(&format!("cargo-flow: skipping {dependency_name}: version '{version_text}' is not a bare numeric requirement"));
                continue;
            }
            let default_feature_list = match registry.version_features_json(dependency_name, &version_text) {
                Ok(Some(json_text)) => default_features_from_json(&json_text),
                Ok(None) => {
                    warn(&format!("cargo-flow: skipping {dependency_name}: {dependency_name}/{version_text} not found on crates.io"));
                    continue;
                }
                Err(error) => {
                    warn(&format!("cargo-flow: skipping {dependency_name}: registry fetch failed: {error}"));
                    continue;
                }
            };
            let Some(default_feature_list) = default_feature_list.filter(|feature_list| !feature_list.is_empty()) else {
                continue;
            };
            if apply_default_features_to_entry(item, &mut key, &default_feature_list) {
                applied_entry_count += 1;
            }
        }
    });
    applied_entry_count
}

/// 读取条目的版本声明形态；非依赖值形态返回 `None`。
fn dependency_entry(item: &Item) -> Option<DependencyEntry> {
    match item {
        Item::Value(Value::String(string_value)) => Some(DependencyEntry { explicit_version: Some(string_value.value().clone()), inherited: false }),
        Item::Value(Value::InlineTable(inline_table)) => Some(DependencyEntry {
            explicit_version: inline_table.get("version").and_then(Value::as_str).map(str::to_string),
            inherited: inline_table.get("workspace").and_then(Value::as_bool) == Some(true),
        }),
        Item::Table(sub_table) => Some(DependencyEntry {
            explicit_version: sub_table.get("version").and_then(Item::as_str).map(str::to_string),
            inherited: sub_table.get("workspace").and_then(Item::as_bool) == Some(true),
        }),
        _ => None,
    }
}

/// 对单条目应用 default 特性改写，返回是否有变化。
fn apply_default_features_to_entry(item: &mut Item, key: &mut KeyMut<'_>, default_feature_list: &[String]) -> bool {
    match item {
        Item::Value(Value::String(_)) => {
            let old_value = match std::mem::replace(item, Item::None) {
                Item::Value(value) => value,
                _ => unreachable!("外层 match 已限定值条目"),
            };
            let relocated_comment = trailing_comment_text(&old_value);
            let mut inline_table = InlineTable::new();
            inline_table.insert("version", old_value);
            inline_table.insert("default-features", Value::from(false));
            inline_table.insert("features", feature_list_value(default_feature_list));
            inline_table.fmt();
            *item = Item::Value(Value::InlineTable(inline_table));
            if let Some(comment) = relocated_comment {
                append_relocated_comment(key, &comment);
            }
            true
        }
        Item::Value(Value::InlineTable(_)) | Item::Table(_) => {
            let Some(table_like) = item.as_table_like_mut() else {
                return false;
            };
            let existing_feature_list = feature_list_of(table_like);
            let merged_feature_list = merge_feature_list(&existing_feature_list, default_feature_list);
            let mut changed = false;
            if merged_feature_list != existing_feature_list {
                set_feature_list(table_like, &merged_feature_list);
                changed = true;
            }
            if !is_default_features_false(table_like) {
                set_default_features_false(table_like);
                changed = true;
            }
            if changed {
                if let Item::Value(Value::InlineTable(inline_table)) = item {
                    inline_table.fmt();
                }
            }
            changed
        }
        _ => false,
    }
}

/// 条目是否已有 `features` 键。
fn entry_has_features_key(item: &Item) -> bool {
    match item {
        Item::Value(Value::InlineTable(inline_table)) => inline_table.contains_key("features"),
        Item::Table(sub_table) => sub_table.contains_key("features"),
        _ => false,
    }
}

/// 为缺 `features` 的条目补键；裸字符串条目转为 `{ version = ..., features = [] }`，返回需要移到条目上方的行尾注释。
fn add_features_key(item: &mut Item) -> Option<String> {
    match item {
        Item::Value(Value::InlineTable(inline_table)) => {
            inline_table.insert("features", empty_feature_list_value());
            inline_table.fmt();
            None
        }
        Item::Value(_) => {
            let old_value = match std::mem::replace(item, Item::None) {
                Item::Value(value) => value,
                _ => unreachable!("外层 match 已限定值条目"),
            };
            let relocated_comment = trailing_comment_text(&old_value);
            let mut inline_table = InlineTable::new();
            inline_table.insert("version", old_value);
            inline_table.insert("features", empty_feature_list_value());
            inline_table.fmt();
            *item = Item::Value(Value::InlineTable(inline_table));
            relocated_comment
        }
        Item::Table(sub_table) => {
            sub_table.insert("features", Item::Value(empty_feature_list_value()));
            None
        }
        _ => None,
    }
}

/// 条目现有 features 字符串列表，保序。
fn feature_list_of(table_like: &dyn TableLike) -> Vec<String> {
    table_like
        .get("features")
        .and_then(Item::as_value)
        .and_then(Value::as_array)
        .map(|array| array.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

/// 现有列表保序，追加 default 中未含的项。
fn merge_feature_list(existing_feature_list: &[String], default_feature_list: &[String]) -> Vec<String> {
    let mut merged_feature_list = existing_feature_list.to_vec();
    for feature in default_feature_list {
        if !merged_feature_list.contains(feature) {
            merged_feature_list.push(feature.clone());
        }
    }
    merged_feature_list
}

/// 改写 `features`；已存在则原位替换（decor 窃取保留行尾注释），否则插入。
fn set_feature_list(table_like: &mut dyn TableLike, feature_list: &[String]) {
    match table_like.get_mut("features") {
        Some(Item::Value(old_value)) => {
            let stolen_decor = std::mem::take(old_value.decor_mut());
            *old_value = feature_list_value(feature_list);
            *old_value.decor_mut() = stolen_decor;
        }
        _ => {
            table_like.insert("features", Item::Value(feature_list_value(feature_list)));
        }
    }
}

/// `default-features` 已显式为 `false` 才视为已设置。
fn is_default_features_false(table_like: &dyn TableLike) -> bool {
    table_like.get("default-features").and_then(Item::as_value).and_then(Value::as_bool) == Some(false)
}

/// 改写 `default-features` 为 `false`；已存在则原位替换，否则插入。
fn set_default_features_false(table_like: &mut dyn TableLike) {
    match table_like.get_mut("default-features") {
        Some(Item::Value(old_value)) => {
            let stolen_decor = std::mem::take(old_value.decor_mut());
            *old_value = Value::from(false);
            *old_value.decor_mut() = stolen_decor;
        }
        _ => {
            table_like.insert("default-features", Item::Value(Value::from(false)));
        }
    }
}

/// 字符串列表 → TOML 数组值。
fn feature_list_value(feature_list: &[String]) -> Value {
    let mut array = Array::new();
    for feature in feature_list {
        array.push(feature);
    }
    Value::Array(array)
}

/// 空 features 数组值。
fn empty_feature_list_value() -> Value {
    Value::Array(Array::new())
}

/// 条目行尾注释（trim 后以 `#` 开头的后缀）。
fn trailing_comment_text(value: &Value) -> Option<String> {
    value
        .decor()
        .suffix()
        .and_then(RawString::as_str)
        .map(str::trim)
        .filter(|text| text.starts_with('#'))
        .map(str::to_string)
}

/// 把行尾注释追加为条目键前缀的整行注释；保留既有前缀注释。
fn append_relocated_comment(key: &mut KeyMut<'_>, comment: &str) {
    let mut prefix_text = key.leaf_decor().prefix().and_then(RawString::as_str).unwrap_or("").to_string();
    if !prefix_text.is_empty() && !prefix_text.ends_with('\n') {
        prefix_text.push('\n');
    }
    prefix_text.push_str(comment);
    prefix_text.push('\n');
    key.leaf_decor_mut().set_prefix(prefix_text);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{FakeRegistry, RegistryError};
    use std::cell::RefCell;
    use toml_edit::DocumentMut;
    const SERDE_VERSION_JSON: &str = "{\"version\":{\"num\":\"1.0.228\",\"features\":{\"default\":[\"std\",\"derive\"],\"derive\":[\"std\"]}}}";
    const EMPTY_DEFAULT_JSON: &str = "{\"version\":{\"features\":{\"default\":[]}}}";
    fn serde_registry() -> FakeRegistry {
        let mut registry = FakeRegistry::default();
        registry.response_map.insert(("serde".to_string(), "1.0".to_string()), Ok(Some(SERDE_VERSION_JSON.to_string())));
        registry
    }
    #[test]
    fn fills_missing_features_for_bare_string_entry() {
        let mut document = "[dependencies]\nserde = \"1.0\"\n".parse::<DocumentMut>().unwrap();
        let filled_entry_count = fill_features(document.as_table_mut());
        assert_eq!(filled_entry_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\nserde = { version = \"1.0\", features = [] }\n");
    }
    #[test]
    fn fills_missing_features_for_inherited_entry() {
        let mut document = "[dependencies]\nserde = { workspace = true }\n".parse::<DocumentMut>().unwrap();
        let filled_entry_count = fill_features(document.as_table_mut());
        assert_eq!(filled_entry_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\nserde = { workspace = true, features = [] }\n");
    }
    #[test]
    fn keeps_existing_features_untouched() {
        let input = "[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\n\n[dev-dependencies.tempfile]\nversion = \"3.0\"\nfeatures = [\"x\"]\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let filled_entry_count = fill_features(document.as_table_mut());
        assert_eq!(filled_entry_count, 0);
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn skips_workspace_dependencies_table() {
        let input = "[workspace.dependencies]\nserde = \"1.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let filled_entry_count = fill_features(document.as_table_mut());
        assert_eq!(filled_entry_count, 0);
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn relocates_trailing_comment_when_converting_bare_string() {
        let mut document = "[dependencies]\nserde = \"1.0\" # note\n".parse::<DocumentMut>().unwrap();
        let filled_entry_count = fill_features(document.as_table_mut());
        assert_eq!(filled_entry_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\n# note\nserde = { version = \"1.0\", features = [] }\n");
    }
    #[test]
    fn expands_defaults_for_bare_string_entry() {
        let registry = serde_registry();
        let mut document = "[dependencies]\nserde = \"1.0\"\n".parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 1);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(
            document.to_string(),
            "[dependencies]\nserde = { version = \"1.0\", default-features = false, features = [\"std\", \"derive\"] }\n"
        );
    }
    #[test]
    fn appends_missing_defaults_preserving_existing_order() {
        let registry = serde_registry();
        let mut document = "[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\n".parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 1);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(
            document.to_string(),
            "[dependencies]\nserde = { version = \"1.0\", features = [\"derive\", \"std\"], default-features = false }\n"
        );
    }
    #[test]
    fn fills_features_when_default_features_already_false() {
        let registry = serde_registry();
        let mut document = "[dependencies]\nserde = { version = \"1.0\", default-features = false, features = [] }\n"
            .parse::<DocumentMut>()
            .unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 1);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(
            document.to_string(),
            "[dependencies]\nserde = { version = \"1.0\", default-features = false, features = [\"std\", \"derive\"] }\n"
        );
    }
    #[test]
    fn skips_inherited_entries_without_querying() {
        let registry = serde_registry();
        let input = "[dependencies]\nserde = { workspace = true }\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(document.to_string(), input);
        assert!(registry.query_list.borrow().is_empty());
    }
    #[test]
    fn skips_operator_and_prerelease_versions_with_warning() {
        let registry = serde_registry();
        let input = "[dependencies]\na = \">=1.0\"\nb = \"1.0.3-beta\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        let warning_list = warn_list.into_inner();
        assert_eq!(warning_list.len(), 2);
        assert!(warning_list[0].contains("a"));
        assert!(warning_list[0].contains(">=1.0"));
        assert!(warning_list[1].contains("b"));
        assert_eq!(document.to_string(), input);
        assert!(registry.query_list.borrow().is_empty());
    }
    #[test]
    fn skips_not_found_version_with_warning() {
        let registry = FakeRegistry::default();
        let input = "[dependencies]\nserde = \"1.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        let warning_list = warn_list.into_inner();
        assert_eq!(warning_list.len(), 1);
        assert!(warning_list[0].contains("not found"));
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn skips_registry_failure_with_warning() {
        let mut registry = FakeRegistry::default();
        registry
            .response_map
            .insert(("serde".to_string(), "1.0".to_string()), Err(RegistryError { message: "boom".to_string() }));
        let input = "[dependencies]\nserde = \"1.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        let warning_list = warn_list.into_inner();
        assert_eq!(warning_list.len(), 1);
        assert!(warning_list[0].contains("boom"));
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn skips_empty_default_list_silently() {
        let mut registry = FakeRegistry::default();
        registry.response_map.insert(("serde".to_string(), "1.0".to_string()), Ok(Some(EMPTY_DEFAULT_JSON.to_string())));
        let input = "[dependencies]\nserde = \"1.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(document.to_string(), input);
        assert_eq!(registry.query_list.borrow().len(), 1);
    }
    #[test]
    fn does_not_query_workspace_dependencies() {
        let registry = serde_registry();
        let input = "[workspace.dependencies]\nserde = \"1.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 0);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(document.to_string(), input);
        assert!(registry.query_list.borrow().is_empty());
    }
    #[test]
    fn expands_defaults_for_subtable_dependency() {
        let registry = serde_registry();
        let mut document = "[dependencies.serde]\nversion = \"1.0\"\n".parse::<DocumentMut>().unwrap();
        let warn_list = RefCell::new(Vec::new());
        let applied_entry_count = apply_default_features(document.as_table_mut(), &registry, &mut |warning| warn_list.borrow_mut().push(warning.to_string()));
        assert_eq!(applied_entry_count, 1);
        assert_eq!(warn_list.into_inner(), Vec::<String>::new());
        assert_eq!(
            document.to_string(),
            "[dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"std\", \"derive\"]\ndefault-features = false\n"
        );
    }
}
