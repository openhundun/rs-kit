#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub features: bool,
    pub default_features: bool,
    pub normalize_versions: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ChangeSummary {
    pub expanded_key_count: usize,
    pub features_filled_count: usize,
    pub default_features_applied_count: usize,
    pub versions_normalized_count: usize,
}

pub struct Flow;

impl Flow {
    pub fn process_text(original_text: &str, options: &Options, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> Result<(String, ChangeSummary), toml_edit::TomlError> {
        let mut document = original_text.parse::<toml_edit::DocumentMut>()?;
        let summary = Self::process_document(&mut document, options, registry, warn);
        Ok((document.to_string(), summary))
    }

    fn process_document(document: &mut toml_edit::DocumentMut, options: &Options, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> ChangeSummary {
        let expanded_key_count = Expand::dotted_keys(document.as_table_mut());
        let features_filled_count = if options.features { FeatureFlow::fill(document.as_table_mut()) } else { 0 };
        let default_features_applied_count = if options.default_features {
            FeatureFlow::apply_defaults(document.as_table_mut(), registry, warn)
        } else {
            0
        };
        let versions_normalized_count = if options.normalize_versions { VersionFlow::normalize(document.as_table_mut()) } else { 0 };
        ChangeSummary { expanded_key_count, features_filled_count, default_features_applied_count, versions_normalized_count }
    }
}

pub(crate) struct TablePath;

impl TablePath {
    const EXPANDABLE_SEGMENT_LIST: [&str; 4] = ["package", "dependencies", "dev-dependencies", "build-dependencies"];
    const DEPENDENCY_SEGMENT_LIST: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

    pub(crate) fn is_expandable(path: &[String]) -> bool {
        path.last().is_some_and(|segment| Self::EXPANDABLE_SEGMENT_LIST.contains(&segment.as_str()))
    }

    pub(crate) fn is_dependency(path: &[String]) -> bool {
        path.last().is_some_and(|segment| Self::DEPENDENCY_SEGMENT_LIST.contains(&segment.as_str()))
    }

    pub(crate) fn is_member_dependency(path: &[String]) -> bool {
        Self::is_dependency(path) && !path.first().is_some_and(|segment| segment == "workspace")
    }
}

pub(crate) struct TableWalker;

impl TableWalker {
    pub(crate) fn visit(root_table: &mut toml_edit::Table, visitor: &mut dyn FnMut(&mut toml_edit::Table, &[String]) -> usize) -> usize {
        let mut path = Vec::new();
        Self::visit_nested(root_table, &mut path, visitor)
    }

    fn visit_nested(table: &mut toml_edit::Table, path: &mut Vec<String>, visitor: &mut dyn FnMut(&mut toml_edit::Table, &[String]) -> usize) -> usize {
        let ordinary_table_iter = table.iter_mut().filter_map(|(key, item)| {
            let ordinary_sub_table = item.as_table_mut().filter(|sub_table| !sub_table.is_dotted())?;
            Some((key, ordinary_sub_table))
        });
        ordinary_table_iter
            .map(|(key, sub_table)| {
                path.push(key.get().to_string());
                let current_count = visitor(sub_table, path);
                let descendant_count = Self::visit_nested(sub_table, path, visitor);
                path.pop();
                current_count + descendant_count
            })
            .sum()
    }
}

pub(crate) struct TomlUtil;

impl TomlUtil {
    pub(crate) fn dotted_table_into_value(dotted_table: toml_edit::Table) -> toml_edit::Value {
        let mut inline_table = toml_edit::InlineTable::new();
        dotted_table.into_iter().map(|(key, item)| (key, Self::item_into_inline_value(item))).for_each(|(key, value)| {
            inline_table.insert(key, value);
        });
        inline_table.fmt();
        toml_edit::Value::InlineTable(inline_table)
    }

    pub(crate) fn item_into_inline_value(item: toml_edit::Item) -> toml_edit::Value {
        match item {
            toml_edit::Item::Value(value) => value,
            toml_edit::Item::Table(nested_dotted) => Self::dotted_table_into_value(nested_dotted),
            toml_edit::Item::None | toml_edit::Item::ArrayOfTables(_) => unreachable!("dotted key leaf must be a value or nested dotted table"),
        }
    }

    pub(crate) fn feature_list_value(feature_list: &[String]) -> toml_edit::Value {
        let mut array = toml_edit::Array::new();
        feature_list.iter().for_each(|feature| {
            array.push(feature);
        });
        toml_edit::Value::Array(array)
    }

    pub(crate) fn empty_feature_list_value() -> toml_edit::Value {
        toml_edit::Value::Array(toml_edit::Array::new())
    }

    pub(crate) fn trailing_comment_text(value: &toml_edit::Value) -> Option<String> {
        value
            .decor()
            .suffix()
            .and_then(toml_edit::RawString::as_str)
            .map(str::trim)
            .filter(|text| text.starts_with('#'))
            .map(str::to_string)
    }

    pub(crate) fn append_relocated_comment(key: &mut toml_edit::KeyMut<'_>, comment: &str) {
        let mut prefix_text = key.leaf_decor().prefix().and_then(toml_edit::RawString::as_str).unwrap_or("").to_string();
        if !prefix_text.is_empty() && !prefix_text.ends_with('\n') {
            prefix_text.push('\n');
        }
        prefix_text.push_str(comment);
        prefix_text.push('\n');
        key.leaf_decor_mut().set_prefix(prefix_text);
    }
}

pub(crate) struct Expand;

impl Expand {
    pub(crate) fn dotted_keys(root_table: &mut toml_edit::Table) -> usize {
        TableWalker::visit(root_table, &mut |table, path| {
            if TablePath::is_expandable(path) {
                let dotted_entry_iter = table.iter_mut().filter_map(|(mut key, item)| {
                    let dotted_table = item.as_table_mut().filter(|table| table.is_dotted())?;
                    let key_count = dotted_table.len();
                    let comment_iter = dotted_table.iter().flat_map(|(leaf_key, leaf_item)| {
                        let prefix_text = dotted_table.key(leaf_key).and_then(|key| key.leaf_decor().prefix()).and_then(toml_edit::RawString::as_str);
                        let prefix_comment_iter = prefix_text.into_iter().flat_map(str::lines).map(str::trim).filter(|line| line.starts_with('#'));
                        let trailing_text = leaf_item.as_value().and_then(|value| value.decor().suffix()).and_then(toml_edit::RawString::as_str);
                        let trailing_comment = trailing_text.map(str::trim).filter(|text| text.starts_with('#'));
                        prefix_comment_iter.chain(trailing_comment)
                    });
                    let comment_list: Vec<String> = comment_iter.map(str::to_string).collect();
                    let inline_value = TomlUtil::dotted_table_into_value(std::mem::take(dotted_table));
                    *item = toml_edit::Item::Value(inline_value);
                    if !comment_list.is_empty() {
                        key.leaf_decor_mut().set_prefix(comment_list.join("\n") + "\n");
                    }
                    Some(key_count)
                });
                dotted_entry_iter.sum()
            } else {
                0
            }
        })
    }
}

pub(crate) struct FeatureFlow;

impl FeatureFlow {
    pub(crate) fn fill(root_table: &mut toml_edit::Table) -> usize {
        TableWalker::visit(root_table, &mut |table, path| {
            if TablePath::is_member_dependency(path) {
                table
                    .iter_mut()
                    .filter(|(_, item)| {
                        let item: &toml_edit::Item = item;
                        match item {
                            toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table)) => !inline_table.contains_key("features"),
                            toml_edit::Item::Table(sub_table) => !sub_table.contains_key("features"),
                            _ => true,
                        }
                    })
                    .map(|(mut key, item)| {
                        let relocated_comment = match item {
                            toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table)) => {
                                inline_table.insert("features", TomlUtil::empty_feature_list_value());
                                inline_table.fmt();
                                None
                            }
                            toml_edit::Item::Value(_) => {
                                let old_value = match std::mem::replace(item, toml_edit::Item::None) {
                                    toml_edit::Item::Value(value) => value,
                                    _ => unreachable!("outer match already restricts to value items"),
                                };
                                let relocated_comment = TomlUtil::trailing_comment_text(&old_value);
                                let mut inline_table = toml_edit::InlineTable::new();
                                inline_table.insert("version", old_value);
                                inline_table.insert("features", TomlUtil::empty_feature_list_value());
                                inline_table.fmt();
                                *item = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table));
                                relocated_comment
                            }
                            toml_edit::Item::Table(sub_table) => {
                                sub_table.insert("features", toml_edit::Item::Value(TomlUtil::empty_feature_list_value()));
                                None
                            }
                            _ => None,
                        };
                        if let Some(relocated_comment) = relocated_comment {
                            TomlUtil::append_relocated_comment(&mut key, &relocated_comment);
                        }
                        1
                    })
                    .sum::<usize>()
            } else {
                0
            }
        })
    }

    pub(crate) fn apply_defaults(root_table: &mut toml_edit::Table, registry: &dyn RegistryFetch, warn: &mut dyn FnMut(&str)) -> usize {
        TableWalker::visit(root_table, &mut |table, path| {
            if TablePath::is_member_dependency(path) {
                table
                    .iter_mut()
                    .map(|(mut key, item)| {
                        let dependency_name = key.get();
                        let (explicit_version, inherited) = match item {
                            toml_edit::Item::Value(toml_edit::Value::String(string_value)) => (Some(string_value.value().clone()), false),
                            toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table)) => (
                                inline_table.get("version").and_then(toml_edit::Value::as_str).map(str::to_string),
                                inline_table.get("workspace").and_then(toml_edit::Value::as_bool) == Some(true),
                            ),
                            toml_edit::Item::Table(sub_table) => (
                                sub_table.get("version").and_then(toml_edit::Item::as_str).map(str::to_string),
                                sub_table.get("workspace").and_then(toml_edit::Item::as_bool) == Some(true),
                            ),
                            _ => (None, false),
                        };
                        let default_feature_list = explicit_version
                            .filter(|_| !inherited)
                            .filter(|version_text| {
                                let is_bare_numeric = VersionUtil::bare_numeric_version_segment_list(version_text).is_some();
                                if !is_bare_numeric {
                                    warn(&format!("cargo-flow: skipping {dependency_name}: version '{version_text}' is not a bare numeric requirement"));
                                }
                                is_bare_numeric
                            })
                            .and_then(|version_text| match RegistryFetch::version_features_json(registry, dependency_name, &version_text) {
                                Ok(Some(json_text)) => VersionPayload::default_features_from_json(&json_text),
                                Ok(None) => {
                                    warn(&format!("cargo-flow: skipping {dependency_name}: {dependency_name}/{version_text} not found on crates.io"));
                                    None
                                }
                                Err(error) => {
                                    warn(&format!("cargo-flow: skipping {dependency_name}: registry fetch failed: {error}"));
                                    None
                                }
                            })
                            .filter(|feature_list| !feature_list.is_empty());
                        default_feature_list.map_or(0, |default_feature_list| {
                            let changed = match item {
                                toml_edit::Item::Value(toml_edit::Value::String(_)) => {
                                    let old_value = match std::mem::replace(item, toml_edit::Item::None) {
                                        toml_edit::Item::Value(value) => value,
                                        _ => unreachable!("outer match already restricts to value items"),
                                    };
                                    let relocated_comment = TomlUtil::trailing_comment_text(&old_value);
                                    let mut inline_table = toml_edit::InlineTable::new();
                                    inline_table.insert("version", old_value);
                                    inline_table.insert("default-features", toml_edit::Value::from(false));
                                    inline_table.insert("features", TomlUtil::feature_list_value(&default_feature_list));
                                    inline_table.fmt();
                                    *item = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table));
                                    if let Some(comment) = relocated_comment {
                                        TomlUtil::append_relocated_comment(&mut key, &comment);
                                    }
                                    true
                                }
                                toml_edit::Item::Value(toml_edit::Value::InlineTable(_)) | toml_edit::Item::Table(_) => {
                                    let changed = item.as_table_like_mut().is_some_and(|table_like| {
                                        let feature_array = table_like.get("features").and_then(toml_edit::Item::as_value).and_then(toml_edit::Value::as_array);
                                        let existing_feature_list: Vec<String> = feature_array
                                            .map(|array| array.iter().filter_map(toml_edit::Value::as_str).map(str::to_string).collect())
                                            .unwrap_or_default();
                                        let merged_feature_list = default_feature_list.iter().fold(existing_feature_list.to_vec(), |mut merged_feature_list, feature| {
                                            if !merged_feature_list.contains(feature) {
                                                merged_feature_list.push(feature.clone());
                                            }
                                            merged_feature_list
                                        });
                                        let features_changed = merged_feature_list != existing_feature_list;
                                        if features_changed {
                                            match table_like.get_mut("features") {
                                                Some(toml_edit::Item::Value(old_value)) => {
                                                    let stolen_decor = std::mem::take(old_value.decor_mut());
                                                    *old_value = TomlUtil::feature_list_value(&merged_feature_list);
                                                    *old_value.decor_mut() = stolen_decor;
                                                }
                                                _ => {
                                                    table_like.insert("features", toml_edit::Item::Value(TomlUtil::feature_list_value(&merged_feature_list)));
                                                }
                                            }
                                        }
                                        let default_features_changed = table_like.get("default-features").and_then(toml_edit::Item::as_value).and_then(toml_edit::Value::as_bool) != Some(false);
                                        if default_features_changed {
                                            match table_like.get_mut("default-features") {
                                                Some(toml_edit::Item::Value(old_value)) => {
                                                    let stolen_decor = std::mem::take(old_value.decor_mut());
                                                    *old_value = toml_edit::Value::from(false);
                                                    *old_value.decor_mut() = stolen_decor;
                                                }
                                                _ => {
                                                    table_like.insert("default-features", toml_edit::Item::Value(toml_edit::Value::from(false)));
                                                }
                                            }
                                        }
                                        features_changed || default_features_changed
                                    });
                                    if changed && let toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table)) = item {
                                        inline_table.fmt();
                                    }
                                    changed
                                }
                                _ => false,
                            };
                            usize::from(changed)
                        })
                    })
                    .sum::<usize>()
            } else {
                0
            }
        })
    }
}

