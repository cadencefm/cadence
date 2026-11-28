//! Unified parameter addressing (`InstrumentParam`, `ParamAddress`), normalization (`0.0..=1.0`),
//! Rack path resolution (`parse_for_rack`), and value interpolation modes.

use crate::dsp::MAX_CHANNEL_GAIN;
use crate::mixer::{ChannelParam, ChannelRef, EffectParam};
use crate::rack::{compile_zone_graph, ModuleKindTag};
pub use crate::rack::{ModuleParam, RackParam, RackPatch, ZoneParam};
use anyhow::{anyhow, ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// Normalize a user-entered choice or parameter token for case- and separator-insensitive matching.
pub fn normalize_choice_token(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-', '/'], "_")
}

/// Shared contract for closed discrete choice enums (filter modes, drive circuits,
/// oscillator waveforms, routing topologies, UI tabs, etc.) with canonical slugs,
/// human-readable labels, clamping index conversion, case-insensitive parsing,
/// and wrapping cycle navigation.
pub trait ChoiceEnum: Copy + Eq + Sized + 'static {
    const ALL: &'static [Self];

    fn slug(self) -> &'static str;
    fn label(self) -> &'static str;

    #[inline]
    fn to_index(self) -> usize {
        Self::ALL.iter().position(|&v| v == self).unwrap_or(0)
    }

    #[inline]
    fn from_index(index: usize) -> Self {
        Self::ALL[index.min(Self::ALL.len() - 1)]
    }

    fn parse(s: &str) -> Option<Self>;

    #[inline]
    fn cycle(self, delta: i32) -> Self {
        let len = Self::ALL.len() as isize;
        let idx = self.to_index() as isize;
        Self::ALL[(idx + delta as isize).rem_euclid(len) as usize]
    }
}

