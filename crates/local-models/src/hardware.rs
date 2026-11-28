//! What the computer has: memory, chip, graphics card, free disk and build tools.
//!
//! Read with the system's own tools (`sysctl`, `/proc`, `nvidia-smi`, `df`), each with a short
//! time limit, and on Windows from its own APIs (no PowerShell: it was slow to start, often past
//! the time limit, and opened windows); whatever cannot be read is left out, never guessed as
//! more.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use platform::NoWindow as _;

const GB: f64 = 1e9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Macos,
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppleTier {
    Base,
    Pro,
    Max,
    Ultra,
}

/// An Apple chip, like the M3 Pro.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppleChip {
    pub generation: u32,
    pub tier: AppleTier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
}

/// A graphics card with memory of its own.
#[derive(Debug, Clone, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor: Vendor,
    pub vram: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hardware {
    pub os: Os,
    pub arm64: bool,
    /// Like `(26, 4)`.
    pub os_version: Option<(u32, u32)>,
    pub cpu: String,
    pub apple: Option<AppleChip>,
    /// Bytes of memory.
    pub ram: u64,
    pub gpu: Option<Gpu>,
    pub avx512: bool,
    /// Bytes free where models are kept.
    pub free_disk: Option<u64>,
    /// The build tools found on the `PATH` (`cmake`, `c++`, `swift`).
    pub tools: Vec<String>,
}

/// One thing [`Hardware::facts`] tells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fact {
    Chip,
    Memory,
    Graphics,
    /// How much memory a model may take here.
    ForModels,
    Disk,
    System,
    /// Build tools, which engines built from source need.
    Tools,
}

impl Fact {
    pub fn label(self) -> &'static str {
        match self {
            Self::Chip => "Processor",
            Self::Memory => "Memory",
            Self::Graphics => "Graphics",
            Self::ForModels => "For models",
            Self::Disk => "Disk",
            Self::System => "System",
            Self::Tools => "Build tools",
        }
    }
}

/// Where the model's weights live while it runs, which decides the speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accel {
    /// Apple's unified memory.
    Metal,
    Cuda,
    Vulkan,
    Cpu,
}

impl Accel {
    pub fn key(self) -> &'static str {
        match self {
            Self::Metal => "metal",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
            Self::Cpu => "cpu",
        }
    }
}

impl Hardware {
    /// The platforms engines are built for that fit this computer, best first, like
    /// `["linux-x64-cuda", "linux-x64-vulkan", "linux-x64-cpu"]`.
    pub fn platforms(&self) -> Vec<(String, Accel)> {
        let os = match self.os {
            Os::Macos => "macos",
            Os::Linux => "linux",
            Os::Windows => "windows",
        };
        let arch = if self.arm64 { "arm64" } else { "x64" };
        let accels: &[Accel] = match (self.os, &self.gpu) {
            (Os::Macos, _) if self.arm64 => &[Accel::Metal],
            (_, Some(gpu)) if gpu.vendor == Vendor::Nvidia => {
                &[Accel::Cuda, Accel::Vulkan, Accel::Cpu]
            }
            (_, Some(_)) => &[Accel::Vulkan, Accel::Cpu],
            _ => &[Accel::Cpu],
        };
        accels
            .iter()
            .map(|accel| (format!("{os}-{arch}-{}", accel.key()), *accel))
            .collect()
    }

