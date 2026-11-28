//! The models and engines that can be suggested: `catalog.json`, a reviewed copy of
//! local-llm-radar's catalog. Every download in it is pinned (a revision, a size and a SHA-256).

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

/// The schema this code reads; another one is refused (and tested).
pub const SCHEMA: u32 = 1;

static CATALOG: OnceLock<Catalog> = OnceLock::new();

/// The catalog that ships with the app.
///
/// # Panics
/// If `catalog.json` does not parse, which a test rules out.
pub fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(|| {
        let catalog: Catalog =
            serde_json::from_str(include_str!("../catalog.json")).expect("catalog.json parses");
        assert_eq!(catalog.schema, SCHEMA, "catalog.json has another schema");
        catalog
    })
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub schema: u32,
    /// The local-llm-radar edition it was made from, like `2026-11-27`.
    pub edition: String,
    pub engines: Vec<Engine>,
    pub models: Vec<Model>,
}

impl Catalog {
    pub fn engine(&self, id: &str) -> Option<&Engine> {
        self.engines.iter().find(|engine| engine.id == id)
    }

    pub fn model(&self, id: &str) -> Option<&Model> {
        self.models.iter().find(|model| model.id == id)
    }
}

/// How much it takes to install an engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    /// Downloaded and unpacked, or locked Python packages: nothing to approve.
    Easy,
    /// Built from source on this computer, with tools it must have.
    Involved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Gguf,
    Mlx,
    /// `TurboFieldfare`'s own layout, made by its installer.
    Gturbo,
}

#[derive(Debug, Deserialize)]
pub struct Engine {
    pub id: String,
    pub name: String,
    pub license: String,
    pub effort: Effort,
    /// Whether it applies a request's `chat_template_kwargs`, which is how pi sets thinking.
    #[serde(default)]
    pub template_kwargs: bool,
    pub formats: Vec<Format>,
    /// Its command line; `{bin}`, `{python}`, `{model}`, `{model_dir}`, `{upstream}`, `{id}` (the
    /// model's id, the name pi sees), `{port}` and `{ctx}` are filled in.
    pub launch: Vec<String>,
    /// Answers 200 once the model is loaded.
    pub health: String,
    /// Share of the memory bandwidth it turns into decoding, by accelerator (`metal`, `cuda`,
    /// `vulkan`, `cpu`); above 1 where speculative decoding is built in. For estimates only.
    pub efficiency: BTreeMap<String, f64>,
    /// When not empty, the only models (by id) it runs.
    #[serde(default)]
    pub families: Vec<String>,
    /// Reads mixture-of-experts weights from the disk as needed, so a model larger than the
    /// memory still runs.
    #[serde(default)]
    pub streams_experts: bool,
    #[serde(default)]
    pub requires: Requires,
    pub sources: Vec<String>,
    /// Where it runs: `<os>-<arch>-<accelerator>`, like `macos-arm64-metal`.
    pub platforms: Vec<String>,
    pub install: Install,
}

impl Engine {
    pub fn runs(&self, model: &Model) -> bool {
        self.families.is_empty() || self.families.contains(&model.id)
    }

    /// The engine's project page: the GitHub repository it is downloaded from, else its first
    /// source.
    pub fn home_page(&self) -> Option<String> {
        let pinned = match &self.install {
            Install::Archive { assets, .. } => {
                assets.values().next().map(|asset| asset.url.as_str())
            }
            Install::Build { source, .. } => Some(source.url.as_str()),
            Install::Python { .. } => None,
        };
        pinned
            .into_iter()
            .chain(self.sources.iter().map(String::as_str))
            .find_map(github_repo)
            .or_else(|| self.sources.first().cloned())
    }

    /// The version installed, for its folder name.
    pub fn version(&self) -> &str {
        match &self.install {
            Install::Archive { version, .. }
            | Install::Build { version, .. }
            | Install::Python { version, .. } => version,
        }
    }
}

/// `https://github.com/<owner>/<repo>` of any GitHub URL in a repository.
fn github_repo(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/').filter(|part| !part.is_empty());
    let (owner, repo) = (parts.next()?, parts.next()?);
    Some(format!("https://github.com/{owner}/{repo}"))
}