pub(crate) struct VersionFlow;

impl VersionFlow {
    pub(crate) fn normalize(root_table: &mut toml_edit::Table) -> usize {
        TableWalker::visit(root_table, &mut |table, path| {
            if TablePath::is_dependency(path) {
                let dependency_item_iter = table.iter_mut().map(|(_, item)| item);
                dependency_item_iter
                    .map(|item| match item {
                        toml_edit::Item::Value(toml_edit::Value::InlineTable(inline_table)) => {
                            let version_field = inline_table.get_mut("version");
                            version_field.map_or(0, |version_value| usize::from(Self::normalize_version_value(version_value)))
                        }
                        toml_edit::Item::Table(sub_table) => {
                            let version_field = sub_table.get_mut("version");
                            version_field.map_or(0, |version_item| usize::from(Self::normalize_version_item(version_item)))
                        }
                        toml_edit::Item::Value(_) => usize::from(Self::normalize_version_item(item)),
                        _ => 0,
                    })
                    .sum()
            } else {
                0
            }
        })
    }

    fn normalize_version_item(item: &mut toml_edit::Item) -> bool {
        match item.as_value_mut() {
            Some(value) => Self::normalize_version_value(value),
            None => false,
        }
    }

    fn normalize_version_value(old_value: &mut toml_edit::Value) -> bool {
        let normalized_text = old_value.as_str().and_then(VersionUtil::normalize_version_text);
        normalized_text.is_some_and(|normalized_text| {
            let replacement = toml_edit::value(normalized_text);
            let toml_edit::Item::Value(new_value) = replacement else {
                unreachable!("value() produces a value item");
            };
            let stolen_decor = std::mem::take(old_value.decor_mut());
            *old_value = new_value;
            *old_value.decor_mut() = stolen_decor;
            true
        })
    }
}

