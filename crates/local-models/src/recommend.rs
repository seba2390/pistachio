//! Four suggestions for a computer: tiny, fast, balanced and most capable.
//!
//! Every way to run a catalog model here is weighed ([`choices`]): an engine that runs on this
//! platform and loads the build, the memory it takes, and the decoding speed, estimated from the
//! memory bandwidth (decoding reads every active weight once per token) or, for engines that stream
//! weights from the disk, measured. Ways under 5 tokens a second are left out.
//!
//! Each tier then takes the most capable model (the catalog's curated rank) that reaches its speed,
//! a different model per tier. Tiny goes by size instead: the most capable model of at most
//! [`TINY_PARAMS_B`](crate::catalog::TINY_PARAMS_B) billion parameters, which nearly any computer
//! runs, picked first so that it is never taken by a faster tier. For that model the default is the best engine that installs
//! unattended; a specialized engine is offered next to it when it is clearly faster.

use crate::catalog::{Build, Catalog, Effort, Engine, Format, Install, Model, Speed};
use crate::hardware::{Accel, Hardware};

const GB: f64 = 1e9;
/// Slower than this is not worth suggesting (more than 5 tokens a second, with a margin).
pub const MIN_TPS: f64 = 6.;
/// Context the engine is started with (or the model's own, when smaller): enough for an agent's
/// instructions and tools, a conversation and files.
pub const CONTEXT: u64 = 32_768;
/// A specialized engine is offered only when it is at least this much faster.
const SPECIALIZED_GAIN: f64 = 1.15;
/// A faster tier's model is at least this much faster than the slower tier's.
const FASTER: f64 = 1.3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// A small model (see [`Model::is_tiny`]), whatever the computer.
    Tiny,
    Fast,
    Balanced,
    Capable,
}

impl Tier {
    pub const ALL: [Self; 4] = [Self::Tiny, Self::Fast, Self::Balanced, Self::Capable];

    pub fn label(self) -> &'static str {
        match self {
            Self::Tiny => "Tiny",
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::Capable => "Most capable",
        }
    }

    /// Tokens a second it must reach.
    pub fn min_tps(self) -> f64 {
        match self {
            Self::Tiny => MIN_TPS,
            Self::Fast => 30.,
            Self::Balanced => 15.,
            Self::Capable => MIN_TPS,
        }
    }
}

/// One way to run a model here.
#[derive(Debug, Clone)]
pub struct Choice {
    pub model: &'static Model,
    pub build: &'static Build,
    pub engine: &'static Engine,
    /// The engine's platform key, like `macos-arm64-metal`.
    pub platform: String,
    pub accel: Accel,
    /// Decoding speed in tokens a second.
    pub tps: f64,
    /// Where `tps` comes from when it was measured rather than estimated.
    pub measured: Option<&'static Speed>,
    /// Bytes of memory it takes while running.
    pub memory: u64,
    /// The weights do not fit in memory; the engine reads them from the disk as needed.
    pub streams: bool,
    pub context: u64,
    /// Bytes downloaded: the engine (when not installed) and the model.
    pub download: u64,
    /// Build tools it needs that are not on this computer.
    pub missing_tools: Vec<String>,
}

impl Choice {
    /// Stable id of the setup: `<model>/<build>/<engine>`.
    pub fn key(&self) -> String {
        format!("{}/{}/{}", self.model.id, self.build.id, self.engine.id)
    }

    pub fn is_easy(&self) -> bool {
        self.engine.effort == Effort::Easy
    }
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub tier: Tier,
    pub model: &'static Model,
    /// The best engine that installs unattended; `None` when only a specialized one runs it.
    pub default: Option<Choice>,
    /// A faster engine that takes more steps (a local build).
    pub specialized: Option<Choice>,
}

/// Bytes an engine itself takes to download.
pub fn engine_download(engine: &Engine, platform: &str) -> u64 {
    match &engine.install {
        Install::Archive { assets, .. } => assets.get(platform).map_or(0, |asset| asset.size),
        Install::Build { source, .. } => source.size,
        Install::Python { packages_size, .. } => {
            let python = python_env::catalog::platform();
            packages_size + python.map_or(0, |p| p.uv_size + p.python_size)
        }
    }
}