/// Define a closed `#[repr(u8)]` choice enum with `serde(rename_all = "snake_case")`,
/// a [`crate::params::ChoiceEnum`] implementation, and idiomatic inherent methods
/// (`ALL`, `slug`, `label`, `to_index`, `from_index`, `parse`, `cycle`).
///
/// Use `enum Name { ... }` for `pub const ALL: &'static [Self]`, or
/// `enum Name [array] { ... }` for a fixed-size `pub const ALL: [Self; N]` array.
#[macro_export]
macro_rules! define_choice_enum {
    // Internal helper: emit the enum declaration itself.
    (@enum
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$var_meta:meta])*
                $variant:ident
            ),+
        }
    ) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, ::serde::Serialize, ::serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[repr(u8)]
        $(#[$enum_meta])*
        $vis enum $name {
            $(
                $(#[$var_meta])*
                $variant,
            )+
        }
    };

    // Internal helper: emit inherent convenience wrappers delegating to `ChoiceEnum`.
    (@inherent) => {
        #[inline]
        pub fn slug(self) -> &'static str {
            <Self as $crate::params::ChoiceEnum>::slug(self)
        }

        #[inline]
        pub fn label(self) -> &'static str {
            <Self as $crate::params::ChoiceEnum>::label(self)
        }

        #[inline]
        pub fn to_index(self) -> usize {
            <Self as $crate::params::ChoiceEnum>::to_index(self)
        }

        #[inline]
        pub fn from_index(index: usize) -> Self {
            <Self as $crate::params::ChoiceEnum>::from_index(index)
        }

        pub fn parse(input: &str) -> Option<Self> {
            <Self as $crate::params::ChoiceEnum>::parse(input)
        }

        #[inline]
        pub fn cycle(self, delta: i32) -> Self {
            <Self as $crate::params::ChoiceEnum>::cycle(self, delta)
        }
    };

    // Internal helper: emit `ChoiceEnum` trait method bodies.
    (@trait_body {
        $(
            $variant:ident => ($slug:literal, $label:literal, [ $($alias:literal),* ])
        ),+
    }) => {
        #[inline]
        fn slug(self) -> &'static str {
            match self {
                $( Self::$variant => $slug, )+
            }
        }

        #[inline]
        fn label(self) -> &'static str {
            match self {
                $( Self::$variant => $label, )+
            }
        }

        #[inline]
        fn to_index(self) -> usize {
            self as usize
        }

        fn parse(input: &str) -> Option<Self> {
            let key = $crate::params::normalize_choice_token(input);
            match key.as_str() {
                $(
                    $slug $( | $alias )* => Some(Self::$variant),
                )+
                _ => <Self as $crate::params::ChoiceEnum>::ALL
                    .iter()
                    .copied()
                    .find(|v| $crate::params::normalize_choice_token(v.label()) == key),
            }
        }
    };

    // Fixed-size array form: `pub const ALL: [Self; N]`.
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident [array] {
            $(
                $(#[$var_meta:meta])*
                $variant:ident => ($slug:literal, $label:literal $(, [ $($alias:literal),* $(,)? ])? )
            ),+ $(,)?
        }
    ) => {
        $crate::define_choice_enum! {
            @enum
            $(#[$enum_meta])*
            $vis enum $name {
                $(
                    $(#[$var_meta])*
                    $variant
                ),+
            }
        }

        impl $name {
            pub const ALL: [Self; [ $( Self::$variant ),+ ].len()] = [ $( Self::$variant ),+ ];
            $crate::define_choice_enum!(@inherent);
        }

        impl $crate::params::ChoiceEnum for $name {
            const ALL: &'static [Self] = &Self::ALL;
            $crate::define_choice_enum! {
                @trait_body {
                    $(
                        $variant => ($slug, $label, [ $( $($alias),* )? ])
                    ),+
                }
            }
        }
    };

    // Default slice form: `pub const ALL: &'static [Self]`.
    (
        $(#[$enum_meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$var_meta:meta])*
                $variant:ident => ($slug:literal, $label:literal $(, [ $($alias:literal),* $(,)? ])? )
            ),+ $(,)?
        }
    ) => {
        $crate::define_choice_enum! {
            @enum
            $(#[$enum_meta])*
            $vis enum $name {
                $(
                    $(#[$var_meta])*
                    $variant
                ),+
            }
        }

        impl $name {
            pub const ALL: &'static [Self] = &[ $( Self::$variant ),+ ];
            $crate::define_choice_enum!(@inherent);
        }

        impl $crate::params::ChoiceEnum for $name {
            const ALL: &'static [Self] = Self::ALL;
            $crate::define_choice_enum! {
                @trait_body {
                    $(
                        $variant => ($slug, $label, [ $( $($alias),* )? ])
                    ),+
                }
            }
        }
    };
}

/// Authored parameter target for instruments (parameter links, modulations, and panel pins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum InstrumentParam {
    Rack {
        param: RackParam,
    },
    Zone {
        zone: u16,
        param: ZoneParam,
    },
    Module {
        zone: u16,
        module: u16,
        param: ModuleParam,
    },
    Cable {
        zone: u16,
        cable: u16,
    },
}

impl InstrumentParam {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Rack { param } => param.label(),
            Self::Zone { param, .. } => param.label(),
            Self::Module { param, .. } => param.label(),
            Self::Cable { .. } => "Cable Amt",
        }
    }

    pub fn is_automatable(self) -> bool {
        match self {
            Self::Rack { param } => param.is_automatable(),
            Self::Zone { param, .. } => param.is_automatable(),
            Self::Module { param, .. } => param.is_automatable(),
            Self::Cable { .. } => true,
        }
    }

    pub fn is_frame_based(self) -> bool {
        matches!(self, Self::Module { param, .. } if param.is_frame_based())
    }

    pub fn format_normalized(self, norm: f32, total_frames: usize) -> String {
        match self {
            Self::Rack { param } => param.format_normalized(norm),
            Self::Zone { param, .. } => param.format_normalized(norm),
            Self::Module { param, .. } => param.format_normalized(norm, total_frames),
            Self::Cable { .. } => {
                let amt = -2.0 + norm.clamp(0.0, 1.0) * 4.0;
                format!("{:+.0}%", amt * 100.0)
            }
        }
    }

    /// Unified parameter resolver for a [`RackPatch`].
    ///
    /// Supports:
    /// - `<zone>.<module>.<param>` (e.g. `z1.filter.cutoff`, `BD.body_vco.tune`)
    /// - `<zone>.c<cable_id>` / `<zone>.cable.<cable_id>` (and `c<id>` / `cable.<id>` on 1-zone racks)
    /// - `rack.<param>` / `bus.<param>` (e.g. `rack.cutoff`, `rack.gain`)
    /// - `<zone>.<zone_param>` (e.g. `BD.gain`, `z1.glide`)
    /// - `<module>.<param>` on single-zone racks (e.g. `filter.cutoff`, `amp_env.decay`, `out.pan`)
    /// - Unambiguous shorthand (`cutoff`, `res`, `decay`, `env_mod`, `pan`, `glide`, `bits`, etc.)
    ///   matching curated `panel_pins` first, then unique module parameters, then zone/rack params.
    pub fn parse_for_rack(name: &str, rack: &RackPatch, require_automatable: bool) -> Option<Self> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return None;
        }
        let ok = |p: Self| (!require_automatable || p.is_automatable()).then_some(p);

        let parts: Vec<&str> = trimmed
            .split(['.', ':'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();

        // 1. 3-part explicit path: `<zone>.<module>.<param>` or `<zone>.cable.<id>`.
        if parts.len() == 3 {
            let zone_id = rack.resolve_zone_id(parts[0])?;
            let zone = rack.zone(zone_id)?;
            if parts[1].eq_ignore_ascii_case("cable") || parts[1].eq_ignore_ascii_case("c") {
                if let Some(cable_id) = parse_cable_token(parts[2], zone) {
                    return ok(Self::Cable {
                        zone: zone_id,
                        cable: cable_id,
                    });
                }
                return None;
            }
            let mod_id = zone.resolve_module_id(parts[1])?;
            let module = zone.module(mod_id)?;
            let param = ModuleParam::parse_for_kind(parts[2], module.kind.tag())?;
            return ok(Self::Module {
                zone: zone_id,
                module: mod_id,
                param,
            });
        }

        // 2. 2-part path: `rack.<param>`, `cable.<id>`, `<module>.<param>`, `<zone>.c<id>`, `<zone>.<zone_param>`.
        if parts.len() == 2 {
            let a = parts[0];
            let b = parts[1];
            if a.eq_ignore_ascii_case("rack") || a.eq_ignore_ascii_case("bus") {
                return RackParam::parse(b).and_then(|param| ok(Self::Rack { param }));
            }
            if rack.zones.len() == 1 {
                let z0 = &rack.zones[0];
                if a.eq_ignore_ascii_case("cable") || a.eq_ignore_ascii_case("c") {
                    if let Some(cable_id) = parse_cable_token(b, z0) {
                        return ok(Self::Cable {
                            zone: z0.id,
                            cable: cable_id,
                        });
                    }
                }
                if let Some(mod_id) = z0.resolve_module_id(a) {
                    let module = z0.module(mod_id)?;
                    if let Some(param) = ModuleParam::parse_for_kind(b, module.kind.tag()) {
                        return ok(Self::Module {
                            zone: z0.id,
                            module: mod_id,
                            param,
                        });
                    }
                }
            }
            if let Some(zone_id) = rack.resolve_zone_id(a) {
                let zone = rack.zone(zone_id)?;
                if let Some(cable_id) = parse_prefixed_cable_token(b, zone) {
                    return ok(Self::Cable {
                        zone: zone_id,
                        cable: cable_id,
                    });
                }
                if let Some(param) = ZoneParam::parse(b) {
                    return ok(Self::Zone {
                        zone: zone_id,
                        param,
                    });
                }
            }
            return None;
        }

        // 3. 1-part shorthand.
        let norm_key = normalize_choice_token(trimmed);

        // 3a. Single-zone cable shorthand (`c5`, `cable5`).
        if rack.zones.len() == 1 {
            let z0 = &rack.zones[0];
            if let Some(cable_id) = parse_prefixed_cable_token(trimmed, z0) {
                return ok(Self::Cable {
                    zone: z0.id,
                    cable: cable_id,
                });
            }
        }

        // 3b. Matching curated PanelPin label (e.g. `env_mod` on Acid 303 -> cable #5!).
        if let Some(pin) = rack.panel_pins.iter().find(|pin| {
            normalize_choice_token(pin.display_label()) == norm_key
                && rack.contains_param(pin.target)
        }) {
            if let Some(found) = ok(pin.target) {
                return Some(found);
            }
        }

        // 3c. Single-zone unambiguous module parameter shorthand.
        if rack.zones.len() == 1 {
            let z0 = &rack.zones[0];
            let mut matches: Vec<(u16, ModuleParam)> = Vec::new();
            for m in &z0.modules {
                if let Some(p) = ModuleParam::parse_for_kind(trimmed, m.kind.tag()) {
                    if !require_automatable || p.is_automatable() {
                        matches.push((m.id, p));
                    }
                }
            }
            if matches.len() == 1 {
                let (module, param) = matches[0];
                return ok(Self::Module {
                    zone: z0.id,
                    module,
                    param,
                });
            }
            if matches.len() > 1 {
                // Disambiguate multiple Env modules by preferring the primary amp lifetime envelope.
                if matches.iter().all(|(mid, _)| {
                    z0.module(*mid)
                        .is_some_and(|m| m.kind.tag() == ModuleKindTag::Env)
                }) {
                    if let Ok(graph) = compile_zone_graph(z0) {
                        if let Some(&amp_slot) = graph.lifetime_envs.first() {
                            let amp_id = graph.ops[usize::from(amp_slot)].module_id;
                            if let Some(&(_, param)) =
                                matches.iter().find(|(mid, _)| *mid == amp_id)
                            {
                                return ok(Self::Module {
                                    zone: z0.id,
                                    module: amp_id,
                                    param,
                                });
                            }
                        }
                    }
                }
            }

            if let Some(param) = ZoneParam::parse(trimmed) {
                return ok(Self::Zone { zone: z0.id, param });
            }
        }

        // 3d. Rack-bus parameter fallback (`gain`, `pan`, `tune`, `cutoff`, `resonance`).
        RackParam::parse(trimmed).and_then(|param| ok(Self::Rack { param }))
    }
}