    /// Bytes a model may take while running with `accel`, leaving room for the system and apps.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn budget(&self, accel: Accel) -> u64 {
        let share = |bytes: u64, share: f64| (bytes as f64 * share) as u64;
        match (accel, &self.gpu) {
            // macOS lets the graphics side use about three quarters of the memory.
            (Accel::Metal, _) => share(self.ram, 0.70),
            (Accel::Cuda | Accel::Vulkan, Some(gpu)) => share(gpu.vram, 0.90),
            _ => share(self.ram, 0.60),
        }
    }

    /// Memory bandwidth in GB/s where the weights are read from with `accel`: what decoding speed
    /// mostly depends on. Typical values for the chip or card, else a conservative guess.
    pub fn bandwidth(&self, accel: Accel) -> f64 {
        match (accel, &self.gpu, self.apple) {
            (Accel::Metal, _, Some(chip)) => apple_bandwidth(chip),
            (Accel::Cuda | Accel::Vulkan, Some(gpu), _) => gpu_bandwidth(gpu),
            // Two channels of DDR5 in practice; Apple chips without Metal do not happen.
            _ => 70.,
        }
    }

    /// `M3 Pro · 18 GB memory · macOS 27.0`.
    #[allow(clippy::cast_precision_loss)]
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = match self.apple {
            Some(chip) => vec![chip.label()],
            None if self.cpu.is_empty() => vec![],
            None => vec![self.cpu.clone()],
        };
        let memory = if self.apple.is_some() {
            "unified memory"
        } else {
            "memory"
        };
        parts.push(format!("{:.0} GB {memory}", self.ram as f64 / GB));
        if let Some(gpu) = &self.gpu {
            parts.push(format!("{} ({:.0} GB)", gpu.name, gpu.vram as f64 / GB));
        }
        let os = match self.os {
            Os::Macos => "macOS",
            Os::Linux => "Linux",
            Os::Windows => "Windows",
        };
        parts.push(match self.os_version {
            Some((major, minor)) if self.os == Os::Macos => format!("{os} {major}.{minor}"),
            _ => os.to_owned(),
        });
        parts.join(" · ")
    }

    /// What was found, one fact a line, for people.
    #[allow(clippy::cast_precision_loss)]
    pub fn facts(&self) -> Vec<(Fact, String)> {
        let gb = |bytes: u64| format!("{:.0} GB", bytes as f64 / GB);
        let chip = match self.apple {
            Some(chip) => chip.label(),
            None if self.cpu.is_empty() => "Unknown".to_owned(),
            None => self.cpu.clone(),
        };
        let memory = if self.apple.is_some() {
            format!("{} unified memory", gb(self.ram))
        } else {
            gb(self.ram)
        };
        let graphics = match (self.apple, &self.gpu) {
            (Some(chip), _) if self.os == Os::Macos => format!(
                "{} GPU with Metal · ~{:.0} GB/s",
                chip.label(),
                apple_bandwidth(chip)
            ),
            (_, Some(gpu)) => format!(
                "{} · {} · ~{:.0} GB/s",
                gpu.name,
                gb(gpu.vram),
                gpu_bandwidth(gpu)
            ),
            _ => "None that models can use: they run on the processor".to_owned(),
        };
        let for_models = self.platforms().first().map_or_else(
            || "Unknown".to_owned(),
            |(_, accel)| format!("Up to about {} for a model", gb(self.budget(*accel))),
        );
        let os = match self.os {
            Os::Macos => "macOS",
            Os::Linux => "Linux",
            Os::Windows => "Windows",
        };
        let os = match self.os_version {
            Some((major, minor)) if self.os == Os::Macos => format!("{os} {major}.{minor}"),
            _ => os.to_owned(),
        };
        let arch = if self.arm64 { "arm64" } else { "x86-64" };
        let tools = if self.tools.is_empty() {
            "None found".to_owned()
        } else {
            self.tools.join(", ")
        };
        vec![
            (Fact::Chip, chip),
            (Fact::Memory, memory),
            (Fact::Graphics, graphics),
            (Fact::ForModels, for_models),
            (
                Fact::Disk,
                self.free_disk
                    .map_or_else(|| "Unknown".to_owned(), |free| format!("{} free", gb(free))),
            ),
            (Fact::System, format!("{os} · {arch}")),
            (Fact::Tools, tools),
        ]
    }

    /// Whether this macOS is at least `version` (`"26.4"`).
    pub fn macos_at_least(&self, version: &str) -> bool {
        let wanted = parse_version(version);
        self.os == Os::Macos
            && matches!((self.os_version, wanted), (Some(have), Some(want)) if have >= want)
    }

    pub fn has_tool(&self, tool: &str) -> bool {
        self.tools.iter().any(|have| have == tool)
    }
}

