//! 点键展开：白名单表内的点键合并为内联表条目（核心功能，始终执行）。
//!
//! ## 行为
//! 同一根键的全部点键合并为一个内联表条目：子键保持原出现顺序，值原样平移；嵌套点键（`a.b.c`）转为语义等价的嵌套内联表。被折叠行上方的整行注释与行尾注释按原顺序移到新条目上方，注释文本不丢弃。新条目在原位置替换点键条目，文件其余字节经 round-trip 原样保留。
//!
//! ```toml
//! serde.workspace = true
//! serde.features = ["derive"] # note
//! ```
//!
//! 展开为 `serde = { workspace = true, features = ["derive"] }`，`# note` 移到新条目上方。

use toml_edit::{InlineTable, Item, RawString, Table, Value};

use crate::{is_expandable_table, visit_tables};

/// 展开所有白名单表内的点键，返回被折叠的点键条数（每个点键路径计一）。
pub(crate) fn expand_dotted_keys(root_table: &mut Table) -> usize {
    let mut expanded_key_count = 0usize;
    visit_tables(root_table, &mut |table, path| {
        if !is_expandable_table(path) {
            return;
        }
        let dotted_key_list: Vec<String> = table
            .iter()
            .filter(|(_, item)| matches!(item, Item::Table(dotted) if dotted.is_dotted()))
            .map(|(key, _)| key.to_string())
            .collect();
        for root_key in dotted_key_list {
            let Some(Item::Table(dotted_ref)) = table.get(&root_key) else {
                continue;
            };
            if !dotted_ref.is_dotted() {
                continue;
            }
            let dotted_table = dotted_ref.clone();
            expanded_key_count += dotted_table.len();
            let comment_list = collect_comment_list(&dotted_table);
            let inline_value = dotted_table_into_value(dotted_table);
            table.insert(&root_key, Item::Value(inline_value));
            if !comment_list.is_empty() {
                if let Some(mut key) = table.key_mut(&root_key) {
                    key.leaf_decor_mut().set_prefix(comment_list.join("\n") + "\n");
                }
            }
        }
    });
    expanded_key_count
}

/// 按原出现顺序收集点键组的注释：叶子键前缀中的整行注释与叶子值的行尾注释。
fn collect_comment_list(dotted_table: &Table) -> Vec<String> {
    let mut comment_list = Vec::new();
    for (leaf_key, leaf_item) in dotted_table.iter() {
        let prefix_text = dotted_table.key(leaf_key).and_then(|key| key.leaf_decor().prefix()).and_then(RawString::as_str);
        if let Some(text) = prefix_text {
            for line in text.lines() {
                let trimmed_line = line.trim();
                if trimmed_line.starts_with('#') {
                    comment_list.push(trimmed_line.to_string());
                }
            }
        }
        if let Item::Value(value) = leaf_item {
            let trailing_text = value.decor().suffix().and_then(RawString::as_str).map(str::trim).filter(|text| text.starts_with('#'));
            if let Some(text) = trailing_text {
                comment_list.push(text.to_string());
            }
        }
    }
    comment_list
}

/// 把点键表折叠为内联表值；嵌套点键递归折叠为嵌套内联表。
fn dotted_table_into_value(dotted_table: Table) -> Value {
    let mut inline_table = InlineTable::new();
    for (key, item) in dotted_table {
        inline_table.insert(key, item_into_inline_value(item));
    }
    inline_table.fmt();
    Value::InlineTable(inline_table)
}

fn item_into_inline_value(item: Item) -> Value {
    match item {
        Item::Value(value) => value,
        Item::Table(nested_dotted) => dotted_table_into_value(nested_dotted),
        Item::None | Item::ArrayOfTables(_) => unreachable!("点键的叶子只能是值或嵌套点键"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml_edit::DocumentMut;
    #[test]
    fn expands_single_dotted_key() {
        let mut document = "[dependencies]\nserde.workspace = true\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\nserde = { workspace = true }\n");
    }
    #[test]
    fn merges_dotted_keys_of_same_root_in_original_order() {
        let mut document = "[dependencies]\nserde.workspace = true\nserde.features = [\"derive\"]\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 2);
        assert_eq!(document.to_string(), "[dependencies]\nserde = { workspace = true, features = [\"derive\"] }\n");
    }
    #[test]
    fn expands_multiple_whitelisted_tables() {
        let mut document = "[package]\nedition.workspace = true\n\n[dependencies]\nserde.workspace = true\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 2);
        assert_eq!(document.to_string(), "[package]\nedition = { workspace = true }\n\n[dependencies]\nserde = { workspace = true }\n");
    }
    #[test]
    fn expands_target_dependency_tables() {
        let mut document = "[target.'cfg(unix)'.dependencies]\nserde.workspace = true\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 1);
        assert_eq!(document.to_string(), "[target.'cfg(unix)'.dependencies]\nserde = { workspace = true }\n");
    }
    #[test]
    fn expands_workspace_package_table() {
        let mut document = "[workspace.package]\nedition.workspace = true\n\n[workspace.dependencies]\nserde.workspace = true\n"
            .parse::<DocumentMut>()
            .unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 2);
        assert_eq!(
            document.to_string(),
            "[workspace.package]\nedition = { workspace = true }\n\n[workspace.dependencies]\nserde = { workspace = true }\n"
        );
    }
    #[test]
    fn keeps_workspace_false_value_as_is() {
        let mut document = "[dependencies]\nserde.workspace = false\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\nserde = { workspace = false }\n");
    }
    #[test]
    fn leaves_non_whitelisted_tables_untouched() {
        let input = "[features]\nspecial.deck = []\n\n[package.metadata]\nnotes.owner = \"me\"\n";
        let mut document = input.parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 0);
        assert_eq!(document.to_string(), input);
    }
    #[test]
    fn preserves_comments_in_original_order() {
        let mut document = "[dependencies]\n# c1\nserde.workspace = true\nserde.features = [\"derive\"] # c2\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 2);
        assert_eq!(document.to_string(), "[dependencies]\n# c1\n# c2\nserde = { workspace = true, features = [\"derive\"] }\n");
    }
    #[test]
    fn expands_deep_dotted_keys_as_nested_inline_tables() {
        let mut document = "[dependencies]\na.b.c = 1\n".parse::<DocumentMut>().unwrap();
        let expanded_key_count = expand_dotted_keys(document.as_table_mut());
        assert_eq!(expanded_key_count, 1);
        assert_eq!(document.to_string(), "[dependencies]\na = { b = { c = 1 } }\n");
    }
}
