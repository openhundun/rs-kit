//! 版本号归一：裸数字版本去掉尾部 `.0` 至最短 caret 等价形。
//!
//! ## 规则
//! cargo 的裸版本即 caret requirement，仅在 caret 区间不变时收缩；`^0.0.4` 与 `^0.0` 不等价，故 `0.0.x`、`0.0`、`0.0.0` 保持不变。
//!
//! ```text
//! 1.2.0 → 1.2    1.0.0 → 1    0.2.0 → 0.2    3.27.0 → 3.27
//! 1.2.3 / 0.0.4 / 0.0.0 / 0.0 / 0 保持不变
//! ```

use toml_edit::{Item, Table, Value};

use crate::{is_dependency_table, visit_tables};

/// 归一化依赖表条目的 `version`（含 `[workspace.dependencies]`，不含 `[package].version`），返回改写个数。
pub(crate) fn normalize_versions(root_table: &mut Table) -> usize {
    let mut normalized_version_count = 0usize;
    visit_tables(root_table, &mut |table, path| {
        if !is_dependency_table(path) {
            return;
        }
        for (_, item) in table.iter_mut() {
            normalized_version_count += normalize_entry_version(item);
        }
    });
    normalized_version_count
}

/// 条目级归一：裸字符串条目整体即版本，内联表与子表取 `version` 字段。
fn normalize_entry_version(item: &mut Item) -> usize {
    match item {
        Item::Value(Value::InlineTable(inline_table)) => inline_table.get_mut("version").map_or(0, |version_value| usize::from(normalize_version_value(version_value))),
        Item::Table(sub_table) => sub_table.get_mut("version").map_or(0, |version_item| usize::from(normalize_version_item(version_item))),
        Item::Value(_) => usize::from(normalize_version_item(item)),
        _ => 0,
    }
}

/// 原位改写一个字符串版本项；保留条目 decor（行尾注释不丢），返回是否改写。
fn normalize_version_item(item: &mut Item) -> bool {
    match item.as_value_mut() {
        Some(value) => normalize_version_value(value),
        None => false,
    }
}

fn normalize_version_value(old_value: &mut Value) -> bool {
    let Value::String(formatted_string) = old_value else {
        return false;
    };
    let Some(normalized_text) = normalize_version_text(formatted_string.value()) else {
        return false;
    };
    let replacement = toml_edit::value(normalized_text);
    let Item::Value(new_value) = replacement else {
        unreachable!("value() 产生值条目");
    };
    let stolen_decor = std::mem::take(old_value.decor_mut());
    *old_value = new_value;
    *old_value.decor_mut() = stolen_decor;
    true
}

/// 裸数字版本判定：1-3 个全数字段，返回分段数值；非裸数字（带操作符、pre-release、`*` 等）返回 `None`。
pub(crate) fn bare_numeric_version_segment_list(version_text: &str) -> Option<Vec<u64>> {
    let segment_list: Vec<&str> = version_text.split('.').collect();
    if segment_list.len() > 3 {
        return None;
    }
    let mut number_list = Vec::with_capacity(segment_list.len());
    for segment in segment_list {
        if segment.is_empty() || !segment.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        number_list.push(segment.parse::<u64>().ok()?);
    }
    Some(number_list)
}

/// 最短 caret 等价形；不变或非裸数字返回 `None`。
pub(crate) fn normalize_version_text(version_text: &str) -> Option<String> {
    let mut number_list = bare_numeric_version_segment_list(version_text)?;
    loop {
        let segment_count = number_list.len();
        if segment_count <= 1 || number_list.last().is_none_or(|last_number| *last_number != 0) {
            break;
        }
        match segment_count {
            3 => {
                if number_list[0] == 0 && number_list[1] == 0 {
                    break;
                }
            }
            2 => {
                if number_list[0] == 0 {
                    break;
                }
            }
            _ => unreachable!("段数不超过 3"),
        }
        number_list.pop();
    }
    let normalized_text = number_list.iter().map(u64::to_string).collect::<Vec<_>>().join(".");
    (normalized_text != version_text).then_some(normalized_text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::DocumentMut;
    #[test]
    fn shortens_trailing_zero_versions_to_shortest_caret_form() {
        let case_list = [
            ("1.2.0", "1.2"),
            ("1.0.0", "1"),
            ("0.2.0", "0.2"),
            ("3.27.0", "3.27"),
            ("1.2.3", "1.2.3"),
            ("0.0.4", "0.0.4"),
            ("0.0.0", "0.0.0"),
            ("0.0", "0.0"),
            ("0", "0"),
            ("=1.2.3", "=1.2.3"),
            (">=1.0", ">=1.0"),
            ("1.*", "1.*"),
            ("1.2.3-beta", "1.2.3-beta"),
            ("1.2.3 ", "1.2.3 "),
        ];
        for (version_text, expected_text) in case_list {
            let actual_text = normalize_version_text(version_text).unwrap_or_else(|| version_text.to_string());
            assert_eq!(actual_text, expected_text);
        }
    }
    #[test]
    fn normalizes_versions_in_document_forms() {
        let input = "[package]\nname = \"demo\"\nversion = \"1.2.0\"\n\n[dependencies]\nserde = \"1.2.0\"\nserde_json = { version = \"1.0.0\", features = [\"x\"] }\n\n[workspace.dependencies]\ntempfile = \"3.1.0\"\n";
        let expected = "[package]\nname = \"demo\"\nversion = \"1.2.0\"\n\n[dependencies]\nserde = \"1.2\"\nserde_json = { version = \"1\", features = [\"x\"] }\n\n[workspace.dependencies]\ntempfile = \"3.1\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let normalized_version_count = normalize_versions(document.as_table_mut());
        assert_eq!(normalized_version_count, 3);
        assert_eq!(document.to_string(), expected);
    }
    #[test]
    fn leaves_package_version_untouched() {
        let input = "[package]\nversion = \"1.2.0\"\n\n[workspace.package]\nversion = \"1.0.0\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let normalized_version_count = normalize_versions(document.as_table_mut());
        assert_eq!(normalized_version_count, 0);
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn keeps_trailing_comments_when_normalizing() {
        let input = "[dependencies]\nserde = \"1.2.0\" # keep me\n";
        let expected = "[dependencies]\nserde = \"1.2\" # keep me\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let normalized_version_count = normalize_versions(document.as_table_mut());
        assert_eq!(normalized_version_count, 1);
        assert_eq!(document.to_string(), expected);
    }
    #[test]
    fn normalizes_subtable_dependency_versions() {
        let input = "[dependencies.serde]\nversion = \"1.2.0\"\n";
        let expected = "[dependencies.serde]\nversion = \"1.2\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let normalized_version_count = normalize_versions(document.as_table_mut());
        assert_eq!(normalized_version_count, 1);
        assert_eq!(document.to_string(), expected);
    }
}
