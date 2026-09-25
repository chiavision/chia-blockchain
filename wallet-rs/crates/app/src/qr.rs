//! QR codes painted directly as quads — no image decoding, no temp files.

use gpui::prelude::*;
use gpui::{Bounds, canvas, fill, point, px, rgb, size};
use qrcode::{Color, EcLevel, QrCode};

const QUIET: usize = 3;

pub fn qr_code(data: &str, side: f32) -> impl IntoElement {
    let code = QrCode::with_error_correction_level(data.as_bytes(), EcLevel::M).ok();
    let (width, modules) = code.map(|c| (c.width(), c.to_colors())).unwrap_or((0, Vec::new()));
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            window.paint_quad(fill(bounds, rgb(0xffffff)).corner_radii(px(12.)));
            if width == 0 {
                return;
            }
            let n = width + QUIET * 2;
            // Whole-pixel modules keep edges crisp for scanners.
            let cell = (f32::from(bounds.size.width) / n as f32).floor().max(1.0);
            let used = cell * n as f32;
            let off_x = bounds.origin.x + px(((f32::from(bounds.size.width) - used) / 2.0).floor());
            let off_y = bounds.origin.y + px(((f32::from(bounds.size.height) - used) / 2.0).floor());
            for (i, m) in modules.iter().enumerate() {
                if *m == Color::Dark {
                    let (x, y) = ((i % width + QUIET) as f32, (i / width + QUIET) as f32);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(off_x + px(x * cell), off_y + px(y * cell)),
                            size(px(cell), px(cell)),
                        ),
                        rgb(0x0a0c0f),
                    ));
                }
            }
        },
    )
    .size(px(side))
    .flex_none()
}
