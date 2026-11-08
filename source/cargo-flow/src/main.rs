#[derive(clap::Parser)]
#[command(
    name = "cargo-flow",
    version,
    about = "Normalize Cargo.toml structure: dotted-key expansion (always on) plus optional features / default-features / version normalization"
)]
struct Cli {
    #[arg(
        value_name = "PATH",
        default_value = "./Cargo.toml",
        help = "Cargo.toml file or its directory; defaults to ./Cargo.toml; multiple paths processed in order."
    )]
    path_list: Vec<std::path::PathBuf>,
    #[arg(long, help = "Fill missing features = [] on member dependency entries.")]
    features: bool,
    #[arg(long, help = "Rewrite default-features and features from the crates.io default feature list.")]
    default_features: bool,
    #[arg(long, help = "Normalize bare numeric versions to the shortest caret-equivalent form.")]
    normalize_versions: bool,
}

struct CratesIoRegistry;

impl cargo_flow::RegistryFetch for CratesIoRegistry {
    fn version_features_json(&self, name: &str, version: &str) -> Result<Option<String>, cargo_flow::RegistryError> {
        let url = format!("https://crates.io/api/v1/crates/{name}/{version}");
        match ureq::get(&url).set("user-agent", "cargo-flow/0.1.0").call() {
            Ok(response) => response.into_string().map(Some).map_err(|error| cargo_flow::RegistryError { message: error.to_string() }),
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(error) => Err(cargo_flow::RegistryError { message: error.to_string() }),
        }
    }
}

fn main() {
    let cli = <Cli as clap::Parser>::parse();
    let options = cargo_flow::Options {
        features: cli.features,
        default_features: cli.default_features,
        normalize_versions: cli.normalize_versions,
    };
    let registry = CratesIoRegistry;
    for raw_path in &cli.path_list {
        let target_path = if raw_path.is_dir() { raw_path.join("Cargo.toml") } else { raw_path.to_path_buf() };
        let original_text = match std::fs::read_to_string(&target_path) {
            Ok(original_text) => original_text,
            Err(error) => {
                eprintln!("cargo-flow: io error on {}: {error}", target_path.display());
                std::process::exit(1);
            }
        };
        let (rendered_text, summary) = match cargo_flow::Flow::process_text(&original_text, &options, &registry, &mut |warning| eprintln!("{warning}")) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("cargo-flow: {error}");
                std::process::exit(1);
            }
        };
        if rendered_text != original_text
            && let Err(error) = std::fs::write(&target_path, rendered_text)
        {
            eprintln!("cargo-flow: io error on {}: {error}", target_path.display());
            std::process::exit(1);
        }
        let display_path = target_path.display();
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
}