#[derive(Debug, Default, Deserialize)]
pub struct Requires {
    /// Like `26.4`.
    pub min_macos: Option<String>,
    /// The oldest Apple chip generation (3 for M3).
    pub apple_chip_min: Option<u32>,
    #[serde(default)]
    pub avx512: bool,
    /// Programs it needs on the `PATH` (to build it).
    #[serde(default)]
    pub tools: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Install {
    /// A release archive per platform, with `binary` somewhere inside.
    Archive {
        binary: String,
        version: String,
        assets: BTreeMap<String, Asset>,
    },
    /// Source at a commit, built with `steps` (shell commands run in the unpacked source).
    Build {
        binary: String,
        version: String,
        source: Source,
        steps: Vec<String>,
        /// Fetches the model in the engine's own format (`{model_dir}` is filled in).
        model_step: Option<String>,
    },
    /// Locked Python packages, installed with a private Python.
    Python {
        version: String,
        /// A `requirements.txt` with the hash of every file.
        requirements: String,
        packages_size: u64,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub url: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    /// GitHub makes the tarball on request, so the checksum is of the uncompressed tar.
    pub url: String,
    pub size: u64,
    pub tar_sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub released: String,
    pub license: String,
    pub why: String,
    pub params: String,
    pub total_params_b: f64,
    pub active_params_b: f64,
    pub context: u64,
    /// Curated rank of how useful it is for an agent (0-100). Only compared, never shown.
    pub quality: u32,
    pub vision: bool,
    /// Whether it can think before it answers; `None` when the catalog does not say.
    #[serde(default)]
    pub reasoning: Option<bool>,
    /// The `reasoning_effort` its chat template takes for each of pi's thinking levels; empty
    /// when thinking is only on or off (`enable_thinking`).
    #[serde(default)]
    pub thinking_levels: BTreeMap<String, String>,
    pub tool_calling: bool,
    pub sources: Vec<String>,
    pub builds: Vec<Build>,
    pub speeds: Vec<Speed>,
}

impl Model {
    pub fn build(&self, id: &str) -> Option<&Build> {
        self.builds.iter().find(|build| build.id == id)
    }

    pub fn is_moe(&self) -> bool {
        self.active_params_b * 2.0 < self.total_params_b
    }

    /// Small enough for nearly any computer: at most [`TINY_PARAMS_B`] billion parameters
    /// (about 2 GB or less at 4 bits).
    pub fn is_tiny(&self) -> bool {
        self.total_params_b <= TINY_PARAMS_B
    }
}

/// The most parameters (billions) of a model the Tiny suggestion takes.
pub const TINY_PARAMS_B: f64 = 3.0;

#[derive(Debug, Deserialize)]
pub struct Build {
    pub id: String,
    pub format: Format,
    pub quant: String,
    pub repo: String,
    /// The Hugging Face commit the files are pinned to; `None` when the engine fetches them.
    pub revision: Option<String>,
    pub files: Vec<File>,
    pub size: u64,
    pub engines: Vec<String>,
    pub note: Option<String>,
}

impl Build {
    /// `GGUF`, `MLX`.
    /// The build's page on Hugging Face.
    pub fn page(&self) -> String {
        format!("https://huggingface.co/{}", self.repo)
    }

    pub fn format_label(&self) -> &'static str {
        match self.format {
            Format::Gguf => "GGUF",
            Format::Mlx => "MLX",
            Format::Gturbo => "TurboFieldfare",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct File {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub url: String,
}

/// A measured decode speed, which wins over an estimate on similar hardware.
#[derive(Debug, Deserialize)]
pub struct Speed {
    pub engine: String,
    pub hardware: String,
    /// `apple`, `nvidia` or `cpu`.
    pub chip: String,
    pub ram_gb: Option<f64>,
    pub tps: f64,
    pub source: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_to_the_pages_of_engines_and_models() {
        let catalog = catalog();
        let page = |id: &str| catalog.engine(id).unwrap().home_page().unwrap();
        assert_eq!(page("llama-cpp"), "https://github.com/ggml-org/llama.cpp");
        assert_eq!(
            page("ik-llama-cpp"),
            "https://github.com/ikawrakow/ik_llama.cpp"
        );
        assert_eq!(
            page("rapid-mlx"),
            "https://github.com/raullenchai/Rapid-MLX"
        );
        let build = &catalog.models[0].builds[0];
        assert_eq!(
            build.page(),
            format!("https://huggingface.co/{}", build.repo)
        );
    }

    fn hex64(text: &str) -> bool {
        text.len() == 64
            && text
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    }

    #[test]
    fn parses_and_everything_is_pinned() {
        let catalog = catalog();
        assert!(!catalog.models.is_empty() && !catalog.engines.is_empty());
        for engine in &catalog.engines {
            match &engine.install {
                Install::Archive { assets, .. } => {
                    assert!(!assets.is_empty(), "{}", engine.id);
                    for asset in assets.values() {
                        assert!(hex64(&asset.sha256), "{} {}", engine.id, asset.name);
                        assert!(asset.size > 0 && asset.url.starts_with("https://github.com/"));
                    }
                }
                Install::Build { source, .. } => {
                    assert!(hex64(&source.tar_sha256), "{}", engine.id);
                    assert!(source.url.starts_with("https://codeload.github.com/"));
                    assert!(source.url.rsplit('/').next().is_some_and(|c| c.len() == 40));
                }
                Install::Python { requirements, .. } => {
                    assert!(requirements.contains("--hash=sha256:"), "{}", engine.id);
                }
            }
        }
        for model in &catalog.models {
            assert!(!model.builds.is_empty(), "{}", model.id);
            for build in &model.builds {
                for id in &build.engines {
                    assert!(catalog.engine(id).is_some(), "{} names {id}", model.id);
                }
                if build.format == Format::Gturbo {
                    continue;
                }
                let revision = build.revision.as_deref().unwrap_or_default();
                assert_eq!(revision.len(), 40, "{}/{}", model.id, build.id);
                assert!(!build.files.is_empty());
                assert_eq!(build.files.iter().map(|f| f.size).sum::<u64>(), build.size);
                for file in &build.files {
                    assert!(hex64(&file.sha256), "{}/{}", model.id, file.name);
                    assert!(
                        file.url.starts_with("https://huggingface.co/")
                            && file.url.contains(&format!("/resolve/{revision}/")),
                        "{}",
                        file.url
                    );
                }
            }
        }
    }

    #[test]
    fn ids_are_unique() {
        let catalog = catalog();
        let mut ids: Vec<_> = catalog.models.iter().map(|m| &m.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), catalog.models.len());
    }
}