fn parse_cable_token(tok: &str, zone: &crate::rack::RackZone) -> Option<u16> {
    let stripped = tok
        .trim()
        .strip_prefix("cable")
        .or_else(|| tok.trim().strip_prefix("CABLE"))
        .or_else(|| tok.trim().strip_prefix('c'))
        .or_else(|| tok.trim().strip_prefix('C'))
        .unwrap_or(tok.trim());
    let id = stripped.parse::<u16>().ok()?;
    zone.cable(id).map(|c| c.id)
}

fn parse_prefixed_cable_token(tok: &str, zone: &crate::rack::RackZone) -> Option<u16> {
    let t = tok.trim();
    let lower = t.to_ascii_lowercase();
    let rest = lower
        .strip_prefix("cable")
        .or_else(|| lower.strip_prefix('c'))?;
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let id = rest.parse::<u16>().ok()?;
    zone.cable(id).map(|c| c.id)
}

/// Unified native-value parameter address for `Edit::SetParam`, Macros, MIDI Learn, and UI controls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ParamAddress {
    Rack {
        instrument: u64,
        param: RackParam,
    },
    Zone {
        instrument: u64,
        zone: u16,
        param: ZoneParam,
    },
    Module {
        instrument: u64,
        zone: u16,
        module: u16,
        param: ModuleParam,
    },
    Cable {
        instrument: u64,
        zone: u16,
        cable: u16,
    },
    Channel {
        channel: ChannelRef,
        param: ChannelParam,
    },
    Send {
        channel: ChannelRef,
        target: u64,
    },
    Effect {
        device: u64,
        param: EffectParam,
    },
}

