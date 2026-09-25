//! Colours and type. One dark theme, tuned for contrast on money-critical text.

use gpui::{Hsla, Rgba, rgb, rgba};

pub const SANS: &str = "Inter";
pub const MONO: &str = "JetBrains Mono";

pub fn bg() -> Rgba {
    rgb(0x0a0c0f)
}
pub fn sidebar() -> Rgba {
    rgb(0x0d1014)
}
pub fn surface() -> Rgba {
    rgb(0x12161b)
}
pub fn surface_hi() -> Rgba {
    rgb(0x181d24)
}
pub fn surface_hover() -> Rgba {
    rgb(0x1e242c)
}
pub fn border() -> Rgba {
    rgb(0x222932)
}
pub fn border_hi() -> Rgba {
    rgb(0x2f3843)
}
pub fn text() -> Rgba {
    rgb(0xe9eef3)
}
pub fn text_dim() -> Rgba {
    rgb(0xa4afbc)
}
pub fn muted() -> Rgba {
    rgb(0x6c7785)
}

/// Chia green, straight from the original GUI.
pub fn accent() -> Rgba {
    rgb(0x3aac59)
}
pub fn accent_hi() -> Rgba {
    rgb(0x4cd07a)
}
pub fn accent_soft() -> Rgba {
    rgba(0x3aac5926)
}
pub fn teal() -> Rgba {
    rgb(0x1f9e8f)
}
pub fn danger() -> Rgba {
    rgb(0xf0525a)
}
pub fn danger_soft() -> Rgba {
    rgba(0xf0525a22)
}
pub fn warning() -> Rgba {
    rgb(0xf5b94a)
}
pub fn warning_soft() -> Rgba {
    rgba(0xf5b94a1f)
}
pub fn info() -> Rgba {
    rgb(0x6aa7ff)
}
pub fn transparent() -> Rgba {
    rgba(0x00000000)
}
pub fn rgb_white() -> Rgba {
    rgb(0xffffff)
}
pub fn scrim() -> Rgba {
    rgba(0x05070acc)
}

pub fn hsla(c: Rgba) -> Hsla {
    c.into()
}