/// Every way to run a catalog model on `hardware` at [`MIN_TPS`] or more, fastest first.
pub fn choices(hardware: &Hardware, catalog: &'static Catalog) -> Vec<Choice> {
    let mut all = Vec::new();
    for model in &catalog.models {
        for build in &model.builds {
            for engine in build.engines.iter().filter_map(|id| catalog.engine(id)) {
                if let Some(choice) = weigh(hardware, model, build, engine) {
                    all.push(choice);
                }
            }
        }
    }
    all.sort_by(|a, b| b.tps.total_cmp(&a.tps));
    all
}

#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn weigh(
    hardware: &Hardware,
    model: &'static Model,
    build: &'static Build,
    engine: &'static Engine,
) -> Option<Choice> {
    if !engine.formats.contains(&build.format) || !engine.runs(model) || !allowed(hardware, engine)
    {
        return None;
    }
    let size = build.size as f64 / GB;
    let context = model.context.min(CONTEXT);
    // The context's cache, and the engine's own (a Python one takes more).
    let overhead = if model.total_params_b > 10. { 2.0 } else { 1.0 }
        + if matches!(engine.install, Install::Python { .. }) {
            0.8
        } else {
            0.3
        };
    let platforms = hardware.platforms();
    for (platform, accel) in platforms
        .iter()
        .filter(|(key, _)| engine.platforms.contains(key))
    {
        let Some(efficiency) = engine.efficiency.get(accel.key()) else {
            continue;
        };
        let budget = hardware.budget(*accel) as f64 / GB;
        // TurboFieldfare keeps a small core in memory and streams the rest by design.
        let resident = if build.format == Format::Gturbo {
            3.0
        } else {
            size + overhead
        };
        let fits = resident <= budget;
        let streams = !fits && engine.streams_experts && model.is_moe();
        if !fits && !streams {
            // Perhaps on the next platform (the CPU, with the system memory).
            continue;
        }
        // When weights come from the disk the bandwidth says little: only measured speeds count
        // there. Elsewhere a measurement on other hardware would mislead, so it is estimated.
        let from_disk = streams || build.format == Format::Gturbo;
        let measured = from_disk
            .then(|| measured(hardware, model, engine, *accel))
            .flatten();
        let tps = match (measured, from_disk) {
            (Some(speed), _) => speed.tps,
            (None, true) => continue,
            (None, false) => {
                // A mixture of experts reads more than its active weights: attention, shared
                // experts and the router (fitted to llama.cpp and Splash measurements).
                let share = model.active_params_b / model.total_params_b;
                let per_token = size
                    * if model.is_moe() {
                        (share * 1.4 + 0.02).min(1.)
                    } else {
                        share
                    };
                hardware.bandwidth(*accel) * efficiency / per_token.max(0.1)
            }
        };
        if tps < MIN_TPS {
            return None;
        }
        let download = build.size + engine_download(engine, platform);
        if hardware
            .free_disk
            .is_some_and(|free| free < download + download / 10)
        {
            return None;
        }
        return Some(Choice {
            model,
            build,
            engine,
            platform: platform.clone(),
            accel: *accel,
            tps,
            measured,
            memory: ((if streams { budget.min(size) } else { resident }) * GB) as u64,
            streams,
            context,
            download,
            missing_tools: engine
                .requires
                .tools
                .iter()
                .filter(|tool| !hardware.has_tool(tool))
                .cloned()
                .collect(),
        });
    }
    None
}

/// Whether the engine can run here at all (system version, chip, CPU features).
fn allowed(hardware: &Hardware, engine: &Engine) -> bool {
    let requires = &engine.requires;
    requires
        .min_macos
        .as_deref()
        .is_none_or(|version| hardware.macos_at_least(version))
        && requires
            .apple_chip_min
            .is_none_or(|min| hardware.apple.is_some_and(|chip| chip.generation >= min))
        && (!requires.avx512 || hardware.avx512)
}