impl ParamAddress {
    #[inline]
    pub fn from_instrument(instrument: u64, param: InstrumentParam) -> Self {
        match param {
            InstrumentParam::Rack { param } => Self::Rack { instrument, param },
            InstrumentParam::Zone { zone, param } => Self::Zone {
                instrument,
                zone,
                param,
            },
            InstrumentParam::Module {
                zone,
                module,
                param,
            } => Self::Module {
                instrument,
                zone,
                module,
                param,
            },
            InstrumentParam::Cable { zone, cable } => Self::Cable {
                instrument,
                zone,
                cable,
            },
        }
    }

    #[inline]
    pub fn as_instrument(self) -> Option<(u64, InstrumentParam)> {
        match self {
            Self::Rack { instrument, param } => Some((instrument, InstrumentParam::Rack { param })),
            Self::Zone {
                instrument,
                zone,
                param,
            } => Some((instrument, InstrumentParam::Zone { zone, param })),
            Self::Module {
                instrument,
                zone,
                module,
                param,
            } => Some((
                instrument,
                InstrumentParam::Module {
                    zone,
                    module,
                    param,
                },
            )),
            Self::Cable {
                instrument,
                zone,
                cable,
            } => Some((instrument, InstrumentParam::Cable { zone, cable })),
            Self::Channel { .. } | Self::Send { .. } | Self::Effect { .. } => None,
        }
    }

    /// Whether this parameter can be modulated, bound to Macros, or recorded via MIDI CC/Learn.
    pub fn is_automatable(self) -> bool {
        match self.as_instrument() {
            Some((_, p)) => p.is_automatable(),
            None => true,
        }
    }
}

crate::define_choice_enum! {
    /// Interpolation mode for `Values` patterns and compiled automation/modulation lanes:
    /// `Hold` latches each breakpoint until the next; `Ramp` interpolates linearly in
    /// normalized space across audio control blocks.
    #[derive(Default)]
    pub enum ValueMode {
        #[default]
        Hold => ("hold", "HOLD"),
        Ramp => ("ramp", "RAMP"),
    }
}