impl AppleChip {
    pub fn label(self) -> String {
        let tier = match self.tier {
            AppleTier::Base => "",
            AppleTier::Pro => " Pro",
            AppleTier::Max => " Max",
            AppleTier::Ultra => " Ultra",
        };
        format!("Apple M{}{tier}", self.generation)
    }
}

/// Typical memory bandwidth of an Apple chip in GB/s (the cut-down variants of a Max have less).
pub fn apple_bandwidth(chip: AppleChip) -> f64 {
    use AppleTier::{Base, Max, Pro, Ultra};
    match (chip.generation, chip.tier) {
        (1, Base) => 68.,
        (2 | 3, Base) => 100.,
        (4, Base) => 120.,
        (3, Pro) => 150.,
        (1 | 2, Pro) => 200.,
        (4, Pro) => 273.,
        (1..=3, Max) => 400.,
        (4, Max) => 546.,
        (_, Ultra) => 800.,
        // M5 and later: the M5's own figure, and the same growth for the larger chips.
        (_, Base) => 153.,
        (_, Pro) => 300.,
        (_, Max) => 600.,
    }
}

/// Typical memory bandwidth of a graphics card in GB/s, from its name, else from its memory.
#[allow(clippy::cast_precision_loss)]
pub fn gpu_bandwidth(gpu: &Gpu) -> f64 {
    let name = gpu.name.to_lowercase();
    let laptop = name.contains("laptop") || name.contains("mobile") || name.contains("max-q");
    let known: &[(&str, f64, f64)] = &[
        // (model, desktop, laptop)
        ("5090", 1792., 896.),
        ("5080", 960., 896.),
        ("5070 ti", 896., 448.),
        ("5070", 672., 384.),
        ("5060 ti", 448., 448.),
        ("5060", 448., 384.),
        ("4090", 1008., 576.),
        ("4080", 717., 432.),
        ("4070 ti", 504., 432.),
        ("4070", 504., 256.),
        ("4060 ti", 288., 288.),
        ("4060", 272., 256.),
        ("4050", 192., 192.),
        ("3090", 936., 936.),
        ("3080", 760., 448.),
        ("3070", 448., 448.),
        ("3060", 360., 336.),
    ];
    if let Some((_, desktop, mobile)) = known.iter().find(|(model, ..)| name.contains(model)) {
        return if laptop { *mobile } else { *desktop };
    }
    match gpu.vram as f64 / GB {
        v if v <= 8.5 => 256.,
        v if v <= 12.5 => 360.,
        v if v <= 16.5 => 448.,
        _ => 700.,
    }
}