/// A measured speed for this engine on similar hardware: the same kind of chip, and the most
/// memory that is not more than this computer has.
#[allow(clippy::cast_precision_loss)]
fn measured(
    hardware: &Hardware,
    model: &'static Model,
    engine: &Engine,
    accel: Accel,
) -> Option<&'static Speed> {
    let chip = match accel {
        Accel::Metal => "apple",
        Accel::Cuda | Accel::Vulkan => "nvidia",
        Accel::Cpu => "cpu",
    };
    let ram = hardware.ram as f64 / GB;
    model
        .speeds
        .iter()
        .filter(|speed| speed.engine == engine.id && speed.chip == chip)
        .filter(|speed| speed.ram_gb.is_some_and(|gb| gb <= ram * 1.1))
        .max_by(|a, b| a.ram_gb.unwrap_or(0.).total_cmp(&b.ram_gb.unwrap_or(0.)))
}

/// Up to four suggestions in [`Tier::ALL`]'s order (Tiny, then the fastest first); a tier no
/// model reaches is left out.
pub fn suggest(hardware: &Hardware, catalog: &'static Catalog) -> Vec<Suggestion> {
    let all = choices(hardware, catalog);
    let mut taken: Vec<&str> = Vec::new();
    // Tiny first: the most capable small model that runs here.
    let tiny = catalog
        .models
        .iter()
        .filter(|model| model.is_tiny())
        .filter_map(|model| {
            let ways: Vec<&Choice> = all
                .iter()
                .filter(|c| c.model.id == model.id && c.tps >= Tier::Tiny.min_tps())
                .collect();
            let shown = pick_for(Tier::Tiny, &ways, Effort::Easy)
                .or_else(|| pick_for(Tier::Tiny, &ways, Effort::Involved))?;
            Some((model, shown.tps))
        })
        .max_by(|(a, a_tps), (b, b_tps)| a.quality.cmp(&b.quality).then(a_tps.total_cmp(b_tps)))
        .map(|(model, _)| {
            taken.push(&model.id);
            suggestion(Tier::Tiny, model, &all, Tier::Tiny.min_tps())
        });
    let mut picks = Vec::new();
    // The most capable model first; each faster tier then takes the most capable model that is
    // clearly faster than the tier before. So speed rises and capability falls from tier to tier.
    let mut slowest: Option<f64> = None;
    for tier in [Tier::Capable, Tier::Balanced, Tier::Fast] {
        let floor = slowest.map_or(0., |tps| tps * FASTER).max(tier.min_tps());
        let ways = |model: &Model| -> Vec<&Choice> {
            all.iter()
                .filter(|c| c.model.id == model.id && c.tps >= floor)
                .collect()
        };
        let best = catalog
            .models
            .iter()
            .filter(|model| !taken.contains(&model.id.as_str()))
            .filter_map(|model| {
                let ways = ways(model);
                let shown = pick(&ways, Effort::Easy).or_else(|| pick(&ways, Effort::Involved))?;
                Some((model, shown.tps))
            })
            .max_by(|(a, a_tps), (b, b_tps)| {
                a.quality.cmp(&b.quality).then(a_tps.total_cmp(b_tps))
            });
        let Some((model, tps)) = best else {
            continue;
        };
        slowest = Some(tps);
        taken.push(&model.id);
        picks.push(suggestion(tier, model, &all, floor));
    }
    picks.extend(tiny);
    picks.reverse();
    picks
}

/// `model` for `tier`: the ways that reach `floor` tokens a second.
fn suggestion(tier: Tier, model: &'static Model, all: &[Choice], floor: f64) -> Suggestion {
    let fits: Vec<&Choice> = all
        .iter()
        .filter(|c| c.model.id == model.id && c.tps >= floor)
        .collect();
    // A default is always offered when an unattended engine runs the model at all, even
    // when only the specialized one reaches the tier's speed.
    let any: Vec<&Choice> = all.iter().filter(|c| c.model.id == model.id).collect();
    let default =
        pick_for(tier, &fits, Effort::Easy).or_else(|| pick_for(tier, &any, Effort::Easy));
    let specialized = pick_for(tier, &fits, Effort::Involved).filter(|special| {
        default
            .as_ref()
            .is_none_or(|default| special.tps >= default.tps * SPECIALIZED_GAIN)
    });
    Suggestion {
        tier,
        model,
        default,
        specialized,
    }
}

