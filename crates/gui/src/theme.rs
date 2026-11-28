//! Semantic theme token system for `cadence-gui`.
//!
//! No deck, chrome bar, or overlay ever hardcodes raw hex or RGBA values:
//! every visual element queries named semantic tokens from `&Theme`, making
//! future theme switching or user-defined themes a single struct swap.

/// Perceptual sRGB color with explicit conversion to linear GPU RGBA and `glyphon::Color`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: ((rgb >> 16) & 0xff) as u8,
            g: ((rgb >> 8) & 0xff) as u8,
            b: (rgb & 0xff) as u8,
            a: 1.0,
        }
    }

    pub const fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }

    pub fn scale_rgb(self, factor: f32) -> Self {
        let f = factor.clamp(0.0, 2.0);
        Self {
            r: ((f32::from(self.r) * f).round().clamp(0.0, 255.0)) as u8,
            g: ((f32::from(self.g) * f).round().clamp(0.0, 255.0)) as u8,
            b: ((f32::from(self.b) * f).round().clamp(0.0, 255.0)) as u8,
            a: self.a,
        }
    }

    pub fn mix(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        let inv = 1.0 - t;
        Self {
            r: (f32::from(self.r) * inv + f32::from(other.r) * t).round() as u8,
            g: (f32::from(self.g) * inv + f32::from(other.g) * t).round() as u8,
            b: (f32::from(self.b) * inv + f32::from(other.b) * t).round() as u8,
            a: self.a * inv + other.a * t,
        }
    }

    /// Convert to linear-space RGBA floats for `wgpu` sRGB/HDR render targets.
    pub fn to_linear(self) -> [f32; 4] {
        [
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
            self.a.clamp(0.0, 1.0),
        ]
    }

    pub fn to_wgpu_clear(self) -> wgpu::Color {
        let [r, g, b, a] = self.to_linear();
        wgpu::Color {
            r: f64::from(r),
            g: f64::from(g),
            b: f64::from(b),
            a: f64::from(a),
        }
    }

    pub fn to_glyphon(self) -> glyphon::Color {
        glyphon::Color::rgba(
            self.r,
            self.g,
            self.b,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }
}

fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceTokens {
    pub canvas: Color,
    pub elevated: Color,
    pub recessed: Color,
    pub header: Color,
    pub overlay: Color,
    pub overlay_scrim: Color,
    pub selection: Color,
    pub active_row: Color,
    pub key_white: Color,
    pub key_black: Color,
    pub key_root: Color,
    pub key_scale: Color,
}

#[derive(Clone, Debug)]
pub struct BorderTokens {
    pub hairline: Color,
    pub subtle: Color,
    pub focused: Color,
    pub grid_bar: Color,
    pub grid_beat: Color,
    pub grid_step: Color,
}

#[derive(Clone, Debug)]
pub struct TextTokens {
    pub primary: Color,
    pub secondary: Color,
    pub muted: Color,
    pub inverted: Color,
}

#[derive(Clone, Debug)]
pub struct AccentTokens {
    pub focus: Color,
    pub edit: Color,
    pub jam: Color,
    pub routing: Color,
    pub aero: Color,
    pub playhead: Color,
    /// Full-height timeline edit/cue cursor bar (distinct from `playhead` across all themes).
    pub edit_cursor: Color,
    pub warning_key: Color,
    pub modulation: Color,
    pub alert: Color,
    pub override_bar: Color,
    pub audition: Color,
}

#[derive(Clone, Debug)]
pub struct MeterTokens {
    pub low: Color,
    pub mid: Color,
    pub high: Color,
    /// Informational 32-bit float headroom (`> 0 dBFS` on internal bus or pre-limiter Master).
    pub hot: Color,
    /// True digital clipping (`> 0 dBFS` on Master post-fader DAC/WAV output or non-finite).
    pub clip: Color,
    /// Pre-fader / pre-insert input level indicator.
    pub pre_fader: Color,
    /// Unity `0 dBFS` reference hairline across meters and faders.
    pub zero_db: Color,
    /// Active dynamics gain reduction bar fill.
    pub gr: Color,
    pub track_bg: Color,
    pub hold_tick: Color,
}