/// Looks at this computer. `models_dir` is where models would go (for the free space), `path`
/// the `PATH` builds would run with (the login shell's). Blocks for up to a few seconds.
pub fn scan(models_dir: &Path, path: Option<&OsStr>) -> Hardware {
    let os = if cfg!(target_os = "macos") {
        Os::Macos
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else {
        Os::Linux
    };
    let arm64 = cfg!(target_arch = "aarch64");
    let mut hardware = Hardware {
        os,
        arm64,
        os_version: None,
        cpu: String::new(),
        apple: None,
        ram: 0,
        gpu: None,
        avx512: false,
        free_disk: None,
        tools: ["cmake", "c++", "swift"]
            .into_iter()
            .filter(|tool| on_path(tool, path))
            .map(str::to_owned)
            .collect(),
    };
    match os {
        Os::Macos => {
            hardware.cpu =
                output("sysctl", &["-n", "machdep.cpu.brand_string"]).unwrap_or_default();
            hardware.apple = parse_apple_chip(&hardware.cpu);
            hardware.ram = output("sysctl", &["-n", "hw.memsize"])
                .and_then(|text| text.parse().ok())
                .unwrap_or_default();
            hardware.os_version = output("sw_vers", &["-productVersion"])
                .as_deref()
                .and_then(parse_version);
        }
        Os::Linux => {
            let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
            hardware.cpu = parse_cpuinfo_name(&cpuinfo);
            hardware.avx512 = cpuinfo.contains(" avx512f");
            hardware.ram = std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| parse_meminfo(&text))
                .unwrap_or_default();
            hardware.gpu = nvidia_gpu();
        }
        Os::Windows => {
            #[cfg(windows)]
            {
                use platform::windows;
                hardware.cpu = windows::processor_name().unwrap_or_default();
                hardware.ram = windows::total_memory().unwrap_or_default();
                let adapters: Vec<_> = windows::graphics_adapters()
                    .into_iter()
                    .map(|a| (a.name, a.vendor_id, a.dedicated_memory))
                    .collect();
                // nvidia-smi knows an NVIDIA card's memory best; DirectX lists every card.
                hardware.gpu = nvidia_gpu().or_else(|| pick_adapter(&adapters));
            }
        }
    }
    #[cfg(target_arch = "x86_64")]
    {
        hardware.avx512 |= std::arch::is_x86_feature_detected!("avx512f");
    }
    hardware.free_disk = free_space(models_dir);
    hardware
}

/// Memory a graphics card needs of its own to be worth running models on.
const MIN_VRAM: u64 = 2_000_000_000;

/// The card to run models on from Windows' list of graphics adapters (name, PCI vendor, bytes
/// of memory of its own): the one with the most memory, leaving out graphics built into the
/// processor (which use the system's memory, like the processor itself).
pub fn pick_adapter(adapters: &[(String, u32, u64)]) -> Option<Gpu> {
    adapters
        .iter()
        .filter(|(name, _, vram)| *vram >= MIN_VRAM && !is_integrated(name))
        .filter_map(|(name, vendor, vram)| {
            let vendor = match vendor {
                0x10DE => Vendor::Nvidia,
                0x1002 | 0x1022 => Vendor::Amd,
                0x8086 => Vendor::Intel,
                _ => return None,
            };
            Some(Gpu {
                name: name.clone(),
                vendor,
                vram: *vram,
            })
        })
        .max_by_key(|gpu| gpu.vram)
}

/// Whether an adapter's name is that of graphics built into the processor ("AMD Radeon(TM)
/// Graphics", "AMD Radeon 780M Graphics", "Intel(R) UHD Graphics", "Intel(R) Arc(TM)
/// Graphics"), which may still report memory set aside for it.
pub fn is_integrated(name: &str) -> bool {
    let name = name.to_lowercase();
    let discrete_intel = name.contains("arc")
        && name.split_whitespace().any(|word| {
            let mut chars = word.chars();
            matches!(chars.next(), Some('a' | 'b'))
                && chars.as_str().len() == 3
                && chars.as_str().chars().all(|c| c.is_ascii_digit())
        });
    let discrete = name.contains("rx ")
        || name.contains("radeon pro")
        || name.contains("radeon vii")
        || name.contains("nvidia")
        || discrete_intel;
    !discrete && (name.contains("graphics") || name.contains("uhd") || name.contains("iris"))
}

fn nvidia_gpu() -> Option<Gpu> {
    output(
        "nvidia-smi",
        &[
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ],
    )
    .as_deref()
    .and_then(parse_nvidia_smi)
}

/// Free bytes on the disk `dir` is on (or would be on).
fn free_space(dir: &Path) -> Option<u64> {
    let existing = dir.ancestors().find(|path| path.exists())?;
    #[cfg(windows)]
    {
        platform::windows::free_disk(existing)
    }
    #[cfg(not(windows))]
    {
        let text = output("df", &["-k", &existing.to_string_lossy()])?;
        parse_df(&text)
    }
}