impl ValueMode {
    pub fn toggle(self) -> Self {
        self.cycle(1)
    }
}

pub use crate::dsp::math::{db_to_lin, lin_to_db, log_to_norm, norm_to_log};

pub fn format_pan(pan: f32) -> String {
    if pan.abs() < 0.02 {
        "Center".into()
    } else if pan < 0.0 {
        format!("L{:.0}", -pan * 100.0)
    } else {
        format!("R{:.0}", pan * 100.0)
    }
}

/// Parse a stereo pan value (`-1.0..=1.0`) from center, L/R, percentage, or decimal text.
pub fn parse_pan(raw: &str) -> Result<f32> {
    const ERR: &str = "Pan must be within -1.0..1.0, -100%..100%, or L100..C..R100";
    let trimmed = raw.trim();
    ensure!(!trimmed.is_empty(), "{ERR}");
    let lower = trimmed.to_ascii_lowercase();
    let v = if matches!(lower.as_str(), "c" | "center") {
        0.0
    } else if let Some(rest) = lower.strip_prefix('l') {
        let num = rest.trim().strip_suffix('%').unwrap_or(rest.trim()).trim();
        let mag: f32 = num.parse().map_err(|_| anyhow!("{ERR}"))?;
        ensure!(mag.is_finite() && (0.0..=100.0).contains(&mag), "{ERR}");
        -(mag / 100.0)
    } else if let Some(rest) = lower.strip_prefix('r') {
        let num = rest.trim().strip_suffix('%').unwrap_or(rest.trim()).trim();
        let mag: f32 = num.parse().map_err(|_| anyhow!("{ERR}"))?;
        ensure!(mag.is_finite() && (0.0..=100.0).contains(&mag), "{ERR}");
        mag / 100.0
    } else if let Some(pct) = trimmed.strip_suffix('%') {
        pct.trim().parse::<f32>().map_err(|_| anyhow!("{ERR}"))? / 100.0
    } else {
        trimmed.parse::<f32>().map_err(|_| anyhow!("{ERR}"))?
    };
    ensure!(v.is_finite() && (-1.0..=1.0).contains(&v), "{ERR}");
    Ok(if v == 0.0 { 0.0 } else { v })
}

/// Parse a native channel strip gain (`0.0..=MAX_CHANNEL_GAIN`) from decibels, percentage, or scalar text.
pub fn parse_channel_gain(raw: &str) -> Result<f32> {
    const ERR: &str = "Channel gain must be within -inf..+6.0 dB (0..2.0 or 0%..200%)";
    let trimmed = raw.trim();
    ensure!(!trimmed.is_empty(), "{ERR}");
    let lower = trimmed.to_ascii_lowercase();
    if lower == "-inf" {
        return Ok(0.0);
    }
    if let Some(rest) = lower.strip_suffix("db") {
        let num = rest.trim();
        if num == "-inf" {
            return Ok(0.0);
        }
        let db: f32 = num.parse().map_err(|_| anyhow!("{ERR}"))?;
        ensure!(db.is_finite() && db <= 6.021, "{ERR}");
        let lin = if db == 0.0 { 1.0 } else { db_to_lin(db) };
        ensure!(
            lin.is_finite() && (0.0..=MAX_CHANNEL_GAIN).contains(&lin),
            "{ERR}"
        );
        return Ok(if lin == 0.0 { 0.0 } else { lin });
    }
    if let Some(pct) = trimmed.strip_suffix('%') {
        let n: f32 = pct.trim().parse().map_err(|_| anyhow!("{ERR}"))?;
        let lin = n / 100.0;
        ensure!(
            n.is_finite() && lin.is_finite() && (0.0..=MAX_CHANNEL_GAIN).contains(&lin),
            "{ERR}"
        );
        return Ok(if lin == 0.0 { 0.0 } else { lin });
    }
    let lin: f32 = trimmed.parse().map_err(|_| anyhow!("{ERR}"))?;
    ensure!(
        lin.is_finite() && (0.0..=MAX_CHANNEL_GAIN).contains(&lin),
        "{ERR}"
    );
    Ok(if lin == 0.0 { 0.0 } else { lin })
}

pub fn format_hz(hz: f32) -> String {
    if hz >= 1_000.0 {
        format!("{:.2} kHz", hz / 1_000.0)
    } else {
        format!("{:.0} Hz", hz)
    }
}

