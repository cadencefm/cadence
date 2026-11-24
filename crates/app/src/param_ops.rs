//! Single source of truth for resolving, parsing, formatting, nudging, and mapping
//! `ParamAddress` and parameter text values across controls, Ex commands, MIDI,
//! Macros, Rack zones/modules/cables, authoring drafts, and value/step editors.
use super::*;
use cadence_core::filter_patterns::FILTER_ENV_PATTERN_COUNT;
use cadence_core::rack::{module_sample_key, Unit};

/// Minimum and maximum step gate length (in 16th-note steps), matching `cadence_core::music::Step`.
pub(crate) const MIN_STEP_GATE: f32 = 0.05;
pub(crate) const MAX_STEP_GATE: f32 = 64.0;

/// Maximum addressable Filter rhythmic envelope groove bank slots (`00..=127`).
pub(crate) const FILTER_BANK_COUNT: usize = FILTER_ENV_PATTERN_COUNT;
pub(crate) const FILTER_BANK_MAX_INDEX: u8 = (FILTER_BANK_COUNT - 1) as u8;

/// Parse a finite decimal or `%`-scaled scalar (`0.5`, `50%`, `-0.5`, `-50%`).
pub(crate) fn parse_amount(raw: &str) -> Result<f32> {
    let trimmed = raw.trim();
    let value = if let Some(percent) = trimmed.strip_suffix('%') {
        percent.trim().parse::<f32>()? / 100.0
    } else {
        trimmed.parse::<f32>()?
    };
    ensure!(value.is_finite(), "Value must be finite");
    Ok(value)
}

/// Parse a normalized `0.0..=1.0` or `0%..=100%` value (used for velocities, amounts,
/// sends, macro levels, and normalized modulation curves).
pub(crate) fn parse_normalized(raw: &str) -> Result<f32> {
    let value = parse_amount(raw)?;
    ensure!(
        (0.0..=1.0).contains(&value),
        "Value must be within 0..1 or 0%..100%"
    );
    Ok(value)
}

/// Unified pan parser accepting `C`/`center`, `L50`/`R50`/`L50%`/`R50%`,
/// `-0.5`/`+0.5`/`0`, and `-50%`/`+50%` (re-exported from `cadence_core::params`).
pub(crate) use cadence_core::params::parse_pan;

/// Unified channel parameter name parser (`gain | level | trim` and `pan`).
pub(crate) fn parse_channel_param(name: &str) -> Option<ChannelParam> {
    match name.trim().to_ascii_lowercase().as_str() {
        "gain" | "level" | "trim" => Some(ChannelParam::Gain),
        "pan" => Some(ChannelParam::Pan),
        _ => None,
    }
}

/// Unified Rack instrument parameter parser delegating directly to `InstrumentParam::parse_for_rack`.
pub(crate) fn parse_instrument_param(name: &str, rack: &RackPatch) -> Option<InstrumentParam> {
    InstrumentParam::parse_for_rack(name, rack, true)
}

impl App {
    /// Total decoded sample frames for a Sampler module `(instrument, zone_id, module_id)`,
    /// or `0` if none/unloaded.
    pub(crate) fn module_sample_frames(
        &self,
        instrument: Id,
        zone_id: u16,
        module_id: u16,
    ) -> usize {
        self.instrument_sample(module_sample_key(instrument, zone_id, module_id))
            .map_or(0, |s| s.frames)
    }

    fn total_frames_for_instrument_param(&self, instrument: Id, target: InstrumentParam) -> usize {
        match target {
            InstrumentParam::Module { zone, module, .. } => {
                self.module_sample_frames(instrument, zone, module)
            }
            _ => 0,
        }
    }

    /// Parse a globally-qualified dotted Rack parameter such as
    /// `LEAD.Filter.cutoff`, `KIT.BD.gain`, `KIT.z1.VCO.tune`, or `LEAD.c5`.
    pub(crate) fn parse_dotted_zone_param(
        &self,
        raw: &str,
        require_automatable: bool,
    ) -> Option<ParamAddress> {
        let trimmed = raw.trim();
        let (inst_tok, rest) = trimmed
            .split_once('.')
            .or_else(|| trimmed.split_once(':'))?;
        let inst_id = self.instrument_id(inst_tok).ok()?;
        let inst = self.instrument(inst_id)?;
        let param = InstrumentParam::parse_for_rack(rest, &inst.rack, require_automatable)?;
        Some(ParamAddress::from_instrument(inst_id, param))
    }

    /// Whether any Part per-note `parameter_link` targets `address`.
    pub(crate) fn param_has_part_link(&self, address: ParamAddress) -> bool {
        let Some((inst_id, inst_param)) = address.as_instrument() else {
            return false;
        };
        self.doc.project().parts.iter().any(|part| {
            part.instrument == inst_id
                && part.parameter_links.iter().any(|l| l.target == inst_param)
        })
    }