fn on_path(tool: &str, path: Option<&OsStr>) -> bool {
    let Some(path) = path
        .map(OsStr::to_owned)
        .or_else(|| std::env::var_os("PATH"))
    else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let file = dir.join(tool);
        file.is_file() || (cfg!(windows) && file.with_extension("exe").is_file())
    })
}

/// What `program` prints, if it runs within a few seconds and succeeds.
fn output(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .no_window()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) | Err(_) => return None,
            Ok(None) if std::time::Instant::now() > deadline => {
                let _ = child.kill();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let mut text = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut text).ok()?;
    Some(text.trim().to_owned())
}

/// `"Apple M3 Pro"` → M3 Pro.
pub fn parse_apple_chip(brand: &str) -> Option<AppleChip> {
    let rest = brand.trim().strip_prefix("Apple M")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let generation = digits.parse().ok()?;
    let tier = match rest[digits.len()..].trim() {
        "Pro" => AppleTier::Pro,
        "Max" => AppleTier::Max,
        "Ultra" => AppleTier::Ultra,
        _ => AppleTier::Base,
    };
    Some(AppleChip { generation, tier })
}

/// `"26.4.1"` → `(26, 4)`.
pub fn parse_version(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |minor| minor.parse().ok())?;
    Some((major, minor))
}

