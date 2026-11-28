//! Icons bundled by the app (gpui-kit ships a subset of Lucide).

use gpui_kit::component::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppIcon {
    Brain,
    CheckCheck,
    CodeXml,
    CornerDownRight,
    Download,
    ExternalLink,
    Files,
    Gauge,
    GitBranch,
    GitCompare,
    Globe,
    GlobeLock,
    GlobeOff,
    Gpu,
    HandHelping,
    HardDrive,
    Image,
    Key,
    Laptop,
    Library,
    Link,
    MemoryStick,
    MessageSquare,
    Package,
    Paperclip,
    Pencil,
    PiLogo,
    Play,
    Plug,
    RotateCcw,
    ScrollText,
    Server,
    ShieldCheck,
    ShieldOff,
    Sparkles,
    SquarePen,
    TextWrap,
    Trash,
    WifiOff,
    Wrench,
}

impl AppIcon {
    pub fn path(self) -> &'static str {
        match self {
            Self::Brain => "icons/brain.svg",
            Self::CheckCheck => "icons/check-check.svg",
            Self::CodeXml => "icons/code-xml.svg",
            Self::CornerDownRight => "icons/corner-down-right.svg",
            Self::Download => "icons/download.svg",
            Self::Files => "icons/files.svg",
            Self::GitBranch => "icons/git-branch.svg",
            Self::GitCompare => "icons/git-compare.svg",
            Self::Globe => "icons/globe.svg",
            Self::GlobeLock => "icons/globe-lock.svg",
            Self::GlobeOff => "icons/globe-off.svg",
            Self::HandHelping => "icons/hand-helping.svg",
            Self::Image => "icons/image.svg",
            Self::Key => "icons/key.svg",
            Self::Library => "icons/library.svg",
            Self::Link => "icons/link.svg",
            Self::MessageSquare => "icons/message-square.svg",
            Self::Package => "icons/package.svg",
            Self::Paperclip => "icons/paperclip.svg",
            Self::Pencil => "icons/pencil.svg",
            Self::PiLogo => "icons/pi-logo.svg",
            Self::Play => "icons/play.svg",
            Self::RotateCcw => "icons/rotate-ccw.svg",
            Self::Server => "icons/server.svg",
            Self::ShieldCheck => "icons/shield-check.svg",
            Self::ShieldOff => "icons/shield-off.svg",
            Self::Sparkles => "icons/sparkles.svg",
            Self::SquarePen => "icons/square-pen.svg",
            Self::TextWrap => "icons/text-wrap.svg",
            Self::Trash => "icons/trash.svg",
            Self::Gauge => "icons/gauge.svg",
            Self::Gpu => "icons/gpu.svg",
            Self::HardDrive => "icons/hard-drive.svg",
            Self::Laptop => "icons/laptop.svg",
            Self::MemoryStick => "icons/memory-stick.svg",
            Self::Plug => "icons/plug.svg",
            Self::ExternalLink => "icons/external-link.svg",
            Self::ScrollText => "icons/scroll-text.svg",
            Self::WifiOff => "icons/wifi-off.svg",
            Self::Wrench => "icons/wrench.svg",
        }
    }
}

impl From<AppIcon> for Icon {
    fn from(icon: AppIcon) -> Self {
        Icon::default().path(icon.path())
    }
}