    /// Check whether `address` is bound to any Macro, driven by a direct Modulation,
    /// or linked by any Part per-note `parameter_link`.
    pub(crate) fn param_is_controlled(&self, address: ParamAddress) -> bool {
        self.param_is_macro_bound(address)
            || self.param_has_part_link(address)
            || self
                .param_address_to_mod_target(address)
                .is_some_and(|target| {
                    self.doc
                        .project()
                        .modulations
                        .iter()
                        .any(|m| m.target == target)
                })
    }

    /// Resolve an effect device by numeric ID (`#7` / `7`) OR case-insensitive name.
    pub(crate) fn effect_id(&self, raw: &str) -> Result<Id> {
        if let Ok(id) = raw.trim().trim_start_matches('#').parse::<Id>() {
            if self.effect(id).is_some() {
                return Ok(id);
            }
        }
        let all: Vec<(Id, &str)> = self
            .doc
            .project()
            .mixer
            .channels
            .iter()
            .flat_map(|c| c.inserts.iter())
            .chain(self.doc.project().mixer.master.inserts.iter())
            .map(|d| (d.id, d.name.as_str()))
            .collect();
        lookup(raw, all.into_iter(), "effect device")
    }

    /// Read any `ParamAddress` in normalized `0.0..=1.0` space.
    pub fn read_param_normalized(&self, address: ParamAddress) -> Option<f32> {
        if let Some((instrument, ip)) = address.as_instrument() {
            let inst = self.instrument(instrument)?;
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            return inst.rack.read_param_normalized(ip, frames);
        }
        match address {
            ParamAddress::Effect { device, param } => self.effect(device)?.1.read_normalized(param),
            ParamAddress::Channel { channel, param } => {
                let strip = self.strip(channel)?;
                match param {
                    ChannelParam::Gain => Some(param.normalize(strip.gain)),
                    ChannelParam::Pan => Some(param.normalize(strip.pan)),
                }
            }
            ParamAddress::Send { channel, target } => match channel {
                ChannelRef::Channel(id) => {
                    let ch = self.channel(id)?;
                    if ch.kind == ChannelKind::Return {
                        return None;
                    }
                    if let Some(send) = ch.sends.iter().find(|s| s.target == target) {
                        return Some(send.amount.clamp(0.0, 1.0));
                    }
                    self.channel(target)
                        .is_some_and(|dst| dst.kind == ChannelKind::Return)
                        .then_some(0.0)
                }
                ChannelRef::Master => None,
            },
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Format an effect parameter's native value using this session's loaded `FilterGrooveCatalog`.
    pub(crate) fn format_effect_param_native(&self, param: EffectParam, value: f32) -> String {
        if param == EffectParam::EnvPattern {
            let id = value.round().clamp(0.0, f32::from(FILTER_BANK_MAX_INDEX)) as u8;
            return self.assets.filter_grooves.preset(id).name;
        }
        param.format_native(value)
    }

    /// Format a normalized `0.0..=1.0` value in native display units for any `ParamAddress`.
    pub(crate) fn format_param_normalized_value(&self, address: ParamAddress, norm: f32) -> String {
        let n = norm.clamp(0.0, 1.0);
        if let Some((instrument, ip)) = address.as_instrument() {
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            if let Some(inst) = self.instrument(instrument) {
                return inst.rack.format_param_normalized(ip, n, frames);
            }
            return match ip {
                InstrumentParam::Rack { param } => param.format_normalized(n),
                InstrumentParam::Zone { param, .. } => param.format_normalized(n),
                InstrumentParam::Module { param, .. } => param.format_normalized(n, frames),
                InstrumentParam::Cable { .. } => format!("{:+.0}%", (-2.0 + n * 4.0) * 100.0),
            };
        }
        match address {
            ParamAddress::Effect { param, .. } => {
                self.format_effect_param_native(param, param.denormalize(n))
            }
            ParamAddress::Channel { param, .. } => match param {
                ChannelParam::Gain => cadence_core::dsp::format_gain_db(param.denormalize(n)),
                ChannelParam::Pan => cadence_core::params::format_pan(param.denormalize(n)),
            },
            ParamAddress::Send { .. } => format!("{:.0}%", n * 100.0),
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Convert a normalized `0.0..=1.0` value into `(native_value, short_name, formatted_value)`
    /// for any `ParamAddress`.
    pub(crate) fn resolve_param_normalized_display(
        &self,
        address: ParamAddress,
        norm: f32,
    ) -> Result<(f32, String, String)> {
        let norm = norm.clamp(0.0, 1.0);
        if let Some((instrument, ip)) = address.as_instrument() {
            let inst = self.instrument(instrument).context("Instrument missing")?;
            let mut rack = inst.rack.clone();
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            ensure!(
                rack.write_param_normalized(ip, norm, frames),
                "Instrument '{}' does not contain parameter target",
                inst.name
            );
            let native = rack
                .read_param_native(ip)
                .context("Rack parameter missing after write")?;
            let val_str = rack
                .format_param(ip, frames)
                .unwrap_or_else(|| rack.format_param_normalized(ip, norm, frames));
            let (_, short) = self.format_macro_target(address);
            return Ok((native, short, val_str));
        }
        let result = match address {
            ParamAddress::Effect { device, param } => {
                let (_, dev) = self.effect(device).context("Effect missing")?;
                ensure!(dev.supports_param(param), "Effect parameter unsupported");
                let native = param.denormalize(norm);
                let val_str = self.format_effect_param_native(param, native);
                (native, format!("{} {}", dev.name, param.label()), val_str)
            }
            ParamAddress::Channel { channel, param } => {
                let ch_name = self.channel_name(channel).to_string();
                let native = param.denormalize(norm);
                let val_str = match param {
                    ChannelParam::Gain => cadence_core::dsp::format_gain_db(native),
                    ChannelParam::Pan => cadence_core::params::format_pan(native),
                };
                (native, format!("{ch_name} {}", param.label()), val_str)
            }
            ParamAddress::Send { channel, target } => {
                let ch_name = self.channel_name(channel).to_string();
                let dst_name = self.channel_name(ChannelRef::Channel(target)).to_string();
                (
                    norm,
                    format!("{ch_name} -> {dst_name}"),
                    format!("{:.0}%", norm * 100.0),
                )
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        };
        Ok(result)
    }

    /// Resolve `(target_kind, human_readable_label)` for any `ParamAddress`.
    pub(crate) fn format_macro_target(
        &self,
        address: ParamAddress,
    ) -> (view::MacroTargetKind, String) {
        use view::MacroTargetKind;
        match address {
            ParamAddress::Rack { instrument, param } => {
                let name = self
                    .instrument(instrument)
                    .map_or_else(|| format!("Rack #{instrument}"), |i| i.name.clone());
                (MacroTargetKind::Rack, format!("{name} · {}", param.label()))
            }
            ParamAddress::Zone {
                instrument,
                zone,
                param,
            } => {
                let inst = self.instrument(instrument);
                let inst_name =
                    inst.map_or_else(|| format!("Rack #{instrument}"), |i| i.name.clone());
                let z_name = inst
                    .and_then(|i| i.rack.zone(zone))
                    .map_or_else(|| format!("z{zone}"), |z| z.name.clone());
                (
                    MacroTargetKind::Zone,
                    format!("{inst_name}.{z_name} · {}", param.label()),
                )
            }
            ParamAddress::Module {
                instrument,
                zone,
                module,
                param,
            } => {
                let inst = self.instrument(instrument);
                let inst_name =
                    inst.map_or_else(|| format!("Rack #{instrument}"), |i| i.name.clone());
                let z = inst.and_then(|i| i.rack.zone(zone));
                let m_name = z
                    .and_then(|z| z.module(module))
                    .map_or_else(|| format!("m{module}"), |m| m.name.clone());
                let multi_zone = inst.is_some_and(|i| i.rack.zones.len() > 1);
                if multi_zone {
                    let z_name = z.map_or_else(|| format!("z{zone}"), |z| z.name.clone());
                    (
                        MacroTargetKind::Module,
                        format!("{inst_name}.{z_name}.{m_name} · {}", param.label()),
                    )
                } else {
                    (
                        MacroTargetKind::Module,
                        format!("{inst_name} · {m_name} {}", param.label()),
                    )
                }
            }
            ParamAddress::Cable {
                instrument,
                zone,
                cable,
            } => {
                let inst = self.instrument(instrument);
                let inst_name =
                    inst.map_or_else(|| format!("Rack #{instrument}"), |i| i.name.clone());
                let detail = inst
                    .and_then(|i| i.rack.zone(zone))
                    .and_then(|z| {
                        let c = z.cable(cable)?;
                        let from = z
                            .module(c.from_module)
                            .map_or_else(|| format!("m{}", c.from_module), |m| m.name.clone());
                        let to = z
                            .module(c.to_module)
                            .map_or_else(|| format!("m{}", c.to_module), |m| m.name.clone());
                        Some(format!("{from}→{to}.{}", c.to_port.slug()))
                    })
                    .unwrap_or_else(|| format!("c{cable}"));
                (MacroTargetKind::Cable, format!("{inst_name} · {detail}"))
            }
            ParamAddress::Effect { device, param } => {
                let name = self
                    .effect(device)
                    .map_or_else(|| format!("FX #{device}"), |(_, d)| d.name.clone());
                (
                    MacroTargetKind::Effect,
                    format!("{name} · {}", param.label()),
                )
            }
            ParamAddress::Channel { channel, param } => (
                MacroTargetKind::Channel,
                format!("{} · {}", self.channel_name(channel), param.label()),
            ),
            ParamAddress::Send { channel, target } => (
                MacroTargetKind::Send,
                format!(
                    "{} → {}",
                    self.channel_name(channel),
                    self.channel_name(ChannelRef::Channel(target))
                ),
            ),
        }
    }

    /// Human-readable status description after setting `address` to native `value`.
    pub(crate) fn describe_param(&self, address: ParamAddress, value: f32) -> String {
        if let Some((instrument, ip)) = address.as_instrument() {
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            if let Some(inst) = self.instrument(instrument) {
                let mut rack = inst.rack.clone();
                let _ = rack.write_param_native(ip, value);
                let formatted = rack
                    .format_param(ip, frames)
                    .unwrap_or_else(|| format!("{value:.2}"));
                let (_, label) = self.format_macro_target(address);
                return format!("{label} → {formatted}");
            }
            return format!("{ip:?} → {value:.2}");
        }
        match address {
            ParamAddress::Channel { channel, param } => {
                let formatted = match param {
                    ChannelParam::Gain => cadence_core::dsp::format_gain_db(value),
                    ChannelParam::Pan => cadence_core::params::format_pan(value),
                };
                format!(
                    "{} {} → {formatted}",
                    self.channel_name(channel),
                    param.label()
                )
            }
            ParamAddress::Send { channel, target } => format!(
                "{} send → {} {:.0}%",
                self.channel_name(channel),
                self.channel_name(ChannelRef::Channel(target)),
                value.clamp(0.0, 1.0) * 100.0
            ),
            ParamAddress::Effect { param, .. } => {
                format!(
                    "{} → {}",
                    param.label(),
                    self.format_effect_param_native(param, value)
                )
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Map a continuous `ParamAddress` to its automatable `ModulationTarget`, if supported.
    pub(crate) fn param_address_to_mod_target(
        &self,
        address: ParamAddress,
    ) -> Option<ModulationTarget> {
        if let Some((instrument, ip)) = address.as_instrument() {
            let supported = self
                .instrument(instrument)
                .is_some_and(|i| i.rack.supports_automatable_param(ip));
            return supported.then_some(ModulationTarget::Instrument {
                instrument,
                parameter: ip,
            });
        }
        match address {
            ParamAddress::Effect { device, param } => self
                .effect(device)
                .is_some_and(|(_, d)| d.read_normalized(param).is_some())
                .then_some(ModulationTarget::Effect {
                    device,
                    parameter: param,
                }),
            ParamAddress::Channel {
                channel,
                param: ChannelParam::Gain,
            } => Some(ModulationTarget::ChannelGain { channel }),
            ParamAddress::Channel {
                channel,
                param: ChannelParam::Pan,
            } => Some(ModulationTarget::ChannelPan { channel }),
            ParamAddress::Send { channel, target } => {
                let src_ok = matches!(
                    channel,
                    ChannelRef::Channel(id)
                        if self.channel(id).is_some_and(|c| c.kind != ChannelKind::Return)
                );
                let dst_ok = self
                    .channel(target)
                    .is_some_and(|c| c.kind == ChannelKind::Return);
                (src_ok && dst_ok).then_some(ModulationTarget::Send { channel, target })
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Parse native text input for any `ParamAddress`, honoring unified pan, cable, and `%` forms.
    pub(crate) fn parse_param_native(&self, address: ParamAddress, raw: &str) -> Result<f32> {
        if let Some((instrument, ip)) = address.as_instrument() {
            if let Some(inst) = self.instrument(instrument) {
                return inst.rack.parse_param_value_native(ip, raw);
            }
            return match ip {
                InstrumentParam::Rack { param } => param.parse_value_native(raw),
                InstrumentParam::Zone { param, .. } => param.parse_value_native(raw),
                InstrumentParam::Module { param, .. } => param.parse_value_native(raw),
                InstrumentParam::Cable { .. } => bail!("Instrument #{instrument} not found"),
            };
        }
        match address {
            ParamAddress::Effect {
                param: EffectParam::EnvPattern,
                ..
            } => {
                let trimmed = raw.trim();
                if let Ok(n) = trimmed.parse::<u8>() {
                    ensure!(
                        usize::from(n) < FILTER_BANK_COUNT,
                        "Pattern must be within 0..{FILTER_BANK_MAX_INDEX}"
                    );
                    Ok(f32::from(n))
                } else if let Some(id) = self.assets.filter_grooves.find_by_name(trimmed) {
                    Ok(f32::from(id))
                } else {
                    bail!("Filter pattern expects 0..={FILTER_BANK_MAX_INDEX} or a groove name")
                }
            }
            ParamAddress::Effect { param, .. } => param.parse_value_native(raw),
            ParamAddress::Channel {
                param: ChannelParam::Gain,
                ..
            } => cadence_core::params::parse_channel_gain(raw),
            ParamAddress::Channel {
                param: ChannelParam::Pan,
                ..
            } => parse_pan(raw),
            ParamAddress::Send { .. } => parse_normalized(raw),
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Human-readable accepted units hint for unified `Mode::ValueInput` exact entry.
    pub(crate) fn param_units_hint(&self, address: ParamAddress) -> &'static str {
        match address {
            ParamAddress::Rack { param, .. } => match param {
                RackParam::Gain => "dB (-24dB..+12dB)",
                RackParam::Pan => "L50 / C / R50, -1..+1, or -100%..+100%",
                RackParam::Tune => "semitones (-24..+24)",
                RackParam::Cutoff => "Hz or kHz (e.g. 1200hz, 1.8khz)",
                RackParam::Resonance => "0..0.92 or 0%..92%",
            },
            ParamAddress::Zone { param, .. } => match param {
                ZoneParam::Gain => "dB (-36dB..+12dB)",
                ZoneParam::Pan => "L50 / C / R50, -1..+1, or -100%..+100%",
                ZoneParam::Glide => "ms or s (e.g. 40ms, 0.2s)",
                ZoneParam::RootMidi => "MIDI 0–127 or note name (e.g. C3, F#4)",
                ZoneParam::VoiceMode
                | ZoneParam::Legato
                | ZoneParam::PitchTracking
                | ZoneParam::PhaseReset => "named mode or on/off",
            },
            ParamAddress::Module { param, .. } => match param.unit() {
                Unit::Hz => "Hz or kHz (e.g. 1200hz, 1.8khz)",
                Unit::Ms => "ms or s (e.g. 25ms, 0.4s)",
                Unit::Db => "dB (e.g. -6db, +3db, 0db)",
                Unit::Pan => "L50 / C / R50, -1..+1, or -100%..+100%",
                Unit::Semitones => "semitones (e.g. -12, +7)",
                Unit::Cents => "cents (e.g. -12, +7.5)",
                Unit::Degrees => "degrees (0..360)",
                Unit::Octaves => "octaves (e.g. +2.0)",
                Unit::Note => "MIDI 0–127 or note name (e.g. C3, F#4)",
                Unit::Bits => "bits (2..24)",
                Unit::Percent => "0..1 or 0%..100%",
                Unit::Plain => {
                    if param.is_discrete() {
                        "named mode or index"
                    } else {
                        "numeric value or % (e.g. 0.5, 50%)"
                    }
                }
            },
            ParamAddress::Cable {
                instrument,
                zone,
                cable,
            } => {
                if let Some(port) = self
                    .instrument(instrument)
                    .and_then(|i| i.rack.zone(zone))
                    .and_then(|z| z.cable(cable))
                    .map(|c| c.to_port)
                {
                    match port {
                        InPort::PitchCv => "semitones (e.g. +12st, -24st, 0)",
                        InPort::In => "0..2 or 0%..200% (e.g. 1.0, 100%)",
                        InPort::Cv => "attenuverter (-200%..+200% or octaves)",
                        InPort::FmIn | InPort::PwCv | InPort::TrigIn => {
                            "depth (-100%..+100% or 0..1)"
                        }
                    }
                } else {
                    "cable attenuverter amount (e.g. 100%, +50%, -0.5)"
                }
            }
            ParamAddress::Effect { param, .. } => match param.unit() {
                cadence_core::mixer::ParameterUnit::SixteenthSteps => {
                    "16th-note steps or fractions (e.g. 3, 1.5, 3/16, 1/8)"
                }
                cadence_core::mixer::ParameterUnit::Ratio => "0..1 or 0%..100% (e.g. 0.5, 50%)",
                cadence_core::mixer::ParameterUnit::BipolarRatio => {
                    "-1..+1 or -100%..+100% (e.g. +25%, -0.5, sym)"
                }
                cadence_core::mixer::ParameterUnit::Seconds => "s or ms (e.g. 2.5s, 400ms)",
                cadence_core::mixer::ParameterUnit::Milliseconds => "ms or s (e.g. 25ms, 0.2s)",
                cadence_core::mixer::ParameterUnit::Hertz => "Hz or kHz (e.g. 1200hz, 1.8khz)",
                cadence_core::mixer::ParameterUnit::Decibels => "dB (e.g. -6db, +3db, 0db)",
                cadence_core::mixer::ParameterUnit::FilterMode => {
                    "lp / bp / hp / acid / lp6 (or index)"
                }
                cadence_core::mixer::ParameterUnit::DelayMode => "ping_pong / stereo (or index)",
                cadence_core::mixer::ParameterUnit::DriveMode => {
                    "tape / hard_clip / fold (or index)"
                }
                cadence_core::mixer::ParameterUnit::CompressionRatio => "ratio (e.g. 4, 8:1)",
                cadence_core::mixer::ParameterUnit::Octaves => "octaves (e.g. +2.5, -1.5, off)",
                cadence_core::mixer::ParameterUnit::FilterPattern => {
                    "groove ID 0–127 or groove name"
                }
                cadence_core::mixer::ParameterUnit::QFactor => "Q factor (e.g. 0.7, 2.5, 6.0)",
            },
            ParamAddress::Channel {
                param: ChannelParam::Gain,
                ..
            } => "dB (-60dB..+12dB) or % (0%..200%)",
            ParamAddress::Channel {
                param: ChannelParam::Pan,
                ..
            } => "L50 / C / R50, -1..+1, or -100%..+100%",
            ParamAddress::Send { .. } => "0..1 or 0%..100%",
        }
    }

    /// Title, subtitle, and current display readout for unified `Mode::ValueInput` on `address`.
    pub(crate) fn param_value_input_meta(&self, address: ParamAddress) -> (String, String, String) {
        let (group, full_label) = self.format_macro_target(address);
        let current = if let Some((instrument, ip)) = address.as_instrument() {
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            self.instrument(instrument)
                .and_then(|i| i.rack.format_param(ip, frames))
                .unwrap_or_else(|| "—".into())
        } else {
            self.read_param_normalized(address)
                .map(|n| self.format_param_normalized_value(address, n))
                .unwrap_or_else(|| "—".into())
        };
        (full_label, group.badge().into(), current)
    }

    /// Resolve a typed target triple `(kind, target, param)` (used by `:macro bind` and similar).
    pub(crate) fn parse_param_address(
        &self,
        kind: &str,
        target: &str,
        param: &str,
    ) -> Result<ParamAddress> {
        match kind.trim().to_ascii_lowercase().as_str() {
            "instrument" | "inst" | "rack" => {
                let inst_id = self.instrument_id(target)?;
                let inst = self.instrument(inst_id).context("Instrument missing")?;
                let parsed = parse_instrument_param(param, &inst.rack)
                    .context("Unknown Rack parameter")?;
                Ok(ParamAddress::from_instrument(inst_id, parsed))
            }
            "zone" | "module" | "mod" | "cable" => self
                .parse_dotted_zone_param(&format!("{target}.{param}"), true)
                .context(
                    "Target must be INST.ZONE PARAM, INST.MODULE PARAM, or INST.ZONE.MODULE PARAM",
                ),
            "effect" | "fx" => {
                let device = self.effect_id(target)?;
                let (_, dev) = self.effect(device).context("Effect missing")?;
                let p = dev.parse_param(param).context("Unknown effect parameter")?;
                Ok(ParamAddress::Effect { device, param: p })
            }
            "channel" | "ch" => {
                let channel = self.channel_ref(target)?;
                let p_lower = param.trim().to_ascii_lowercase();
                if let Some(ret_raw) = p_lower.strip_prefix("send:") {
                    return Ok(ParamAddress::Send {
                        channel,
                        target: self.channel_id(ret_raw)?,
                    });
                }
                let p = parse_channel_param(param)
                    .context("Channel parameter must be gain, level, trim, pan, or send:<RETURN>")?;
                Ok(ParamAddress::Channel { channel, param: p })
            }
            "send" => {
                let channel = self.channel_ref(target)?;
                let dst = self.channel_id(param)?;
                Ok(ParamAddress::Send {
                    channel,
                    target: dst,
                })
            }
            other => bail!(
                "Unknown parameter target '{other}'; use instrument, rack, zone, module, cable, effect, channel, or send"
            ),
        }
    }

    /// Nudge any `ParamAddress` by one fine/coarse step and commit cleanly.
    pub(crate) fn nudge_param(
        &mut self,
        address: ParamAddress,
        direction: i8,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        if let Some((instrument, ip)) = address.as_instrument() {
            let mut rack = self
                .instrument(instrument)
                .context("Instrument missing")?
                .rack
                .clone();
            let frames = self.total_frames_for_instrument_param(instrument, ip);
            let next = rack.nudge_param(ip, direction, coarse, frames)?;
            // `ModuleParam::LoopMode` updates `loop_end` / `one_shot` atomically alongside loop mode
            // inside `SamplerModule::nudge_param`; commit the full Rack update when toggling LoopMode.
            if matches!(
                ip,
                InstrumentParam::Module {
                    param: ModuleParam::LoopMode,
                    ..
                }
            ) {
                let formatted = rack
                    .format_param(ip, frames)
                    .unwrap_or_else(|| "Off".into());
                let (_, target_label) = self.format_macro_target(address);
                let label = format!("{target_label} → {formatted}");
                return self.commit_coalesced(
                    vec![Edit::UpdateRackPatch {
                        id: instrument,
                        rack,
                    }],
                    label,
                    document::CoalesceKey::Param(address),
                    coalesce,
                );
            }
            let label = self.describe_param(address, next);
            return self.commit_param(address, next, label, coalesce);
        }
        match address {
            ParamAddress::Effect { device, param } => {
                let dev = self.effect(device).context("Effect missing")?.1;
                let next = if param == EffectParam::EnvPattern {
                    let cur_id = match dev.kind {
                        cadence_core::mixer::EffectKind::Filter(p) => p.env_pattern,
                        _ => bail!("Effect does not support EnvPattern"),
                    };
                    let step = isize::from(direction) * if coarse { 5 } else { 1 };
                    f32::from(self.assets.filter_grooves.step_id(cur_id, step))
                } else {
                    dev.nudge_native(param, direction, coarse)?
                };
                if param == EffectParam::EnvPattern
                    && matches!(
                        dev.kind,
                        cadence_core::mixer::EffectKind::Filter(p) if p.env_pattern_id.is_some()
                    )
                {
                    let edit = self.effect_edit(device, |d| {
                        if let cadence_core::mixer::EffectKind::Filter(fp) = &mut d.kind {
                            fp.env_pattern_id = None;
                            fp.env_pattern =
                                next.round().clamp(0.0, f32::from(FILTER_BANK_MAX_INDEX)) as u8;
                        }
                        Ok(())
                    })?;
                    let out = self.apply(Action::Edit { edit })?;
                    self.status = self.describe_param(address, next);
                    return Ok(out);
                }
                let label = self.describe_param(address, next);
                self.commit_param(address, next, label, coalesce)
            }
            ParamAddress::Channel { channel, param } => {
                let strip = self.strip(channel).context("Channel missing")?;
                let next = match param {
                    ChannelParam::Gain => {
                        let delta_db = f32::from(direction) * if coarse { 3.0 } else { 1.0 };
                        cadence_core::dsp::nudge_channel_gain_db(strip.gain, delta_db)
                    }
                    ChannelParam::Pan => {
                        let step_pct = if coarse { 10.0 } else { 1.0 };
                        (((strip.pan * 100.0).round() + f32::from(direction) * step_pct) / 100.0)
                            .clamp(-1.0, 1.0)
                    }
                };
                let label = self.describe_param(address, next);
                self.commit_param(address, next, label, coalesce)
            }
            ParamAddress::Send { .. } => {
                let step_pct = if coarse { 5.0 } else { 1.0 };
                let cur = self.read_param_normalized(address).unwrap_or(0.0);
                let next = (((cur * 100.0).round() + f32::from(direction) * step_pct) / 100.0)
                    .clamp(0.0, 1.0);
                let label = self.describe_param(address, next);
                self.commit_param(address, next, label, coalesce)
            }
            ParamAddress::Rack { .. }
            | ParamAddress::Zone { .. }
            | ParamAddress::Module { .. }
            | ParamAddress::Cable { .. } => unreachable!(),
        }
    }

    /// Authoritative default native value for any `ParamAddress`, derived directly from `cadence-core`.
    pub(crate) fn default_param_native(&self, address: ParamAddress) -> Option<f32> {
        match address {
            ParamAddress::Rack { param, .. } => {
                Some(param.read_native(&RackPatch::default_synth()))
            }
            ParamAddress::Zone { param, .. } => RackPatch::default_synth()
                .zones
                .first()
                .map(|z| param.read_native(z)),
            ParamAddress::Module {
                instrument,
                zone,
                module,
                param,
            } => self
                .instrument(instrument)?
                .rack
                .zone(zone)?
                .module(module)?
                .kind
                .tag()
                .default_kind()
                .read_native(param),
            ParamAddress::Cable {
                instrument,
                zone,
                cable,
            } => {
                let z = self.instrument(instrument)?.rack.zone(zone)?;
                let c = z.cable(cable)?;
                let to_kind = z.module(c.to_module)?.kind.tag();
                Some(cadence_core::rack::cable_default_amount(to_kind, c.to_port))
            }
            ParamAddress::Effect { device, param } => {
                let def = self.effect(device)?.1.kind.tag().default_device(0);
                def.read_normalized(param).map(|n| param.denormalize(n))
            }
            ParamAddress::Channel { param, .. } => {
                let def = cadence_core::mixer::ChannelStrip::default();
                Some(match param {
                    ChannelParam::Gain => def.gain,
                    ChannelParam::Pan => def.pan,
                })
            }
            ParamAddress::Send { .. } => Some(0.0),
        }
    }

    /// Reset any `ParamAddress` back to its authoritative default value (`Alt+Backspace`).
    pub(crate) fn reset_param_default(&mut self, address: ParamAddress) -> Result<Outcome> {
        let Some(value) = self.default_param_native(address) else {
            return Ok(Outcome::default());
        };
        let label = format!("Reset {}", self.describe_param(address, value));
        self.commit_param(address, value, label, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unified_pan_and_normalized_parsers_accept_all_documented_forms() {
        assert_eq!(parse_pan("C").unwrap(), 0.0);
        assert_eq!(parse_pan("center").unwrap(), 0.0);
        assert!((parse_pan("L50").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("R50").unwrap() - 0.5).abs() < 1e-6);
        assert!((parse_pan("l25%").unwrap() + 0.25).abs() < 1e-6);
        assert!((parse_pan("-0.5").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("-50%").unwrap() + 0.5).abs() < 1e-6);
        assert!((parse_pan("+75%").unwrap() - 0.75).abs() < 1e-6);
        assert!(parse_pan("1.5").is_err());
        assert!(parse_pan("L150").is_err());

        assert!((parse_normalized("0.5").unwrap() - 0.5).abs() < 1e-6);
        assert!((parse_normalized("85%").unwrap() - 0.85).abs() < 1e-6);
        assert!(parse_normalized("1.5").is_err());
        assert!(parse_normalized("-0.1").is_err());

        assert_eq!(parse_channel_param("gain"), Some(ChannelParam::Gain));
        assert_eq!(parse_channel_param("level"), Some(ChannelParam::Gain));
        assert_eq!(parse_channel_param("trim"), Some(ChannelParam::Gain));
        assert_eq!(parse_channel_param("pan"), Some(ChannelParam::Pan));
        assert_eq!(parse_channel_param("other"), None);
    }

    #[test]
    fn parse_param_native_delegates_pan_uniformly_across_rack_zone_module_and_channel() {
        let app = App::new(Project::demo(), "pan-ops.json".into()).unwrap();
        let inst = &app.doc.project().instruments[0];
        let inst_id = inst.id;
        let zone0 = &inst.rack.zones[0];
        let out_id = zone0.out_module().unwrap().id;
        let ch_id = app.doc.project().mixer.channels[0].id;

        for addr in [
            ParamAddress::Rack {
                instrument: inst_id,
                param: RackParam::Pan,
            },
            ParamAddress::Zone {
                instrument: inst_id,
                zone: zone0.id,
                param: ZoneParam::Pan,
            },
            ParamAddress::Module {
                instrument: inst_id,
                zone: zone0.id,
                module: out_id,
                param: ModuleParam::Pan,
            },
            ParamAddress::Channel {
                channel: ChannelRef::Channel(ch_id),
                param: ChannelParam::Pan,
            },
        ] {
            assert_eq!(app.parse_param_native(addr, "C").unwrap(), 0.0);
            assert!((app.parse_param_native(addr, "L50").unwrap() + 0.5).abs() < 1e-6);
            assert!((app.parse_param_native(addr, "R25%").unwrap() - 0.25).abs() < 1e-6);
            assert!((app.parse_param_native(addr, "-0.5").unwrap() + 0.5).abs() < 1e-6);
        }

        let rack_pan = ParamAddress::Rack {
            instrument: inst_id,
            param: RackParam::Pan,
        };
        assert_eq!(app.format_param_normalized_value(rack_pan, 0.5), "Center");
        assert_eq!(app.format_param_normalized_value(rack_pan, 0.25), "L50");
        assert_eq!(app.format_param_normalized_value(rack_pan, 0.75), "R50");
    }

    #[test]
    fn effect_param_units_hint_matches_delay_sixteenth_steps_and_unmaterialized_sends() {
        let mut app = App::new(Project::demo(), "units-hint.json".into()).unwrap();
        app.execute_command("fx add delay master").unwrap();
        let delay_id = app.doc.project().mixer.master.inserts.last().unwrap().id;
        let delay_time = ParamAddress::Effect {
            device: delay_id,
            param: EffectParam::Time,
        };
        let hint = app.param_units_hint(delay_time);
        assert!(
            hint.contains("16th-note steps") && hint.contains("3/16") && !hint.contains("ms or s"),
            "Delay Time hint must describe sixteenth-note steps/fractions, got: {hint}"
        );
        for valid in ["3", "1.5", "3/16", "1/8"] {
            assert!(
                app.parse_param_native(delay_time, valid).is_ok(),
                "Delay Time must accept '{valid}'"
            );
        }
        assert!(
            app.parse_param_native(delay_time, "25ms").is_err(),
            "Delay Time must reject millisecond literals"
        );

        // Verify valid unmaterialized send reads 0.0 (not None), while Return/Master sources return None.
        app.execute_command("channel return FreshVerb").unwrap();
        let ret_id = app
            .doc
            .project()
            .mixer
            .channels
            .iter()
            .find(|c| c.name == "FreshVerb")
            .unwrap()
            .id;
        let audio_id = app.doc.project().mixer.channels[0].id;
        assert_eq!(
            app.read_param_normalized(ParamAddress::Send {
                channel: ChannelRef::Channel(audio_id),
                target: ret_id,
            }),
            Some(0.0)
        );
        assert_eq!(
            app.read_param_normalized(ParamAddress::Send {
                channel: ChannelRef::Channel(ret_id),
                target: ret_id,
            }),
            None
        );
        assert_eq!(
            app.read_param_normalized(ParamAddress::Send {
                channel: ChannelRef::Master,
                target: ret_id,
            }),
            None
        );
    }
}