pub fn format_db(v: f32) -> String {
    if v.abs() < 0.05 {
        "0.0 dB".into()
    } else {
        format!("{v:+.1} dB")
    }
}

pub fn format_ms(v: f32) -> String {
    if v >= 1_000.0 {
        format!("{:.2} s", v / 1_000.0)
    } else {
        format!("{v:.1} ms")
    }
}

pub fn parse_db(raw: &str) -> Result<f32> {
    let lower = raw.trim().to_ascii_lowercase();
    lower
        .strip_suffix("db")
        .unwrap_or(&lower)
        .trim()
        .parse::<f32>()
        .with_context(|| format!("Invalid dB value '{raw}'"))
}

pub fn parse_hz(raw: &str) -> Result<f32> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(v) = lower.strip_suffix("khz") {
        Ok(v.trim().parse::<f32>()? * 1000.0)
    } else {
        Ok(lower.strip_suffix("hz").unwrap_or(&lower).trim().parse()?)
    }
}

pub fn parse_ms(raw: &str) -> Result<f32> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(v) = lower.strip_suffix("ms") {
        Ok(v.trim().parse::<f32>()?)
    } else if let Some(v) = lower.strip_suffix('s') {
        Ok(v.trim().parse::<f32>()? * 1000.0)
    } else {
        Ok(lower.parse::<f32>()?)
    }
}

pub fn parse_percent_or_unit(raw: &str) -> Result<f32> {
    let lower = raw.trim().to_ascii_lowercase();
    if let Some(v) = lower.strip_suffix('%') {
        Ok(v.trim().parse::<f32>()? / 100.0)
    } else {
        Ok(lower.parse::<f32>()?)
    }
}

/// Format a numeric choice index as the matching [`ChoiceEnum`] variant label.
#[inline]
pub(crate) fn format_choice_index<T: ChoiceEnum>(value: f32) -> &'static str {
    T::from_index(value.round().max(0.0) as usize).label()
}

/// Parse a [`ChoiceEnum`] token into its `f32` discriminant index.
#[inline]
pub(crate) fn parse_choice_index<T: ChoiceEnum>(raw: &str, err: &'static str) -> Result<f32> {
    T::parse(raw).map(|m| m.to_index() as f32).context(err)
}

/// Snap `current` onto a `1/scale` grid and step by `fine_units` or `coarse_units` grid cells.
#[inline]
pub(crate) fn nudge_quantized(
    current: f32,
    dir: i8,
    scale: f32,
    fine_units: f32,
    coarse_units: f32,
    coarse: bool,
) -> f32 {
    let step = if coarse { coarse_units } else { fine_units };
    ((current * scale).round() + f32::from(dir) * step) / scale
}

/// Step a positive log-domain parameter multiplicatively by `fine_factor` or `coarse_factor`.
#[inline]
pub(crate) fn nudge_log(
    current: f32,
    dir: i8,
    fine_factor: f32,
    coarse_factor: f32,
    coarse: bool,
) -> f32 {
    let factor = if coarse { coarse_factor } else { fine_factor };
    if dir >= 0 {
        current * factor
    } else {
        current / factor
    }
}

