//! Everything the UI draws is compiled into the binary: no runtime asset
//! loading, no network fetches, nothing on disk to tamper with.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/", $name)) as &[u8])),*]
    };
}

static FILES: &[(&str, &[u8])] = embed![
    "icons/activity.svg",
    "icons/alert.svg",
    "icons/arrow-left.svg",
    "icons/check.svg",
    "icons/chevron-right.svg",
    "icons/clipboard.svg",
    "icons/coin.svg",
    "icons/copy.svg",
    "icons/cube.svg",
    "icons/eye-off.svg",
    "icons/eye.svg",
    "icons/fingerprint.svg",
    "icons/grid.svg",
    "icons/id.svg",
    "icons/import.svg",
    "icons/info.svg",
    "icons/key.svg",
    "icons/leaf.svg",
    "icons/lock.svg",
    "icons/plus.svg",
    "icons/receive.svg",
    "icons/refresh.svg",
    "icons/send.svg",
    "icons/settings.svg",
    "icons/shield.svg",
    "icons/spinner.svg",
    "icons/trash.svg",
    "icons/wallet.svg",
    "icons/wifi.svg",
    "icons/x.svg",
];

pub static FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Inter-Regular.ttf"),
    include_bytes!("../assets/fonts/Inter-Medium.ttf"),
    include_bytes!("../assets/fonts/Inter-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Inter-Bold.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf"),
    include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf"),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES.iter().find(|(p, _)| *p == path).map(|(_, b)| Cow::Borrowed(*b)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
            .collect())
    }
}

/// Icon names map to `icons/<name>.svg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Activity,
    Alert,
    ArrowLeft,
    Check,
    ChevronRight,
    Clipboard,
    Coin,
    Copy,
    Cube,
    EyeOff,
    Eye,
    Fingerprint,
    Grid,
    Id,
    Import,
    Info,
    Key,
    Leaf,
    Lock,
    Plus,
    Receive,
    Refresh,
    Send,
    Settings,
    Shield,
    Spinner,
    Trash,
    Wallet,
    Wifi,
    X,
}

impl Icon {
    pub fn path(self) -> &'static str {
        match self {
            Icon::Activity => "icons/activity.svg",
            Icon::Alert => "icons/alert.svg",
            Icon::ArrowLeft => "icons/arrow-left.svg",
            Icon::Check => "icons/check.svg",
            Icon::ChevronRight => "icons/chevron-right.svg",
            Icon::Clipboard => "icons/clipboard.svg",
            Icon::Coin => "icons/coin.svg",
            Icon::Copy => "icons/copy.svg",
            Icon::Cube => "icons/cube.svg",
            Icon::EyeOff => "icons/eye-off.svg",
            Icon::Eye => "icons/eye.svg",
            Icon::Fingerprint => "icons/fingerprint.svg",
            Icon::Grid => "icons/grid.svg",
            Icon::Id => "icons/id.svg",
            Icon::Import => "icons/import.svg",
            Icon::Info => "icons/info.svg",
            Icon::Key => "icons/key.svg",
            Icon::Leaf => "icons/leaf.svg",
            Icon::Lock => "icons/lock.svg",
            Icon::Plus => "icons/plus.svg",
            Icon::Receive => "icons/receive.svg",
            Icon::Refresh => "icons/refresh.svg",
            Icon::Send => "icons/send.svg",
            Icon::Settings => "icons/settings.svg",
            Icon::Shield => "icons/shield.svg",
            Icon::Spinner => "icons/spinner.svg",
            Icon::Trash => "icons/trash.svg",
            Icon::Wallet => "icons/wallet.svg",
            Icon::Wifi => "icons/wifi.svg",
            Icon::X => "icons/x.svg",
        }
    }
}