/// Total memory in bytes from `/proc/meminfo`.
pub fn parse_meminfo(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

pub fn parse_cpuinfo_name(text: &str) -> String {
    text.lines()
        .find_map(|line| line.strip_prefix("model name"))
        .and_then(|rest| rest.split_once(':'))
        .map_or_else(String::new, |(_, name)| name.trim().to_owned())
}

/// The first card of `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`.
pub fn parse_nvidia_smi(text: &str) -> Option<Gpu> {
    let (name, mib) = text.lines().next()?.rsplit_once(',')?;
    let mib: u64 = mib.trim().parse().ok()?;
    Some(Gpu {
        name: name.trim().to_owned(),
        vendor: Vendor::Nvidia,
        vram: mib * 1024 * 1024,
    })
}

/// Free bytes from `df -k`'s second line.
pub fn parse_df(text: &str) -> Option<u64> {
    let line = text.lines().nth(1)?;
    let kib: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(kib * 1024)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    #[allow(clippy::float_cmp)] // exact table values
    fn reads_apple_chips() {
        let chip = |generation, tier| Some(AppleChip { generation, tier });
        assert_eq!(parse_apple_chip("Apple M3 Pro"), chip(3, AppleTier::Pro));
        assert_eq!(parse_apple_chip("Apple M4"), chip(4, AppleTier::Base));
        assert_eq!(
            parse_apple_chip("Apple M2 Ultra"),
            chip(2, AppleTier::Ultra)
        );
        assert_eq!(parse_apple_chip("Intel(R) Core(TM) i7"), None);
        assert_eq!(
            apple_bandwidth(AppleChip {
                generation: 4,
                tier: AppleTier::Pro
            }),
            273.
        );
        assert_eq!(
            apple_bandwidth(AppleChip {
                generation: 9,
                tier: AppleTier::Base
            }),
            153.
        );
    }

    #[test]
    #[allow(clippy::float_cmp)] // exact table values
    fn reads_system_tools() {
        assert_eq!(parse_version("26.4.1"), Some((26, 4)));
        assert_eq!(parse_version("27"), Some((27, 0)));
        assert_eq!(
            parse_meminfo("MemTotal:       16303536 kB\nMemFree: 1 kB"),
            Some(16_694_820_864)
        );
        assert_eq!(
            parse_cpuinfo_name("processor\t: 0\nmodel name\t: AMD Ryzen 7 7840U\n"),
            "AMD Ryzen 7 7840U"
        );
        let gpu = parse_nvidia_smi("NVIDIA GeForce RTX 4060 Laptop GPU, 8188\n").unwrap();
        assert_eq!(gpu.vram, 8188 * 1024 * 1024);
        assert_eq!(gpu_bandwidth(&gpu), 256.);
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 971350180 500000000 400000000 56% /";
        assert_eq!(parse_df(df), Some(409_600_000_000));
    }

    #[test]
    fn picks_a_windows_graphics_card() {
        let gb = 1_000_000_000;
        let adapters = |list: &[(&str, u32, u64)]| -> Vec<(String, u32, u64)> {
            list.iter()
                .map(|(name, vendor, vram)| ((*name).to_owned(), *vendor, *vram))
                .collect()
        };
        let laptop = adapters(&[
            ("AMD Radeon(TM) Graphics", 0x1002, 2 * gb),
            ("NVIDIA GeForce RTX 4060 Laptop GPU", 0x10DE, 8 * gb),
        ]);
        let gpu = pick_adapter(&laptop).unwrap();
        assert_eq!((gpu.vendor, gpu.vram), (Vendor::Nvidia, 8 * gb));
        let desktop = adapters(&[("AMD Radeon RX 7800 XT", 0x1002, 16 * gb)]);
        assert_eq!(pick_adapter(&desktop).unwrap().vendor, Vendor::Amd);
        let arc = adapters(&[("Intel(R) Arc(TM) B580 Graphics", 0x8086, 12 * gb)]);
        assert_eq!(pick_adapter(&arc).unwrap().vendor, Vendor::Intel);
        // Graphics in the processor use the system's memory: the processor runs the model.
        let thin = adapters(&[
            ("AMD Radeon 780M Graphics", 0x1002, 4 * gb),
            ("Intel(R) UHD Graphics 620", 0x8086, gb / 8),
            ("Intel(R) Arc(TM) Graphics", 0x8086, 2 * gb),
            ("Microsoft Basic Render Driver", 0x1414, 0),
        ]);
        assert_eq!(pick_adapter(&thin), None);
    }

    #[test]
    fn picks_platforms_and_budgets() {
        let mac = Hardware {
            os: Os::Macos,
            arm64: true,
            os_version: Some((26, 4)),
            cpu: "Apple M3 Pro".into(),
            apple: parse_apple_chip("Apple M3 Pro"),
            ram: 18_000_000_000,
            gpu: None,
            avx512: false,
            free_disk: None,
            tools: vec![],
        };
        assert_eq!(
            mac.platforms(),
            vec![("macos-arm64-metal".to_owned(), Accel::Metal)]
        );
        assert_eq!(mac.budget(Accel::Metal), 12_600_000_000);
        assert!(mac.macos_at_least("26.4") && !mac.macos_at_least("26.5"));
        assert_eq!(
            mac.summary(),
            "Apple M3 Pro · 18 GB unified memory · macOS 26.4"
        );
        let facts = Hardware {
            free_disk: Some(312_000_000_000),
            tools: vec!["cmake".into(), "swift".into()],
            ..mac.clone()
        }
        .facts();
        assert_eq!(
            facts,
            [
                (Fact::Chip, "Apple M3 Pro".to_owned()),
                (Fact::Memory, "18 GB unified memory".to_owned()),
                (
                    Fact::Graphics,
                    "Apple M3 Pro GPU with Metal · ~150 GB/s".to_owned()
                ),
                (Fact::ForModels, "Up to about 13 GB for a model".to_owned()),
                (Fact::Disk, "312 GB free".to_owned()),
                (Fact::System, "macOS 26.4 · arm64".to_owned()),
                (Fact::Tools, "cmake, swift".to_owned()),
            ]
        );
        let pc = Hardware {
            os: Os::Linux,
            arm64: false,
            gpu: parse_nvidia_smi("NVIDIA GeForce RTX 4060 Laptop GPU, 8188"),
            apple: None,
            ..mac
        };
        let keys: Vec<_> = pc.platforms().into_iter().map(|(key, _)| key).collect();
        assert_eq!(
            keys,
            ["linux-x64-cuda", "linux-x64-vulkan", "linux-x64-cpu"]
        );
    }
}