/// Apply a resolved normalized Rack modulation onto a working [`RackPatch`].
pub fn apply_rack_modulation(
    rack: &mut RackPatch,
    param: InstrumentParam,
    norm: f32,
    total_frames: usize,
) -> bool {
    rack.write_param_normalized(param, norm, total_frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_for_rack_resolves_explicit_paths_cables_pins_and_shorthand() {
        let synth = RackPatch::default_synth();
        let acid = RackPatch::acid_303();
        let sampler = RackPatch::default_sampler();

        // 1. Unambiguous single-zone shorthand resolves cleanly.
        assert_eq!(
            InstrumentParam::parse_for_rack("cutoff", &synth, true),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff
            })
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("res", &synth, true),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Resonance
            })
        );
        // `decay` resolves to Amp Env (#4) via PanelPin / lifetime_envs.
        assert_eq!(
            InstrumentParam::parse_for_rack("decay", &synth, true),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 4,
                param: ModuleParam::Decay
            })
        );
        // Curated cable pin label (`env_mod`) resolves straight to cable #5!
        assert_eq!(
            InstrumentParam::parse_for_rack("env_mod", &acid, true),
            Some(InstrumentParam::Cable { zone: 1, cable: 5 })
        );
        // Explicit cable ref (`c5`, `z1.c5`, `z1.cable.5`) resolves cleanly.
        assert_eq!(
            InstrumentParam::parse_for_rack("c5", &acid, true),
            Some(InstrumentParam::Cable { zone: 1, cable: 5 })
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("z1.cable.5", &acid, true),
            Some(InstrumentParam::Cable { zone: 1, cable: 5 })
        );
        // Explicit 2-part `<module>.<param>` and 3-part `<zone>.<module>.<param>`.
        assert_eq!(
            InstrumentParam::parse_for_rack("filter_env.decay", &synth, true),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 5,
                param: ModuleParam::Decay
            })
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("z1.filter.cutoff", &synth, true),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 2,
                param: ModuleParam::Cutoff
            })
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("rack.cutoff", &synth, true),
            Some(InstrumentParam::Rack {
                param: RackParam::Cutoff
            })
        );

        // 2. Non-automatable structural parameters are rejected when `require_automatable = true`,
        //    and accepted when `require_automatable = false`.
        assert_eq!(
            InstrumentParam::parse_for_rack("voice_mode", &synth, true),
            None
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("voice_mode", &synth, false),
            Some(InstrumentParam::Zone {
                zone: 1,
                param: ZoneParam::VoiceMode
            })
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("sampler.loop_mode", &sampler, true),
            None
        );
        assert_eq!(
            InstrumentParam::parse_for_rack("sampler.loop_mode", &sampler, false),
            Some(InstrumentParam::Module {
                zone: 1,
                module: 1,
                param: ModuleParam::LoopMode
            })
        );

        // 3. ParamAddress roundtrip & automatability parity.
        let addr = ParamAddress::from_instrument(7, InstrumentParam::Cable { zone: 1, cable: 5 });
        assert_eq!(
            addr.as_instrument(),
            Some((7, InstrumentParam::Cable { zone: 1, cable: 5 }))
        );
        assert!(addr.is_automatable());
        assert!(!ParamAddress::Zone {
            instrument: 7,
            zone: 1,
            param: ZoneParam::VoiceMode,
        }
        .is_automatable());
    }

    #[test]
    fn parse_channel_gain_accepts_db_percent_scalar_and_rejects_out_of_range() {
        assert_eq!(parse_channel_gain("-inf").unwrap(), 0.0);
        assert_eq!(parse_channel_gain("-infdb").unwrap(), 0.0);
        assert_eq!(parse_channel_gain("-INF dB").unwrap(), 0.0);
        assert_eq!(parse_channel_gain("0dB").unwrap(), 1.0);
        assert_eq!(parse_channel_gain("+0.0 dB").unwrap(), 1.0);
        assert_eq!(parse_channel_gain("-0.0 dB").unwrap(), 1.0);
        assert!((parse_channel_gain("+6dB").unwrap() - db_to_lin(6.0)).abs() < 1e-6);
        assert!((parse_channel_gain("+3.5 dB").unwrap() - db_to_lin(3.5)).abs() < 1e-6);
        assert!((parse_channel_gain("-6dB").unwrap() - db_to_lin(-6.0)).abs() < 1e-6);
        assert!((parse_channel_gain("-60 dB").unwrap() - 0.001).abs() < 1e-6);
        assert_eq!(parse_channel_gain("0").unwrap(), 0.0);
        assert!((parse_channel_gain("0.5").unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(parse_channel_gain("1.0").unwrap(), 1.0);
        assert!((parse_channel_gain("1.5").unwrap() - 1.5).abs() < 1e-6);
        assert_eq!(parse_channel_gain("2.0").unwrap(), 2.0);
        assert_eq!(parse_channel_gain("0%").unwrap(), 0.0);
        assert_eq!(parse_channel_gain("100%").unwrap(), 1.0);
        assert!((parse_channel_gain("150%").unwrap() - 1.5).abs() < 1e-6);
        assert_eq!(parse_channel_gain("200%").unwrap(), 2.0);

        for bad in [
            "", "abc", "-0.1", "2.01", "-5%", "201%", "+6.1dB", "+6.5 dB", "+7dB", "NaN", "inf",
            "+inf", "inf dB", "NaN dB", "NaN%",
        ] {
            let err = parse_channel_gain(bad).unwrap_err().to_string();
            assert!(
                err.contains("-inf..+6.0 dB"),
                "expected clear range error for '{bad}', got: {err}"
            );
        }
    }

    #[test]
    fn parse_pan_accepts_center_lr_percent_and_decimal_and_rejects_invalid() {
        assert_eq!(parse_pan("C").unwrap(), 0.0);
        assert_eq!(parse_pan("center").unwrap(), 0.0);
        assert_eq!(parse_pan("CENTER").unwrap(), 0.0);
        assert_eq!(parse_pan("L0").unwrap(), 0.0);
        assert_eq!(parse_pan("R0%").unwrap(), 0.0);
        assert_eq!(parse_pan("0").unwrap(), 0.0);
        assert_eq!(parse_pan("-0.0").unwrap(), 0.0);
        assert!((parse_pan("L50").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("R50").unwrap() - 0.5).abs() < 1e-6);
        assert!((parse_pan("l25%").unwrap() + 0.25).abs() < 1e-6);
        assert!((parse_pan("R100%").unwrap() - 1.0).abs() < 1e-6);
        assert!((parse_pan("-0.5").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("+0.5").unwrap() - 0.5).abs() < 1e-6);
        assert!((parse_pan("-50%").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("+75%").unwrap() - 0.75).abs() < 1e-6);

        for bad in [
            "", "abc", "1.5", "-1.1", "105%", "-101%", "L150", "R120%", "L-10", "R-5", "NaN",
            "inf", "-inf", "LNaN", "Rinf",
        ] {
            assert!(
                parse_pan(bad).is_err(),
                "expected parse_pan to reject '{bad}'"
            );
        }
    }

    #[test]
    fn choice_enum_contract_covers_slug_label_aliases_clamping_and_wrapping_cycle() {
        fn assert_choice_contract<T: ChoiceEnum + std::fmt::Debug>() {
            let n = T::ALL.len();
            assert!(n > 0);
            for (i, &v) in T::ALL.iter().enumerate() {
                assert_eq!(v.to_index(), i);
                assert_eq!(T::from_index(i), v);
                assert_eq!(T::parse(v.slug()), Some(v));
                assert_eq!(T::parse(&v.slug().to_ascii_uppercase()), Some(v));
                assert_eq!(T::parse(v.label()), Some(v));
                assert_eq!(v.cycle(n as i32), v);
                assert_eq!(v.cycle(-(n as i32)), v);
                assert_eq!(v.cycle(1).cycle(-1), v);
            }
            // Clamping semantics on out-of-range index.
            assert_eq!(T::from_index(n + 99), T::ALL[n - 1]);
            // Wrapping semantics at boundaries.
            assert_eq!(T::ALL[0].cycle(-1), T::ALL[n - 1]);
            assert_eq!(T::ALL[n - 1].cycle(1), T::ALL[0]);
            assert_eq!(T::parse("___nonexistent_choice___"), None);
        }

        assert_choice_contract::<ValueMode>();
        assert_choice_contract::<crate::mixer::ChannelKind>();
        assert_choice_contract::<crate::mixer::ChannelParam>();
        assert_choice_contract::<crate::mixer::DelayMode>();
        assert_choice_contract::<crate::mixer::DriveMode>();
        assert_choice_contract::<crate::mixer::EffectKindTag>();
        assert_choice_contract::<crate::rack::Waveform>();
        assert_choice_contract::<crate::rack::AeroMode>();
        assert_choice_contract::<crate::rack::BodyGeometry>();
        assert_choice_contract::<crate::rack::EnvMode>();
        assert_choice_contract::<crate::rack::FilterMode>();
        assert_choice_contract::<crate::rack::PairTopology>();
        assert_choice_contract::<crate::rack::TriadAxis>();
        assert_choice_contract::<crate::rack::LfoShape>();
        assert_choice_contract::<crate::rack::VoiceMode>();
        assert_choice_contract::<crate::rack::PitchTracking>();
        assert_choice_contract::<crate::rack::TriggerMode>();
        assert_choice_contract::<crate::rack::NoiseColor>();
        assert_choice_contract::<crate::rack::RackTemplate>();
        assert_choice_contract::<crate::rack::InPort>();
        assert_choice_contract::<crate::rack::OutPort>();
        assert_eq!(ValueMode::Hold.toggle(), ValueMode::Ramp);
        assert_eq!(ValueMode::Ramp.toggle(), ValueMode::Hold);
        assert_eq!(serde_json::to_string(&ValueMode::Hold).unwrap(), "\"hold\"");
        assert_eq!(serde_json::to_string(&ValueMode::Ramp).unwrap(), "\"ramp\"");
        // Verify serde wire format matches slug() on every ChoiceEnum.
        for &mode in crate::rack::FilterMode::ALL {
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.slug())
            );
        }
        for &mode in &crate::mixer::DelayMode::ALL {
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.slug())
            );
        }
        for &mode in &crate::mixer::DriveMode::ALL {
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.slug())
            );
        }
    }
}
