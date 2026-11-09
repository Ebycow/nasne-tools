//! 配色（ライト / ダーク）とジャンルの色

use eframe::egui::{Color32, FontFamily};

const GENRE_COLOR: [u32; 16] = [
    0x4a7fd4, 0x2f9e5b, 0xc9a227, 0xd0508a, 0x8a5cd0, 0xe07b2c, 0xb8433a, 0xe05a9c, 0x2b9aa8, 0x7d6a4f, 0x6a9a2f,
    0x5b8db8, 0, 0, 0, 0x8c8c8c,
];

const fn hex(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

pub fn genre_color(g: Option<u8>) -> Option<Color32> {
    g.map(|g| GENRE_COLOR[g as usize & 15]).filter(|&c| c != 0).map(hex)
}

#[derive(Clone, Copy)]
pub struct Pal {
    pub bg: Color32,
    pub panel: Color32,
    pub line: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub rec: Color32,
    pub rec_soft: Color32,
    pub past: Color32,
    pub now: Color32,
    pub warn: Color32,
    pub warn_soft: Color32,
    pub ok: Color32,
}

pub const LIGHT: Pal = Pal {
    bg: hex(0xf5f5f2),
    panel: hex(0xffffff),
    line: hex(0xdeddd6),
    text: hex(0x1f1f1c),
    muted: hex(0x6f6e68),
    accent: hex(0x2563c9),
    accent_soft: hex(0xe3ecfb),
    rec: hex(0xc8372d),
    rec_soft: hex(0xfbe7e5),
    past: hex(0xecebe6),
    now: hex(0xe0861a),
    warn: hex(0xa66300),
    warn_soft: hex(0xfdf0dc),
    ok: hex(0x2f8a4f),
};

pub const DARK: Pal = Pal {
    bg: hex(0x161615),
    panel: hex(0x1f1f1d),
    line: hex(0x34332f),
    text: hex(0xe9e8e3),
    muted: hex(0x9a9890),
    accent: hex(0x6c9ef0),
    accent_soft: hex(0x23324d),
    rec: hex(0xf0776c),
    rec_soft: hex(0x48221f),
    past: hex(0x1a1a19),
    now: hex(0xf0a040),
    warn: hex(0xf0b860),
    warn_soft: hex(0x3d2f17),
    ok: hex(0x6cc28a),
};

pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}