#[derive(Clone, Copy, Debug)]
pub struct RadiusTokens {
    pub panel: f32,
    pub pill: f32,
    pub badge: f32,
    pub note: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct GlowTokens {
    pub playhead: f32,
    pub scope: f32,
    pub active_note: f32,
    pub override_bar: f32,
    pub clip: f32,
    pub focus_border: f32,
    pub edit_border: f32,
    pub jam_border: f32,
    pub bloom_strength: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct FontTokens {
    pub family: &'static str,
    pub body: f32,
    pub small: f32,
    pub title: f32,
    pub mono_advance_ratio: f32,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: &'static str,
    pub surface: SurfaceTokens,
    pub border: BorderTokens,
    pub text: TextTokens,
    pub accent: AccentTokens,
    pub meter: MeterTokens,
    pub tracks: [Color; 8],
    pub radii: RadiusTokens,
    pub glow: GlowTokens,
    pub font: FontTokens,
}

impl Default for Theme {
    fn default() -> Self {
        Self::cadence_dark()
    }
}

impl Theme {
    /// Default Cadence slate + phosphor theme.
    pub fn cadence_dark() -> Self {
        Self {
            name: "Cadence Dark",
            surface: SurfaceTokens {
                canvas: Color::hex(0x0C111B),
                elevated: Color::hex(0x121925),
                recessed: Color::hex(0x090D16),
                header: Color::hex(0x101723),
                overlay: Color::hex(0x131C2B),
                overlay_scrim: Color::hex(0x05080F).with_alpha(0.72),
                selection: Color::hex(0x1B2B3F),
                active_row: Color::hex(0x152030),
                key_white: Color::hex(0x121A28),
                key_black: Color::hex(0x0B101A),
                key_root: Color::hex(0x162839),
                key_scale: Color::hex(0x142031),
            },
            border: BorderTokens {
                hairline: Color::hex(0x263244),
                subtle: Color::hex(0x1A2434),
                focused: Color::hex(0x68DBCD),
                grid_bar: Color::hex(0x324259),
                grid_beat: Color::hex(0x1D283A),
                grid_step: Color::hex(0x141C2B),
            },
            text: TextTokens {
                primary: Color::hex(0xE2EAF2),
                secondary: Color::hex(0xA6B6C9),
                muted: Color::hex(0x6D7E94),
                inverted: Color::hex(0x0B101A),
            },
            accent: AccentTokens {
                focus: Color::hex(0x68DBCD),
                edit: Color::hex(0xFFB347),
                jam: Color::hex(0xD966FF),
                routing: Color::hex(0x56B6E2),
                aero: Color::hex(0x79B8F7),
                playhead: Color::hex(0x8ADB94),
                edit_cursor: Color::hex(0xE8F1FC),
                warning_key: Color::hex(0xF3C068),
                modulation: Color::hex(0xBCA3F4),
                alert: Color::hex(0xF78D89),
                override_bar: Color::hex(0x68DBCD),
                audition: Color::hex(0xF3C068),
            },
            meter: MeterTokens {
                low: Color::hex(0x56B6E2),
                mid: Color::hex(0x8ADB94),
                high: Color::hex(0xF3C068),
                hot: Color::hex(0xF5A642),
                clip: Color::hex(0xF78D89),
                pre_fader: Color::hex(0x7FA8F7),
                zero_db: Color::hex(0xE2EAF2),
                gr: Color::hex(0xF3C068),
                track_bg: Color::hex(0x0B1019),
                hold_tick: Color::hex(0xE2EAF2),
            },
            tracks: [
                Color::hex(0x68DBCD),
                Color::hex(0x56B6E2),
                Color::hex(0xF3C068),
                Color::hex(0xBCA3F4),
                Color::hex(0x8ADB94),
                Color::hex(0xF78D89),
                Color::hex(0x7FA8F7),
                Color::hex(0xE6A3D7),
            ],
            radii: RadiusTokens {
                panel: 5.0,
                pill: 4.0,
                badge: 3.0,
                note: 3.0,
            },
            glow: GlowTokens {
                playhead: 0.72,
                scope: 0.52,
                active_note: 0.45,
                override_bar: 0.38,
                clip: 0.85,
                focus_border: 0.25,
                edit_border: 0.55,
                jam_border: 0.45,
                bloom_strength: 0.42,
            },
            font: FontTokens {
                family: "JetBrains Mono",
                body: 13.0,
                small: 11.5,
                title: 13.5,
                mono_advance_ratio: 0.60,
            },
        }
    }

    pub fn from_id(id: cadence_core::config::ThemeId) -> Self {
        match id {
            cadence_core::config::ThemeId::CadenceDark => Self::cadence_dark(),
            cadence_core::config::ThemeId::MidnightNeon => Self::midnight_neon(),
            cadence_core::config::ThemeId::StudioLight => Self::studio_light(),
        }
    }

    /// Deep obsidian + electric neon phosphor theme for dark club/stage setups.
    pub fn midnight_neon() -> Self {
        let mut t = Self::cadence_dark();
        t.name = "Midnight Neon";
        t.surface = SurfaceTokens {
            canvas: Color::hex(0x070810),
            elevated: Color::hex(0x0E1020),
            recessed: Color::hex(0x05060C),
            header: Color::hex(0x0B0D1A),
            overlay: Color::hex(0x111428),
            overlay_scrim: Color::hex(0x020308).with_alpha(0.78),
            selection: Color::hex(0x1E1B3A),
            active_row: Color::hex(0x161730),
            key_white: Color::hex(0x0F1122),
            key_black: Color::hex(0x080914),
            key_root: Color::hex(0x1B1A38),
            key_scale: Color::hex(0x13152A),
        };
        t.border = BorderTokens {
            hairline: Color::hex(0x2B2748),
            subtle: Color::hex(0x1B1A33),
            focused: Color::hex(0x3DF5E6),
            grid_bar: Color::hex(0x3B3363),
            grid_beat: Color::hex(0x222040),
            grid_step: Color::hex(0x141428),
        };
        t.text = TextTokens {
            primary: Color::hex(0xF0F4FF),
            secondary: Color::hex(0xB4B8DC),
            muted: Color::hex(0x7076A0),
            inverted: Color::hex(0x070810),
        };
        t.accent = AccentTokens {
            focus: Color::hex(0x3DF5E6),
            edit: Color::hex(0xFFAE2B),
            jam: Color::hex(0xE040FB),
            routing: Color::hex(0x47A8FF),
            aero: Color::hex(0x52D1FF),
            playhead: Color::hex(0x5CFFA0),
            edit_cursor: Color::hex(0xF4F7FF),
            warning_key: Color::hex(0xFFD154),
            modulation: Color::hex(0xFF5CE1),
            alert: Color::hex(0xFF5577),
            override_bar: Color::hex(0x3DF5E6),
            audition: Color::hex(0xFFD154),
        };
        t.meter = MeterTokens {
            low: Color::hex(0x47A8FF),
            mid: Color::hex(0x5CFFA0),
            high: Color::hex(0xFFD154),
            hot: Color::hex(0xFF934F),
            clip: Color::hex(0xFF5577),
            pre_fader: Color::hex(0xA47CFF),
            zero_db: Color::hex(0xF0F4FF),
            gr: Color::hex(0xFFD154),
            track_bg: Color::hex(0x080914),
            hold_tick: Color::hex(0xF0F4FF),
        };
        t.tracks = [
            Color::hex(0x3DF5E6),
            Color::hex(0xFF5CE1),
            Color::hex(0xFFD154),
            Color::hex(0x47A8FF),
            Color::hex(0x5CFFA0),
            Color::hex(0xFF5577),
            Color::hex(0xA47CFF),
            Color::hex(0xFF934F),
        ];
        t.glow.bloom_strength = 0.56;
        t.glow.playhead = 0.85;
        t.glow.edit_border = 0.65;
        t.glow.jam_border = 0.55;
        t
    }

    /// Crisp daylight studio palette with high-contrast slate typography.
    pub fn studio_light() -> Self {
        let mut t = Self::cadence_dark();
        t.name = "Studio Light";
        t.surface = SurfaceTokens {
            canvas: Color::hex(0xECEEF2),
            elevated: Color::hex(0xF7F8FA),
            recessed: Color::hex(0xE1E5EC),
            header: Color::hex(0xE5E9F0),
            overlay: Color::hex(0xFAFBFD),
            overlay_scrim: Color::hex(0x161D29).with_alpha(0.38),
            selection: Color::hex(0xD2E6E9),
            active_row: Color::hex(0xE6EDF4),
            key_white: Color::hex(0xF4F6F9),
            key_black: Color::hex(0xE2E6EE),
            key_root: Color::hex(0xD4EAE7),
            key_scale: Color::hex(0xE8F0F6),
        };
        t.border = BorderTokens {
            hairline: Color::hex(0xBFC8D6),
            subtle: Color::hex(0xD2D8E2),
            focused: Color::hex(0x0E8B7E),
            grid_bar: Color::hex(0xAAB5C7),
            grid_beat: Color::hex(0xC6CFD9),
            grid_step: Color::hex(0xD9DFE8),
        };
        t.text = TextTokens {
            primary: Color::hex(0x182230),
            secondary: Color::hex(0x415064),
            muted: Color::hex(0x6C7B90),
            inverted: Color::hex(0xFAFBFD),
        };
        t.accent = AccentTokens {
            focus: Color::hex(0x0E8B7E),
            edit: Color::hex(0xC96200),
            jam: Color::hex(0x8E24AA),
            routing: Color::hex(0x1D6FB8),
            aero: Color::hex(0x16779E),
            playhead: Color::hex(0x1F8A4C),
            edit_cursor: Color::hex(0x1E3A5F),
            warning_key: Color::hex(0xB86A08),
            modulation: Color::hex(0x7B42BC),
            alert: Color::hex(0xC9363D),
            override_bar: Color::hex(0x0E8B7E),
            audition: Color::hex(0xB86A08),
        };
        t.meter = MeterTokens {
            low: Color::hex(0x1D6FB8),
            mid: Color::hex(0x1F8A4C),
            high: Color::hex(0xC97A0C),
            hot: Color::hex(0xD4620A),
            clip: Color::hex(0xC9363D),
            pre_fader: Color::hex(0x3B5EC4),
            zero_db: Color::hex(0x182230),
            gr: Color::hex(0xB86A08),
            track_bg: Color::hex(0xD5DCE6),
            hold_tick: Color::hex(0x182230),
        };
        t.tracks = [
            Color::hex(0x0E8B7E),
            Color::hex(0x1D6FB8),
            Color::hex(0xB86A08),
            Color::hex(0x7B42BC),
            Color::hex(0x1F8A4C),
            Color::hex(0xC9363D),
            Color::hex(0x3B5EC4),
            Color::hex(0xA63D85),
        ];
        t.glow = GlowTokens {
            playhead: 0.28,
            scope: 0.18,
            active_note: 0.16,
            override_bar: 0.14,
            clip: 0.35,
            focus_border: 0.10,
            edit_border: 0.24,
            jam_border: 0.20,
            bloom_strength: 0.12,
        };
        t
    }

    pub fn track_color(&self, index: usize) -> Color {
        self.tracks[index % self.tracks.len()]
    }

    /// Map a 7-bit hardware MIDI RGB LED triplet (`0..=127`) into a sRGB `Color`.
    pub fn hardware_led(&self, r: u8, g: u8, b: u8, alpha: f32) -> Color {
        let scale = |v: u8| ((u16::from(v.min(127)) * 255) / 127) as u8;
        Color::rgb(scale(r), scale(g), scale(b)).with_alpha(alpha)
    }

    /// Estimate character advance width in logical pixels for layout truncation.
    pub fn char_width(&self, font_size: f32) -> f32 {
        font_size * self.font.mono_advance_ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn color_distance(a: Color, b: Color) -> f32 {
        let dr = (a.r as f32 - b.r as f32) / 255.0;
        let dg = (a.g as f32 - b.g as f32) / 255.0;
        let db = (a.b as f32 - b.b as f32) / 255.0;
        (dr * dr + dg * dg + db * db).sqrt()
    }

    #[test]
    fn theme_tokens_expose_distinct_focus_overlay_and_jam_accents() {
        for (name, theme) in [
            ("Cadence Dark", Theme::cadence_dark()),
            ("Midnight Neon", Theme::midnight_neon()),
            ("Studio Light", Theme::studio_light()),
        ] {
            let d_fe = color_distance(theme.accent.focus, theme.accent.edit);
            let d_fj = color_distance(theme.accent.focus, theme.accent.jam);
            let d_ej = color_distance(theme.accent.edit, theme.accent.jam);
            assert!(
                d_fe > 0.30,
                "{name}: focus vs overlay-edit accent too close ({d_fe:.3})"
            );
            assert!(
                d_fj > 0.30,
                "{name}: focus vs jam accent too close ({d_fj:.3})"
            );
            assert!(
                d_ej > 0.30,
                "{name}: overlay-edit vs jam accent too close ({d_ej:.3})"
            );
            assert!(
                theme.glow.jam_border > 0.0,
                "{name}: jam_border glow must be positive"
            );
            let d_cp = color_distance(theme.accent.edit_cursor, theme.accent.playhead);
            assert!(
                d_cp > 0.30,
                "{name}: edit_cursor vs playhead accent too close ({d_cp:.3})"
            );
        }
    }
}