pub(crate) struct VersionUtil;

impl VersionUtil {
    pub(crate) fn bare_numeric_version_segment_list(version_text: &str) -> Option<Vec<u64>> {
        let segment_count = version_text.split('.').take(4).count();
        (segment_count <= 3).then_some(())?;
        version_text.split('.').try_fold(Vec::with_capacity(segment_count), |mut number_list, segment| {
            (!segment.is_empty() && segment.bytes().all(|byte| byte.is_ascii_digit())).then_some(())?;
            number_list.push(segment.parse::<u64>().ok()?);
            Some(number_list)
        })
    }

    pub(crate) fn normalize_version_text(version_text: &str) -> Option<String> {
        let number_list = Self::bare_numeric_version_segment_list(version_text)?;
        let normalized_segment_count = match number_list.as_slice() {
            [major, 0] | [major, 0, 0] if *major != 0 => 1,
            [major, minor, 0] if *major != 0 || *minor != 0 => 2,
            _ => number_list.len(),
        };
        let has_redundant_trailing_zeros = normalized_segment_count != number_list.len();
        let needs_normalization = has_redundant_trailing_zeros || version_text.split('.').any(|segment| segment.len() > 1 && segment.starts_with('0'));
        needs_normalization.then(|| match &number_list[..normalized_segment_count] {
            [major] => major.to_string(),
            [major, minor] => format!("{major}.{minor}"),
            [major, minor, patch] => format!("{major}.{minor}.{patch}"),
            _ => unreachable!("segment count is between 1 and 3"),
        })
    }
}

#[derive(Clone, Debug)]
pub struct RegistryError {
    pub message: String,
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RegistryError {}

pub trait RegistryFetch {
    fn version_features_json(&self, name: &str, version: &str) -> Result<Option<String>, RegistryError>;
}

pub(crate) struct VersionPayload;

impl VersionPayload {
    pub(crate) fn default_features_from_json(json_text: &str) -> Option<Vec<String>> {
        let response = serde_json::from_str::<VersionResponse>(json_text).ok()?;
        response.version.features.get("default").cloned()
    }
}

#[derive(Debug, serde::Deserialize)]
struct VersionResponse {
    version: VersionRecord,
}

#[derive(Debug, serde::Deserialize)]
struct VersionRecord {
    #[serde(default)]
    features: std::collections::HashMap<String, Vec<String>>,
}
