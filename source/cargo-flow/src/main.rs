use std::path::{Path, PathBuf};

use cargo_flow::{ChangeSummary, CratesIoRegistry, Options, process_path};
use clap::Parser;

#[derive(Parser)]
#[command(name = "cargo-flow", version, about = "对 Cargo.toml 做结构归一：点键展开（核心，始终执行）+ 可选 features / default-features / 版本号归一")]
struct Cli {
    /// Cargo.toml 文件或其所在目录；缺省 ./Cargo.toml，多个按顺序处理。
    #[arg(value_name = "PATH", default_value = "./Cargo.toml")]
    path_list: Vec<PathBuf>,
    /// 为成员侧依赖条目补全 features = []。
    #[arg(long)]
    features: bool,
    /// 按 crates.io 的 default 特性改写 default-features 与 features。
    #[arg(long)]
    default_features: bool,
    /// 归一化裸数字版本号为最短 caret 等价形。
    #[arg(long)]
    normalize_versions: bool,
}

fn main() {
    let cli = Cli::parse();
    let options = Options {
        features: cli.features,
        default_features: cli.default_features,
        normalize_versions: cli.normalize_versions,
    };
    let registry = CratesIoRegistry;
    for raw_path in &cli.path_list {
        let target_path = resolve_target_path(raw_path);
        match process_path(&target_path, &options, &registry, &mut |warning| eprintln!("{warning}")) {
            Ok(summary) => print_change_summary(&summary, &target_path),
            Err(error) => {
                eprintln!("cargo-flow: {error}");
                std::process::exit(1);
            }
        }
    }
}

/// 文件参数原样使用，目录参数取 `<dir>/Cargo.toml`。
fn resolve_target_path(raw_path: &Path) -> PathBuf {
    if raw_path.is_dir() { raw_path.join("Cargo.toml") } else { raw_path.to_path_buf() }
}

/// 按功能打印计数摘要；无变化的项不打印。
fn print_change_summary(summary: &ChangeSummary, path: &Path) {
    let display_path = path.display();
    if summary.expanded_key_count > 0 {
        println!("expanded {} dotted keys in {display_path}", summary.expanded_key_count);
    }
    if summary.features_filled_count > 0 {
        println!("filled {} features keys in {display_path}", summary.features_filled_count);
    }
    if summary.default_features_applied_count > 0 {
        println!("applied default-features to {} entries in {display_path}", summary.default_features_applied_count);
    }
    if summary.versions_normalized_count > 0 {
        println!("normalized {} versions in {display_path}", summary.versions_normalized_count);
    }
}