/// The best way with engines of `effort`: the most precise build (the largest), the faster
/// engine on a tie.
fn pick(ways: &[&Choice], effort: Effort) -> Option<Choice> {
    ways.iter()
        .filter(|c| c.engine.effort == effort)
        .max_by(|a, b| {
            a.build
                .size
                .cmp(&b.build.size)
                .then(a.tps.total_cmp(&b.tps))
        })
        .map(|c| (*c).clone())
}

/// For [`Tier::Tiny`], whose point is to be light: the smallest build, the faster engine on a
/// tie. Other tiers [`pick`].
fn pick_for(tier: Tier, ways: &[&Choice], effort: Effort) -> Option<Choice> {
    if tier != Tier::Tiny {
        return pick(ways, effort);
    }
    ways.iter()
        .filter(|c| c.engine.effort == effort)
        .max_by(|a, b| {
            b.build
                .size
                .cmp(&a.build.size)
                .then(a.tps.total_cmp(&b.tps))
        })
        .map(|c| (*c).clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;
    use crate::hardware::{AppleChip, AppleTier, Gpu, Os, Vendor};

    fn mac(generation: u32, tier: AppleTier, gb: u64, version: (u32, u32)) -> Hardware {
        Hardware {
            os: Os::Macos,
            arm64: true,
            os_version: Some(version),
            cpu: String::new(),
            apple: Some(AppleChip { generation, tier }),
            ram: gb * 1_000_000_000,
            gpu: None,
            avx512: false,
            free_disk: Some(500_000_000_000),
            tools: vec![],
        }
    }

    fn pc(gpu: Option<Gpu>, gb: u64) -> Hardware {
        Hardware {
            os: Os::Linux,
            arm64: false,
            os_version: None,
            cpu: String::new(),
            apple: None,
            ram: gb * 1_000_000_000,
            gpu,
            avx512: false,
            free_disk: Some(500_000_000_000),
            tools: vec!["cmake".into(), "c++".into()],
        }
    }

    fn check(suggestions: &[Suggestion]) {
        let mut models: Vec<_> = suggestions.iter().map(|s| &s.model.id).collect();
        models.dedup();
        assert_eq!(models.len(), suggestions.len(), "one model per tier");
        for s in suggestions {
            let some = s.default.iter().chain(&s.specialized);
            for choice in some.clone() {
                assert!(choice.tps >= MIN_TPS, "{} {}", s.model.id, choice.tps);
                assert_eq!(choice.model.id, s.model.id);
            }
            assert!(
                some.clone().any(|c| c.tps >= s.tier.min_tps()),
                "{}",
                s.model.id
            );
            assert!(s.default.is_some() || s.specialized.is_some());
            if let Some(default) = &s.default {
                assert_eq!(default.engine.effort, Effort::Easy);
            }
        }
    }

    #[test]
    fn suggests_four_on_a_mid_range_mac() {
        let suggestions = suggest(&mac(3, AppleTier::Pro, 18, (26, 0)), catalog());
        check(&suggestions);
        assert_eq!(suggestions.len(), 4);
        let tiers: Vec<_> = suggestions.iter().map(|s| s.tier).collect();
        assert_eq!(tiers, Tier::ALL);
        // On a Mac, the default for MLX builds is not llama.cpp.
        assert!(
            suggestions
                .iter()
                .filter_map(|s| s.default.as_ref())
                .any(|c| c.engine.id != "llama-cpp")
        );
    }

    #[test]
    fn fits_what_a_small_mac_has() {
        let hardware = mac(2, AppleTier::Base, 8, (15, 0));
        let suggestions = suggest(&hardware, catalog());
        check(&suggestions);
        for choice in suggestions.iter().flat_map(|s| s.default.iter()) {
            assert!(choice.memory <= hardware.budget(Accel::Metal) || choice.streams);
        }
        // Splash needs macOS 26.4 and an M3 or later.
        assert!(
            choices(&hardware, catalog())
                .iter()
                .all(|c| c.engine.id != "splash")
        );
    }

    #[test]
    fn uses_the_graphics_card_or_the_processor() {
        let laptop = pc(
            Some(Gpu {
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                vendor: Vendor::Nvidia,
                vram: 8_000_000_000,
            }),
            32,
        );
        let suggestions = suggest(&laptop, catalog());
        check(&suggestions);
        assert!(!suggestions.is_empty());
        for choice in suggestions.iter().flat_map(|s| s.default.iter()) {
            assert!(
                choice.platform.starts_with("linux-x64-"),
                "{}",
                choice.platform
            );
            assert_ne!(choice.engine.id, "rapid-mlx");
        }
        let cpu_only = suggest(&pc(None, 16), catalog());
        check(&cpu_only);
        assert!(
            !cpu_only.is_empty(),
            "a 16 GB laptop without a card runs something"
        );
    }

    #[allow(clippy::cast_precision_loss)]
    fn print(hardware: &Hardware) {
        println!("\n{}", hardware.summary());
        for s in suggest(hardware, catalog()) {
            println!("{}: {}", s.tier.label(), s.model.name);
            for (role, choice) in [("default", &s.default), ("specialized", &s.specialized)] {
                if let Some(c) = choice {
                    println!(
                        "  {role}: {} {} via {} ~{:.0} tok/s{}, {:.1} GB memory, {:.1} GB download",
                        c.build.quant,
                        c.build.format_label(),
                        c.engine.name,
                        c.tps,
                        if c.measured.is_some() {
                            " (measured)"
                        } else {
                            ""
                        },
                        c.memory as f64 / 1e9,
                        c.download as f64 / 1e9
                    );
                }
            }
        }
    }

    /// `cargo test -p local-models this_computer -- --ignored --nocapture` prints what this
    /// computer would be offered.
    #[test]
    #[ignore = "depends on the computer"]
    fn this_computer() {
        print(&crate::hardware::scan(&std::env::temp_dir(), None));
    }

    /// `cargo test -p local-models typical_computers -- --ignored --nocapture`: what typical
    /// computers would be offered, to look over after a catalog update.
    #[test]
    #[ignore = "prints for a person to read"]
    fn typical_computers() {
        print(&mac(2, AppleTier::Base, 8, (15, 0)));
        print(&mac(4, AppleTier::Base, 16, (26, 4)));
        print(&mac(4, AppleTier::Pro, 36, (26, 4)));
        print(&pc(
            Some(Gpu {
                name: "NVIDIA GeForce RTX 4060 Laptop GPU".into(),
                vendor: Vendor::Nvidia,
                vram: 8_000_000_000,
            }),
            32,
        ));
        print(&pc(None, 16));
    }

    #[test]
    fn tiny_is_a_small_model_on_any_computer() {
        let computers = [
            mac(2, AppleTier::Base, 8, (15, 0)),
            mac(4, AppleTier::Max, 64, (26, 4)),
            pc(None, 8),
            pc(None, 16),
        ];
        for hardware in &computers {
            let suggestions = suggest(hardware, catalog());
            check(&suggestions);
            let first = suggestions.first().expect("something runs");
            assert_eq!(first.tier, Tier::Tiny, "{}", hardware.summary());
            assert!(first.model.is_tiny(), "{}", first.model.id);
            // The best small model that runs, so the same on every computer here.
            let best_tiny = catalog()
                .models
                .iter()
                .filter(|model| model.is_tiny())
                .max_by_key(|model| model.quality)
                .unwrap();
            assert_eq!(first.model.id, best_tiny.id);
            assert!(suggestions[1..].iter().all(|s| s.tier != Tier::Tiny));
            // Light: the smallest build of it that runs here.
            let default = first.default.as_ref().unwrap();
            let smallest = choices(hardware, catalog())
                .iter()
                .filter(|c| c.model.id == first.model.id && c.is_easy())
                .map(|c| c.build.size)
                .min()
                .unwrap();
            assert_eq!(default.build.size, smallest, "{}", default.build.id);
        }
    }

    #[test]
    fn nothing_on_too_little_memory() {
        assert!(suggest(&pc(None, 2), catalog()).is_empty());
    }
}
