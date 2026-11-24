//! Session-only unified scalar entry (`Mode::ValueInput`) and authored-point navigation.
//!
//! Handles exact value entry across Pattern Values, Instrument / Effect / Channel / Send / Zone
//! `ParamAddress` controls, Rack structural zone fields, Macro dials, Placement offsets,
//! and Song Properties (BPM, Key Root, Length) with live keystroke parse validation.
use super::*;
use crate::param_ops::parse_normalized;
use cadence_core::material::MAX_BARS;
use cadence_core::music::{note_name, parse_root_midi};

const MAX_COUNT: usize = 1_000_000;

fn parse_late_tolerance_steps(trimmed: &str) -> Result<u32> {
    let steps: u32 = trimmed
        .trim_end_matches("steps")
        .trim_end_matches("step")
        .trim()
        .parse()
        .with_context(|| {
            format!(
                "Enter integer steps 0..{}",
                cadence_core::material::MAX_LATE_TOLERANCE
            )
        })?;
    ensure!(
        steps <= cadence_core::material::MAX_LATE_TOLERANCE,
        "Late tolerance must be 0..{}",
        cadence_core::material::MAX_LATE_TOLERANCE
    );
    Ok(steps)
}

#[derive(Clone, Debug)]
pub struct ValueCursorView {
    pub pattern: Id,
    pub step: usize,
    pub offset: usize,
    pub unit: ValueUnit,
    pub mode: ValueMode,
    pub value: f32,
    pub authored: bool,
    pub point_index: Option<usize>,
    pub point_count: usize,
    pub held_from: usize,
}

#[derive(Clone, Debug)]
pub struct ValueInputView {
    pub title: String,
    pub subtitle: String,
    pub units_hint: String,
    pub current_display: String,
    pub input: String,
    pub live_preview: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum ValueInputTarget {
    PatternValue {
        pattern: Id,
        step: usize,
        unit: ValueUnit,
        current_value: f32,
    },
    Param {
        address: ParamAddress,
        title: String,
        subtitle: String,
        current_display: String,
        units_hint: &'static str,
    },
    ZoneField {
        instrument: Id,
        field: ZoneControl,
        title: String,
        subtitle: String,
        current_display: String,
        units_hint: &'static str,
    },
    Macro {
        index: u8,
        title: String,
        current_display: String,
    },
    DuckDepth {
        duck: Id,
        title: String,
        current_display: String,
    },
    DuckRecovery {
        duck: Id,
        title: String,
        current_display: String,
    },
    PlacementStart {
        placement: Id,
        title: String,
        current_start: u32,
    },
    PlacementLength {
        placement: Id,
        title: String,
        current_length: u32,
    },
    PlacementOffset {
        placement: Id,
        title: String,
        current_offset: i32,
    },
    PartTranspose {
        part: Id,
        title: String,
        current_transpose: i16,
    },
    PartVelocity {
        part: Id,
        title: String,
        current_velocity: f32,
    },
    CueBar {
        current_bar: usize,
        max_bars: usize,
    },
    AudioLatencyTrim {
        current_ms: f32,
    },
    SongBpm {
        current_bpm: f64,
    },
    SongKeyRoot {
        current_root: u8,
    },
    SongBars {
        current_bars: usize,
    },
    SettingsDefaultBpm {
        current_bpm: f64,
    },
    SettingsDefaultRoot {
        current_root: u8,
    },
    SettingsLateTolerance {
        current_steps: u32,
    },
    SaveFilterGroove {
        device: Id,
        compiled: cadence_core::filter_patterns::CompiledFilterPattern,
        suggested_id: u8,
        source_summary: String,
    },
    RackConnectCable {
        instrument: Id,
        zone: u16,
        from_module: u16,
        to_module: u16,
        to_port: InPort,
        to_kind: ModuleKindTag,
        default_amount: f32,
        title: String,
        subtitle: String,
        current_display: String,
        units_hint: &'static str,
    },
    RackRenameModule {
        instrument: Id,
        zone: u16,
        module: u16,
        title: String,
        subtitle: String,
        current_name: String,
    },
    RackRenamePin {
        instrument: Id,
        pin_index: usize,
        target: cadence_core::params::InstrumentParam,
        title: String,
        subtitle: String,
        current_label: String,
    },
}

#[derive(Clone, Debug)]
pub(super) struct ValueInput {
    target: ValueInputTarget,
    text: String,
    return_mode: Mode,
    live_preview: Option<String>,
    error: Option<String>,
}

impl App {
    pub fn value_cursor(&self) -> Option<ValueCursorView> {
        if self.scope != EditScope::Definition {
            return None;
        }
        let pattern = self.selected_pattern_object()?;
        let PatternKind::Values {
            unit,
            mode,
            points,
            period,
        } = &pattern.kind
        else {
            return None;
        };
        if points.is_empty() || self.cursor >= *period as usize {
            return None;
        }
        let PatternValue::Value(value) =
            self.resolve_pattern(pattern.id, self.cursor as i64).ok()?
        else {
            return None;
        };
        let point_index = points.iter().position(|p| p.step as usize == self.cursor);
        let after = points.partition_point(|p| p.step as usize <= self.cursor);
        let previous = if after == 0 {
            points.len() - 1
        } else {
            after - 1
        };
        Some(ValueCursorView {
            pattern: pattern.id,
            step: self.cursor + 1,
            offset: self.cursor,
            unit: *unit,
            mode: *mode,
            value,
            authored: point_index.is_some(),
            point_index,
            point_count: points.len(),
            held_from: points[previous].step as usize,
        })
    }

    pub fn value_input_view(&self) -> Option<ValueInputView> {
        let input = self
            .modal
            .value_input()
            .filter(|_| self.input.mode == Mode::ValueInput)?;
        let (title, subtitle, units_hint, current_display) = match &input.target {
            ValueInputTarget::PatternValue {
                pattern,
                step,
                unit,
                current_value,
            } => {
                let name = self
                    .pattern(*pattern)
                    .map_or("Missing source", |p| p.name.as_str())
                    .to_string();
                let hint = if *unit == ValueUnit::Degrees {
                    "DEGREES · integer offset (e.g. -2, +4)".into()
                } else {
                    "NORMALIZED · 0–1 or 0%–100%".into()
                };
                (
                    name,
                    format!("Offset {step} / Step {}", step + 1),
                    hint,
                    format!("{current_value:.3}"),
                )
            }
            ValueInputTarget::Param {
                title,
                subtitle,
                current_display,
                units_hint,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                (*units_hint).into(),
                current_display.clone(),
            ),
            ValueInputTarget::ZoneField {
                title,
                subtitle,
                current_display,
                units_hint,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                (*units_hint).into(),
                current_display.clone(),
            ),
            ValueInputTarget::Macro {
                title,
                current_display,
                ..
            } => (
                title.clone(),
                "MACRO DIAL".into(),
                "0..1 or 0%..100%".into(),
                current_display.clone(),
            ),
            ValueInputTarget::DuckDepth {
                title,
                current_display,
                ..
            } => (
                title.clone(),
                "SIDECHAIN DUCK DEPTH".into(),
                "0..1 or 0%..100% (e.g. 0.65, 65%)".into(),
                current_display.clone(),
            ),
            ValueInputTarget::DuckRecovery {
                title,
                current_display,
                ..
            } => (
                title.clone(),
                "SIDECHAIN DUCK RECOVERY".into(),
                "10..2000 ms (e.g. 120ms, 0.25s)".into(),
                current_display.clone(),
            ),
            ValueInputTarget::PlacementStart {
                title,
                current_start,
                ..
            } => (
                title.clone(),
                "PLACEMENT START POSITION".into(),
                format!("1-based bar (1..{MAX_BARS} or 2b) or step (17s)"),
                format!(
                    "Bar {} · step {}",
                    current_start / 16 + 1,
                    current_start % 16 + 1
                ),
            ),
            ValueInputTarget::PlacementLength {
                title,
                current_length,
                ..
            } => (
                title.clone(),
                "PLACEMENT LENGTH".into(),
                "whole bars (2 or 2b) or steps (24s, min 4s)".into(),
                format!("{current_length} steps"),
            ),
            ValueInputTarget::PlacementOffset {
                title,
                current_offset,
                ..
            } => (
                title.clone(),
                "PLACEMENT PHASE OFFSET".into(),
                "signed integer steps (-1048576..1048576)".into(),
                format!("{current_offset:+} steps"),
            ),
            ValueInputTarget::PartTranspose {
                title,
                current_transpose,
                ..
            } => (
                title.clone(),
                "PART TRANSPOSE".into(),
                "signed semitones (-96..96)".into(),
                format!("{current_transpose:+} st"),
            ),
            ValueInputTarget::PartVelocity {
                title,
                current_velocity,
                ..
            } => (
                title.clone(),
                "PART VELOCITY SCALE".into(),
                "0..1 or 0%..100%".into(),
                format!("{:.0}%", current_velocity * 100.0),
            ),
            ValueInputTarget::CueBar {
                current_bar,
                max_bars,
            } => (
                "Cue Playback to Bar".into(),
                "TRANSPORT CUE".into(),
                format!("1-based bar number (1..{max_bars})"),
                format!("Bar {current_bar}"),
            ),
            ValueInputTarget::AudioLatencyTrim { current_ms } => (
                "Recording Latency Trim".into(),
                "AUDIO INPUT & RECORDING".into(),
                "-250..+250 ms (positive trims earlier, negative pads)".into(),
                format!("{current_ms:+.1} ms"),
            ),
            ValueInputTarget::SongBpm { current_bpm } => (
                "Tempo (BPM)".into(),
                "SONG PROPERTIES".into(),
                "20..300 BPM (e.g. 128, 136.5)".into(),
                format!("{current_bpm:.1} BPM"),
            ),
            ValueInputTarget::SongKeyRoot { current_root } => (
                "Key Root".into(),
                "SONG PROPERTIES".into(),
                "Note name (C4, F#3, Bb2) or MIDI 0–127".into(),
                format!("{} (MIDI {current_root})", note_name(*current_root)),
            ),
            ValueInputTarget::SongBars { current_bars } => (
                "Arrangement Length".into(),
                "SONG PROPERTIES".into(),
                format!("whole bars (1..{MAX_BARS})"),
                format!("{current_bars} bars"),
            ),
            ValueInputTarget::SettingsDefaultBpm { current_bpm } => (
                "Default New-Song Tempo".into(),
                "SETTINGS".into(),
                "20..300 BPM (e.g. 124, 138)".into(),
                format!("{current_bpm:.0} BPM"),
            ),
            ValueInputTarget::SettingsDefaultRoot { current_root } => (
                "Default New-Song Key".into(),
                "SETTINGS".into(),
                "Note name (C1–C6, e.g. D4, F3) or MIDI 24–84".into(),
                format!("{} (MIDI {current_root})", note_name(*current_root)),
            ),
            ValueInputTarget::SettingsLateTolerance { current_steps } => (
                "Late Tolerance".into(),
                "SETTINGS · LIVE TIMING".into(),
                format!("0..{} steps", cadence_core::material::MAX_LATE_TOLERANCE),
                format!(
                    "{current_steps} step{}",
                    if *current_steps == 1 { "" } else { "s" }
                ),
            ),
            ValueInputTarget::SaveFilterGroove {
                suggested_id,
                source_summary,
                ..
            } => (
                "Save Filter Groove to Bank".into(),
                "FILTER GROOVE BANK  ·  presets/filter_grooves.json".into(),
                format!(
                    "Name (auto slot {:02}) or 'NN Name' to target slot 00–127",
                    suggested_id
                ),
                source_summary.clone(),
            ),
            ValueInputTarget::RackConnectCable {
                title,
                subtitle,
                current_display,
                units_hint,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                (*units_hint).into(),
                current_display.clone(),
            ),
            ValueInputTarget::RackRenameModule {
                title,
                subtitle,
                current_name,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                "1–32 printable characters".into(),
                current_name.clone(),
            ),
            ValueInputTarget::RackRenamePin {
                title,
                subtitle,
                current_label,
                ..
            } => (
                title.clone(),
                subtitle.clone(),
                "1–32 printable characters (Quick Panel label)".into(),
                current_label.clone(),
            ),
        };
        Some(ValueInputView {
            title,
            subtitle,
            units_hint,
            current_display,
            input: input.text.clone(),
            live_preview: input.live_preview.clone(),
            error: input.error.clone(),
        })
    }

    pub(super) fn value_canvas(&self) -> bool {
        self.pattern_canvas_focused() && self.value_cursor().is_some()
    }

    fn open_value_input_state(
        &mut self,
        target: ValueInputTarget,
        initial: String,
        return_mode: Mode,
        status: String,
    ) -> Result<Outcome> {
        let mut state = ValueInput {
            target,
            text: initial,
            return_mode,
            live_preview: None,
            error: None,
        };
        self.refresh_value_input_validation(&mut state);
        let return_song_props = self.modal.take_song_props();
        self.modal = ActiveModal::ValueInput {
            state: Box::new(state),
            return_song_props,
        };
        self.input.count = None;
        self.input.mode = Mode::ValueInput;
        self.status = status;
        Ok(self.release_held())
    }

    pub(super) fn begin_value_input(&mut self) -> Result<Outcome> {
        self.begin_value_input_with(String::new())
    }

    pub(super) fn begin_value_input_with(&mut self, initial: String) -> Result<Outcome> {
        let value = self.value_cursor().context("Open a literal value source")?;
        ensure!(self.value_canvas(), "Focus the value canvas first");
        self.open_value_input_state(
            ValueInputTarget::PatternValue {
                pattern: value.pattern,
                step: value.offset,
                unit: value.unit,
                current_value: value.value,
            },
            initial,
            Mode::Normal,
            "Exact value · Enter commits · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_param_value_input(
        &mut self,
        address: ParamAddress,
        initial: String,
    ) -> Result<Outcome> {
        let (title, subtitle, current_display) = self.param_value_input_meta(address);
        let units_hint = self.param_units_hint(address);
        let status = format!("Exact {title} ({units_hint}) · Enter applies · Esc cancels");
        self.open_value_input_state(
            ValueInputTarget::Param {
                address,
                title,
                subtitle,
                current_display,
                units_hint,
            },
            initial,
            Mode::Normal,
            status,
        )
    }

    pub(crate) fn begin_zone_field_value_input(
        &mut self,
        instrument: Id,
        field: ZoneControl,
        initial: String,
    ) -> Result<Outcome> {
        let Some(units_hint) = field.units_hint() else {
            return Ok(Outcome::default());
        };
        let (title, subtitle, current_display) = self
            .instrument(instrument)
            .and_then(|inst| {
                let idx = self.active_zone_index(&inst.rack);
                let z = inst.rack.zones.get(idx)?;
                let cur = match field {
                    ZoneControl::Name => z.name.clone(),
                    ZoneControl::LowMidi => note_name(z.low_midi),
                    ZoneControl::HighMidi => note_name(z.high_midi),
                    ZoneControl::LowVel => format!("{:.0}%", z.low_vel * 100.0),
                    ZoneControl::HighVel => format!("{:.0}%", z.high_vel * 100.0),
                    ZoneControl::ChokeGroup => z
                        .choke_group
                        .map_or_else(|| "None".into(), |g| format!("Group {g}")),
                    ZoneControl::SamplePath => "—".into(),
                };
                Some((
                    format!("{} · {}", z.name, field.label()),
                    format!("{} · z{}", inst.name, z.id),
                    cur,
                ))
            })
            .unwrap_or_else(|| (field.label().into(), "ZONE".into(), "—".into()));
        let status = format!("Exact {title} ({units_hint}) · Enter applies · Esc cancels");
        self.open_value_input_state(
            ValueInputTarget::ZoneField {
                instrument,
                field,
                title,
                subtitle,
                current_display,
                units_hint,
            },
            initial,
            Mode::Normal,
            status,
        )
    }

    pub(crate) fn begin_macro_value_input(
        &mut self,
        index: u8,
        initial: String,
    ) -> Result<Outcome> {
        let (name, cur) = self
            .doc
            .project()
            .macro_control(index)
            .map(|m| (m.name.clone(), format!("{:.0}%", m.value * 100.0)))
            .unwrap_or_else(|| (format!("Macro {}", usize::from(index) + 1), "0%".into()));
        let title = format!("M{} · {name}", usize::from(index) + 1);
        let status = format!("Exact {title} (0–100%) · Enter applies · Esc cancels");
        self.open_value_input_state(
            ValueInputTarget::Macro {
                index,
                title,
                current_display: cur,
            },
            initial,
            Mode::Normal,
            status,
        )
    }

    pub(crate) fn begin_duck_depth_value_input(
        &mut self,
        duck: Id,
        initial: String,
    ) -> Result<Outcome> {
        let d = self
            .doc
            .project()
            .ducks
            .iter()
            .find(|d| d.id == duck)
            .context("Duck missing")?;
        let title = format!("{} Depth", d.name);
        let cur = format!("{:.0}%", d.depth * 100.0);
        let status =
            format!("Exact {title} ({cur}) · 0..1 or 0%..100% · Enter applies · Esc cancels");
        self.open_value_input_state(
            ValueInputTarget::DuckDepth {
                duck,
                title,
                current_display: cur,
            },
            initial,
            Mode::Normal,
            status,
        )
    }

    pub(crate) fn begin_duck_recovery_value_input(
        &mut self,
        duck: Id,
        initial: String,
    ) -> Result<Outcome> {
        let d = self
            .doc
            .project()
            .ducks
            .iter()
            .find(|d| d.id == duck)
            .context("Duck missing")?;
        let title = format!("{} Recovery", d.name);
        let cur = format!("{:.0}ms", d.recovery_ms);
        let status = format!(
            "Exact {title} ({cur}) · {:.0}..{:.0}ms · Enter applies · Esc cancels",
            cadence_core::mixer::DUCK_RECOVERY_MIN_MS,
            cadence_core::mixer::DUCK_RECOVERY_MAX_MS
        );
        self.open_value_input_state(
            ValueInputTarget::DuckRecovery {
                duck,
                title,
                current_display: cur,
            },
            initial,
            Mode::Normal,
            status,
        )
    }

    pub(crate) fn begin_placement_start_value_input(
        &mut self,
        placement: Id,
        initial: String,
    ) -> Result<Outcome> {
        let (_, pl) = self.placement(placement).context("Placement missing")?;
        let part_name = self
            .part(pl.part)
            .map_or("Placement", |p| p.name.as_str())
            .to_string();
        let current_start = pl.start_step;
        self.open_value_input_state(
            ValueInputTarget::PlacementStart {
                placement,
                title: format!("{part_name} Start"),
                current_start,
            },
            initial,
            Mode::Normal,
            "Exact placement start (bar e.g. 2, or step e.g. 17s) · Enter applies · Esc cancels"
                .into(),
        )
    }

    pub(crate) fn begin_placement_length_value_input(
        &mut self,
        placement: Id,
        initial: String,
    ) -> Result<Outcome> {
        let (_, pl) = self.placement(placement).context("Placement missing")?;
        let part_name = self
            .part(pl.part)
            .map_or("Placement", |p| p.name.as_str())
            .to_string();
        let current_length = pl.length_steps;
        self.open_value_input_state(
            ValueInputTarget::PlacementLength {
                placement,
                title: format!("{part_name} Length"),
                current_length,
            },
            initial,
            Mode::Normal,
            "Exact placement length (bars e.g. 4, or steps e.g. 24s) · Enter applies · Esc cancels"
                .into(),
        )
    }

    pub(crate) fn begin_placement_offset_value_input(
        &mut self,
        placement: Id,
        initial: String,
    ) -> Result<Outcome> {
        let (_, pl) = self.placement(placement).context("Placement missing")?;
        let part_name = self
            .part(pl.part)
            .map_or("Placement", |p| p.name.as_str())
            .to_string();
        let current_offset = pl.offset_steps;
        self.open_value_input_state(
            ValueInputTarget::PlacementOffset {
                placement,
                title: format!("{part_name} Offset"),
                current_offset,
            },
            initial,
            Mode::Normal,
            "Exact placement offset (steps) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_part_transpose_value_input(
        &mut self,
        part: Id,
        initial: String,
    ) -> Result<Outcome> {
        let target = self.part(part).context("Part missing")?;
        let title = format!("{} Transpose", target.name);
        let current_transpose = target.transpose;
        self.open_value_input_state(
            ValueInputTarget::PartTranspose {
                part,
                title,
                current_transpose,
            },
            initial,
            Mode::Normal,
            "Exact part transpose (-96..96 st) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_part_velocity_value_input(
        &mut self,
        part: Id,
        initial: String,
    ) -> Result<Outcome> {
        let target = self.part(part).context("Part missing")?;
        let title = format!("{} Velocity", target.name);
        let current_velocity = target.velocity_scale;
        self.open_value_input_state(
            ValueInputTarget::PartVelocity {
                part,
                title,
                current_velocity,
            },
            initial,
            Mode::Normal,
            "Exact part velocity scale (0..1 or 0%..100%) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_cue_bar_value_input(&mut self) -> Result<Outcome> {
        let max_bars = self.doc.project().bars.max(1);
        let current_bar = (self.song_step_heard() as usize / 16 + 1).min(max_bars);
        self.open_value_input_state(
            ValueInputTarget::CueBar {
                current_bar,
                max_bars,
            },
            String::new(),
            Mode::Normal,
            format!("Cue playback to bar (1..{max_bars}) · Enter cues · Esc cancels"),
        )
    }

    pub(crate) fn begin_audio_latency_trim_value_input(&mut self) -> Result<Outcome> {
        let current_ms = self.config.audio_input_latency_ms;
        self.open_value_input_state(
            ValueInputTarget::AudioLatencyTrim { current_ms },
            String::new(),
            Mode::Normal,
            "Manual recording latency trim (-250..+250 ms) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_song_bpm_value_input(&mut self, initial: String) -> Result<Outcome> {
        let current_bpm = self.doc.project().bpm;
        self.open_value_input_state(
            ValueInputTarget::SongBpm { current_bpm },
            initial,
            Mode::SongProperties,
            "Exact tempo (20–300 BPM) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_song_key_root_value_input(&mut self, initial: String) -> Result<Outcome> {
        let current_root = self.doc.project().root_midi;
        self.open_value_input_state(
            ValueInputTarget::SongKeyRoot { current_root },
            initial,
            Mode::SongProperties,
            "Exact key root (e.g. D4, F#3, 62) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_song_bars_value_input(&mut self, initial: String) -> Result<Outcome> {
        let current_bars = self.doc.project().bars;
        self.open_value_input_state(
            ValueInputTarget::SongBars { current_bars },
            initial,
            Mode::SongProperties,
            format!("Exact song length (1–{MAX_BARS} bars) · Enter applies · Esc cancels"),
        )
    }

    pub(crate) fn begin_settings_default_bpm_value_input(
        &mut self,
        initial: String,
    ) -> Result<Outcome> {
        let current_bpm = self.config.default_bpm;
        self.open_value_input_state(
            ValueInputTarget::SettingsDefaultBpm { current_bpm },
            initial,
            Mode::Settings,
            "Exact default tempo (20–300 BPM) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_settings_default_root_value_input(
        &mut self,
        initial: String,
    ) -> Result<Outcome> {
        let current_root = self.config.default_root_midi;
        self.open_value_input_state(
            ValueInputTarget::SettingsDefaultRoot { current_root },
            initial,
            Mode::Settings,
            "Exact default root (C1–C6 or MIDI 24–84) · Enter applies · Esc cancels".into(),
        )
    }

    pub(crate) fn begin_late_tolerance_value_input(
        &mut self,
        initial: String,
        return_mode: Mode,
    ) -> Result<Outcome> {
        let current_steps = self.doc.project().timing.late_tolerance_steps;
        self.open_value_input_state(
            ValueInputTarget::SettingsLateTolerance { current_steps },
            initial,
            return_mode,
            format!(
                "Exact late tolerance (0–{} steps) · Enter applies · Esc cancels",
                cadence_core::material::MAX_LATE_TOLERANCE
            ),
        )
    }

    pub(crate) fn begin_save_filter_groove_input(
        &mut self,
        device: Id,
        compiled: cadence_core::filter_patterns::CompiledFilterPattern,
        suggested_id: u8,
        source_summary: String,
        initial: String,
    ) -> Result<Outcome> {
        self.open_value_input_state(
            ValueInputTarget::SaveFilterGroove {
                device,
                compiled,
                suggested_id,
                source_summary,
            },
            initial,
            Mode::Normal,
            "Save Filter Groove · type name (Ctrl+u clears) · Enter saves to bank · Esc cancels"
                .into(),
        )
    }

    pub(crate) fn begin_rack_cable_amount_input(
        &mut self,
        instrument: Id,
        zone: u16,
        from_module: u16,
        to_module: u16,
        to_port: InPort,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let z = inst
            .rack
            .zone(zone)
            .with_context(|| format!("Zone z{zone} missing"))?;
        let from_name = z
            .module(from_module)
            .map_or_else(|| format!("m{from_module}"), |m| m.name.clone());
        let to_mod = z
            .module(to_module)
            .with_context(|| format!("Target module m{to_module} missing"))?;
        let to_kind = to_mod.kind.tag();
        let to_name = to_mod.name.clone();
        let default_amount = cadence_core::rack::cable_default_amount(to_kind, to_port);
        let default_display =
            cadence_core::rack::format_cable_amount(to_kind, to_port, default_amount);
        let units_hint = match to_port {
            InPort::PitchCv => "semitones (e.g. +12st, -24st) · Enter accepts default",
            InPort::In => "level (0..2 or 0%..200%) · Enter accepts default",
            InPort::Cv => "attenuverter (-200%..+200% or octaves) · Enter accepts default",
            InPort::FmIn | InPort::PwCv | InPort::TrigIn => {
                "depth (-100%..+100%) · Enter accepts default"
            }
        };
        let title = format!("Patch {from_name} → {to_name}.{}", to_port.slug());
        let subtitle = format!("PATCHBAY CABLE · {}", inst.name);
        self.open_value_input_state(
            ValueInputTarget::RackConnectCable {
                instrument,
                zone,
                from_module,
                to_module,
                to_port,
                to_kind,
                default_amount,
                title: title.clone(),
                subtitle,
                current_display: format!("default {default_display}"),
                units_hint,
            },
            String::new(),
            Mode::Normal,
            format!("{title} ({default_display}) · type amount or Enter for default · Esc cancels"),
        )
    }

    pub(crate) fn begin_rack_rename_module(
        &mut self,
        instrument: Id,
        zone: u16,
        module: u16,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let z = inst.rack.zone(zone).context("Rack zone missing")?;
        let m = z.module(module).context("Rack module missing")?;
        let current_name = m.name.clone();
        let title = format!("Rename Module · {}", m.name);
        let subtitle = format!("{} · {} ({})", inst.name, z.name, m.kind.tag().label());
        self.open_value_input_state(
            ValueInputTarget::RackRenameModule {
                instrument,
                zone,
                module,
                title,
                subtitle,
                current_name: current_name.clone(),
            },
            String::new(),
            Mode::Normal,
            format!("Rename module '{current_name}' · Enter applies · Esc cancels"),
        )
    }

    pub(crate) fn begin_rack_rename_pin(
        &mut self,
        instrument: Id,
        pin_index: usize,
    ) -> Result<Outcome> {
        let inst = self.instrument(instrument).context("Instrument missing")?;
        let pin = inst
            .rack
            .panel_pins
            .get(pin_index)
            .context("Quick Panel pin missing")?;
        let current_label = pin.display_label().to_string();
        let target = pin.target;
        let title = format!("Rename Quick Pin · {current_label}");
        let subtitle = format!("{} · Quick Pin #{}", inst.name, pin_index + 1);
        self.open_value_input_state(
            ValueInputTarget::RackRenamePin {
                instrument,
                pin_index,
                target,
                title,
                subtitle,
                current_label: current_label.clone(),
            },
            String::new(),
            Mode::Normal,
            format!("Rename Quick Pin '{current_label}' · Enter applies · Esc cancels"),
        )
    }

    fn preview_value_input(&self, target: &ValueInputTarget, raw: &str) -> Result<Option<String>> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        match target {
            ValueInputTarget::PatternValue { unit, .. } => {
                if *unit == ValueUnit::Normalized {
                    let v = parse_normalized(trimmed)?;
                    Ok(Some(format!("{:.3} ({:.0}%)", v, v * 100.0)))
                } else {
                    let v: f32 = trimmed.parse().context("Enter an integral degree offset")?;
                    ensure!(
                        v.is_finite() && v.fract().abs() < 1e-6,
                        "Enter an integral degree offset"
                    );
                    Ok(Some(format!("{:+.0} deg", v)))
                }
            }
            ValueInputTarget::Param { address, .. } => {
                let native = self.parse_param_native(*address, trimmed)?;
                Ok(Some(self.describe_param(*address, native)))
            }
            ValueInputTarget::ZoneField {
                instrument, field, ..
            } => self
                .preview_zone_field_text(*instrument, *field, trimmed)
                .map(Some),
            ValueInputTarget::Macro { .. } | ValueInputTarget::DuckDepth { .. } => {
                let v = parse_normalized(trimmed)?;
                Ok(Some(format!("{:.0}%", v * 100.0)))
            }
            ValueInputTarget::DuckRecovery { .. } => {
                let ms = crate::commands::parse_ms(trimmed)?;
                ensure!(
                    ms.is_finite()
                        && (cadence_core::mixer::DUCK_RECOVERY_MIN_MS
                            ..=cadence_core::mixer::DUCK_RECOVERY_MAX_MS)
                            .contains(&ms),
                    "Duck recovery must be {:.0}..{:.0} ms",
                    cadence_core::mixer::DUCK_RECOVERY_MIN_MS,
                    cadence_core::mixer::DUCK_RECOVERY_MAX_MS
                );
                Ok(Some(format!("{ms:.0}ms")))
            }
            ValueInputTarget::PlacementStart { placement, .. } => {
                let start_step = drafts::form::parse_position(trimmed)?;
                let (_, pl) = self.placement(*placement).context("Placement missing")?;
                let end_step = start_step
                    .checked_add(pl.length_steps)
                    .context("Placement extends past arrangement bounds")?;
                let end_bar = (end_step as usize).div_ceil(16).max(1);
                ensure!(
                    end_bar <= MAX_BARS,
                    "Placement cannot extend past the {MAX_BARS}-bar arrangement limit"
                );
                Ok(Some(format!(
                    "Bar {} · step {}",
                    start_step / 16 + 1,
                    start_step % 16 + 1
                )))
            }
            ValueInputTarget::PlacementLength { placement, .. } => {
                let length = drafts::form::parse_duration(trimmed)?;
                ensure!(
                    length >= 4,
                    "A placement is at least one beat (4 steps) long"
                );
                let (_, pl) = self.placement(*placement).context("Placement missing")?;
                ensure!(
                    pl.exceptions.iter().all(|e| e.step < length),
                    "Local exception lies past the shortened end"
                );
                let end_step = pl
                    .start_step
                    .checked_add(length)
                    .context("Placement extends past arrangement bounds")?;
                let end_bar = (end_step as usize).div_ceil(16).max(1);
                ensure!(
                    end_bar <= MAX_BARS,
                    "Placement cannot extend past the {MAX_BARS}-bar arrangement limit"
                );
                let label = if length.is_multiple_of(16) {
                    let bars = length / 16;
                    format!(
                        "{bars} bar{} ({length} steps)",
                        if bars == 1 { "" } else { "s" }
                    )
                } else {
                    format!("{length} steps")
                };
                Ok(Some(label))
            }
            ValueInputTarget::PlacementOffset { .. } => {
                let offset: i32 = trimmed
                    .parse()
                    .context("Enter a signed integer step offset")?;
                ensure!(
                    offset.unsigned_abs() <= 1_048_576,
                    "Offset must be within ±1048576 steps"
                );
                Ok(Some(format!("{offset:+} steps")))
            }
            ValueInputTarget::PartTranspose { .. } => {
                let st: i16 = trimmed
                    .trim_end_matches("st")
                    .trim_end_matches("ST")
                    .trim()
                    .parse()
                    .context("Enter signed semitones (-96..96)")?;
                ensure!(
                    (-96..=96).contains(&st),
                    "Part transpose must be -96..96 semitones"
                );
                Ok(Some(format!("{st:+} st")))
            }
            ValueInputTarget::PartVelocity { .. } => {
                let v = parse_normalized(trimmed)?;
                Ok(Some(format!("{:.0}%", v * 100.0)))
            }
            ValueInputTarget::CueBar { max_bars, .. } => {
                let bar: usize = trimmed
                    .trim_start_matches(['b', 'B'])
                    .trim()
                    .parse()
                    .context("Enter a 1-based bar number")?;
                ensure!((1..=*max_bars).contains(&bar), "Bar must be 1..{max_bars}");
                Ok(Some(format!("Cue → Bar {bar}")))
            }
            ValueInputTarget::AudioLatencyTrim { .. } => {
                let ms = crate::commands::parse_ms(trimmed)?;
                ensure!(
                    (cadence_core::runtime::MIN_AUDIO_LATENCY_TRIM_MS
                        ..=cadence_core::runtime::MAX_AUDIO_LATENCY_TRIM_MS)
                        .contains(&ms),
                    "Latency trim must be -250..+250 ms"
                );
                Ok(Some(format!("{ms:+.1} ms")))
            }
            ValueInputTarget::SongBpm { .. } => {
                let bpm: f64 = trimmed
                    .trim_end_matches("bpm")
                    .trim_end_matches("BPM")
                    .trim()
                    .parse()
                    .context("Enter BPM between 20 and 300")?;
                ensure!(
                    bpm.is_finite() && (20.0..=300.0).contains(&bpm),
                    "Tempo must be 20..300 BPM"
                );
                Ok(Some(format!("{bpm:.1} BPM")))
            }
            ValueInputTarget::SongKeyRoot { .. } => {
                let midi = if let Ok(n) = trimmed.parse::<u8>() {
                    ensure!(n <= 127, "MIDI note must be 0..127");
                    n
                } else {
                    parse_root_midi(trimmed)
                        .context("Enter note name (e.g. D4, F#3) or MIDI 0–127")?
                };
                Ok(Some(format!("{} (MIDI {midi})", note_name(midi))))
            }
            ValueInputTarget::SongBars { .. } => {
                let bars: usize = trimmed
                    .trim_end_matches("bars")
                    .trim_end_matches("bar")
                    .trim()
                    .parse()
                    .context("Enter a positive whole number of bars")?;
                ensure!(
                    (1..=MAX_BARS).contains(&bars),
                    "Song length must be 1..{MAX_BARS} bars"
                );
                Ok(Some(format!("{bars} bars ({} steps)", bars * 16)))
            }
            ValueInputTarget::SettingsDefaultBpm { .. } => {
                use cadence_core::config::{MAX_DEFAULT_BPM, MIN_DEFAULT_BPM};
                let bpm: f64 = trimmed
                    .trim_end_matches("bpm")
                    .trim_end_matches("BPM")
                    .trim()
                    .parse()
                    .context("Enter BPM between 20 and 300")?;
                ensure!(
                    bpm.is_finite() && (MIN_DEFAULT_BPM..=MAX_DEFAULT_BPM).contains(&bpm),
                    "Default tempo must be {MIN_DEFAULT_BPM}..{MAX_DEFAULT_BPM} BPM"
                );
                Ok(Some(format!("{bpm:.0} BPM")))
            }
            ValueInputTarget::SettingsDefaultRoot { .. } => {
                use cadence_core::config::{MAX_DEFAULT_ROOT_MIDI, MIN_DEFAULT_ROOT_MIDI};
                let midi = if let Ok(n) = trimmed.parse::<u8>() {
                    n
                } else {
                    parse_root_midi(trimmed)
                        .context("Enter note name (e.g. D4, F3) or MIDI 24–84")?
                };
                ensure!(
                    (MIN_DEFAULT_ROOT_MIDI..=MAX_DEFAULT_ROOT_MIDI).contains(&midi),
                    "Default root must be MIDI {MIN_DEFAULT_ROOT_MIDI}..{MAX_DEFAULT_ROOT_MIDI} (C1–C6)"
                );
                Ok(Some(format!("{} (MIDI {midi})", note_name(midi))))
            }
            ValueInputTarget::SettingsLateTolerance { .. } => {
                let steps = parse_late_tolerance_steps(trimmed)?;
                Ok(Some(format!(
                    "{steps} step{}",
                    if steps == 1 { "" } else { "s" }
                )))
            }
            ValueInputTarget::SaveFilterGroove { compiled, .. } => {
                let (id, preset, _, replacing) =
                    self.plan_save_filter_groove(*compiled, trimmed)?;
                let verb = if replacing { "Update" } else { "Save" };
                Ok(Some(format!(
                    "{verb} slot {id:02} → '{}' (1/{} · {}st)",
                    preset.name, preset.resolution, preset.length
                )))
            }
            ValueInputTarget::RackConnectCable {
                to_kind, to_port, ..
            } => {
                let amount = cadence_core::rack::parse_cable_amount(*to_kind, *to_port, trimmed)?;
                Ok(Some(cadence_core::rack::format_cable_amount(
                    *to_kind, *to_port, amount,
                )))
            }
            ValueInputTarget::RackRenameModule { .. } => {
                let clean = drafts::form::validate_printable_name("Module", trimmed, 32)?;
                Ok(Some(clean))
            }
            ValueInputTarget::RackRenamePin { .. } => {
                let clean = drafts::form::validate_printable_name("Pin label", trimmed, 32)?;
                Ok(Some(clean))
            }
        }
    }

    fn refresh_value_input_validation(&self, state: &mut ValueInput) {
        match self.preview_value_input(&state.target, &state.text) {
            Ok(preview) => {
                state.live_preview = preview;
                state.error = None;
            }
            Err(err) => {
                state.live_preview = None;
                state.error = Some(format!("{err:#}"));
            }
        }
    }

    pub(super) fn handle_value_input(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.code == KeyCode::Esc
            || (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
        {
            let (ret, sp) = self
                .modal
                .take_value_input()
                .map_or((Mode::Normal, None), |(v, sp)| (v.return_mode, sp));
            if let Some(sp) = sp {
                self.modal = ActiveModal::SongProperties(sp);
            }
            self.input.mode = ret;
            self.status = "Value entry cancelled".into();
            return Ok(Outcome::default());
        }
        if key.code == KeyCode::Enter {
            if key.kind == KeyEventKind::Repeat {
                return Ok(Outcome::default());
            }
            let input = self
                .modal
                .value_input()
                .context("Value entry missing")?
                .clone();
            let result = self.commit_value_input(&input);
            return match result {
                Ok(outcome) => {
                    let return_sp = self.modal.take_value_input().and_then(|(_, sp)| sp);
                    if let Some(sp) = return_sp {
                        self.modal = ActiveModal::SongProperties(sp);
                    }
                    self.input.mode = input.return_mode;
                    Ok(outcome)
                }
                Err(error) => {
                    let message = format!("{error:#}");
                    if let Some(v) = self.modal.value_input_mut() {
                        v.error = Some(message.clone());
                        v.live_preview = None;
                    }
                    self.status = message;
                    Ok(Outcome::default())
                }
            };
        }
        let Some((mut state, return_song_props)) = self.modal.take_value_input() else {
            return Ok(Outcome::default());
        };
        let mut mutated = false;
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.text.clear();
                mutated = true;
            }
            KeyCode::Backspace => {
                state.text.pop();
                mutated = true;
            }
            KeyCode::Char(c)
                if !c.is_control()
                    && !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && state.text.len() < 64 =>
            {
                state.text.push(c);
                mutated = true;
            }
            _ => {}
        }
        if mutated {
            self.refresh_value_input_validation(&mut state);
        }
        self.modal = ActiveModal::ValueInput {
            state,
            return_song_props,
        };
        Ok(Outcome::default())
    }

    fn commit_value_input(&mut self, input: &ValueInput) -> Result<Outcome> {
        let trimmed = input.text.trim();
        match &input.target {
            ValueInputTarget::PatternValue {
                pattern,
                step,
                unit,
                current_value,
            } => {
                let value = if trimmed.is_empty() {
                    *current_value
                } else if *unit == ValueUnit::Normalized {
                    parse_amount(trimmed)?
                } else {
                    trimmed
                        .parse::<f32>()
                        .context("Enter an integral degree offset")?
                };
                ensure!(
                    self.pattern(*pattern).is_some_and(
                        |p| matches!(&p.kind, PatternKind::Values { unit: u, .. } if u == unit)
                    ),
                    "The value source changed; cancel and reopen the field"
                );
                let edit = Edit::SetValue {
                    pattern: *pattern,
                    step: *step as u32,
                    value: Some(value),
                };
                let outcome = self.apply(Action::Edit { edit: edit.clone() })?;
                self.last_edit = Some(vec![edit]);
                self.status = format!("Value committed at step {} / offset {step}", step + 1);
                Ok(outcome)
            }
            ValueInputTarget::Param { address, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current value".into();
                    return Ok(Outcome::default());
                }
                let native = self.parse_param_native(*address, trimmed)?;
                self.apply(Action::SetParam {
                    address: *address,
                    value: native,
                })
            }
            ValueInputTarget::ZoneField {
                instrument, field, ..
            } => {
                if trimmed.is_empty() {
                    self.status = "Kept current zone value".into();
                    return Ok(Outcome::default());
                }
                self.apply_zone_field_text(*instrument, *field, trimmed)
            }
            ValueInputTarget::Macro { index, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current macro value".into();
                    return Ok(Outcome::default());
                }
                let val = parse_normalized(trimmed)?;
                self.apply(Action::SetMacro {
                    index: *index,
                    value: val,
                })
            }
            ValueInputTarget::DuckDepth { duck, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current duck depth".into();
                    return Ok(Outcome::default());
                }
                let depth = parse_normalized(trimmed)?;
                let mut updated = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == *duck)
                    .context("Duck missing")?
                    .clone();
                updated.depth = depth;
                let label = format!("{} depth → {:.0}%", updated.name, updated.depth * 100.0);
                self.commit(vec![Edit::PutDuck { duck: updated }], label)
            }
            ValueInputTarget::DuckRecovery { duck, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current duck recovery".into();
                    return Ok(Outcome::default());
                }
                let ms = crate::commands::parse_ms(trimmed)?;
                ensure!(
                    ms.is_finite()
                        && (cadence_core::mixer::DUCK_RECOVERY_MIN_MS
                            ..=cadence_core::mixer::DUCK_RECOVERY_MAX_MS)
                            .contains(&ms),
                    "Duck recovery must be {:.0}..{:.0} ms",
                    cadence_core::mixer::DUCK_RECOVERY_MIN_MS,
                    cadence_core::mixer::DUCK_RECOVERY_MAX_MS
                );
                let mut updated = self
                    .doc
                    .project()
                    .ducks
                    .iter()
                    .find(|d| d.id == *duck)
                    .context("Duck missing")?
                    .clone();
                updated.recovery_ms = ms;
                let label = format!("{} recovery → {:.0}ms", updated.name, updated.recovery_ms);
                self.commit(vec![Edit::PutDuck { duck: updated }], label)
            }
            ValueInputTarget::PlacementStart { placement, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current placement start".into();
                    return Ok(Outcome::default());
                }
                let start_step = drafts::form::parse_position(trimmed)?;
                let current_start = self
                    .placement(*placement)
                    .context("Placement missing")?
                    .1
                    .start_step;
                let shift = i64::from(start_step) - i64::from(current_start);
                if shift == 0 {
                    self.status = "Kept current placement start".into();
                    return Ok(Outcome::default());
                }
                self.reshape_placement(*placement, shift, 0, false)
            }
            ValueInputTarget::PlacementLength { placement, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current placement length".into();
                    return Ok(Outcome::default());
                }
                let length = drafts::form::parse_duration(trimmed)?;
                let current_length = self
                    .placement(*placement)
                    .context("Placement missing")?
                    .1
                    .length_steps;
                let resize = i64::from(length) - i64::from(current_length);
                if resize == 0 {
                    self.status = "Kept current placement length".into();
                    return Ok(Outcome::default());
                }
                self.reshape_placement(*placement, 0, resize, false)
            }
            ValueInputTarget::PlacementOffset { placement, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current placement offset".into();
                    return Ok(Outcome::default());
                }
                let offset: i32 = trimmed
                    .parse()
                    .context("Enter a signed integer step offset")?;
                ensure!(
                    offset.unsigned_abs() <= 1_048_576,
                    "Offset must be within ±1048576 steps"
                );
                let (lane, pl) = self.placement(*placement).context("Placement missing")?;
                let lane_id = lane.id;
                let mut updated = pl.clone();
                updated.offset_steps = offset;
                self.commit(
                    vec![Edit::PutPlacement {
                        lane: lane_id,
                        placement: updated,
                    }],
                    format!("Placement offset → {offset:+} steps"),
                )
            }
            ValueInputTarget::PartTranspose { part, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current part transpose".into();
                    return Ok(Outcome::default());
                }
                let st: i16 = trimmed
                    .trim_end_matches("st")
                    .trim_end_matches("ST")
                    .trim()
                    .parse()
                    .context("Enter signed semitones (-96..96)")?;
                ensure!(
                    (-96..=96).contains(&st),
                    "Part transpose must be -96..96 semitones"
                );
                let mut updated = self.part(*part).context("Part missing")?.clone();
                updated.transpose = st;
                self.commit(
                    vec![Edit::PutPart { part: updated }],
                    format!("Part transpose → {st:+} st"),
                )
            }
            ValueInputTarget::PartVelocity { part, .. } => {
                if trimmed.is_empty() {
                    self.status = "Kept current part velocity".into();
                    return Ok(Outcome::default());
                }
                let v = parse_normalized(trimmed)?;
                let mut updated = self.part(*part).context("Part missing")?.clone();
                updated.velocity_scale = v;
                self.commit(
                    vec![Edit::PutPart { part: updated }],
                    format!("Part velocity → {:.0}%", v * 100.0),
                )
            }
            ValueInputTarget::CueBar { max_bars, .. } => {
                if trimmed.is_empty() {
                    self.status = "Playback position unchanged".into();
                    return Ok(Outcome::default());
                }
                let bar: usize = trimmed
                    .trim_start_matches(['b', 'B'])
                    .trim()
                    .parse()
                    .context("Enter a 1-based bar number")?;
                ensure!((1..=*max_bars).contains(&bar), "Bar must be 1..{max_bars}");
                self.apply(Action::CueBar { bar: bar - 1 })
            }
            ValueInputTarget::AudioLatencyTrim { .. } => {
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let ms = crate::commands::parse_ms(trimmed)?;
                ensure!(
                    (cadence_core::runtime::MIN_AUDIO_LATENCY_TRIM_MS
                        ..=cadence_core::runtime::MAX_AUDIO_LATENCY_TRIM_MS)
                        .contains(&ms),
                    "Latency trim must be -250..+250 ms"
                );
                Ok(self.set_audio_latency_trim_ms(ms))
            }
            ValueInputTarget::SongBpm { .. } => {
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let bpm: f64 = trimmed
                    .trim_end_matches("bpm")
                    .trim_end_matches("BPM")
                    .trim()
                    .parse()
                    .context("Enter BPM between 20 and 300")?;
                ensure!(
                    bpm.is_finite() && (20.0..=300.0).contains(&bpm),
                    "Tempo must be 20..300 BPM"
                );
                self.commit(
                    vec![Edit::SetTempo { bpm }],
                    format!("Tempo → {bpm:.1} BPM"),
                )
            }
            ValueInputTarget::SongKeyRoot { .. } => {
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let root_midi = if let Ok(n) = trimmed.parse::<u8>() {
                    ensure!(n <= 127, "MIDI note must be 0..127");
                    n
                } else {
                    parse_root_midi(trimmed)
                        .context("Enter note name (e.g. D4, F#3) or MIDI 0–127")?
                };
                let scale = self.doc.project().scale.clone();
                let label = format!(
                    "Key → {} {}",
                    note_name(root_midi),
                    cadence_core::music::scale_label(&scale)
                );
                self.commit(vec![Edit::SetKey { root_midi, scale }], label)
            }
            ValueInputTarget::SongBars { .. } => {
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let bars: usize = trimmed
                    .trim_end_matches("bars")
                    .trim_end_matches("bar")
                    .trim()
                    .parse()
                    .context("Enter a positive whole number of bars")?;
                ensure!(
                    (1..=MAX_BARS).contains(&bars),
                    "Song length must be 1..{MAX_BARS} bars"
                );
                self.commit(
                    vec![Edit::SetBars { bars }],
                    format!("Arrangement length → {bars} bars"),
                )
            }
            ValueInputTarget::SettingsDefaultBpm { .. } => {
                use cadence_core::config::{MAX_DEFAULT_BPM, MIN_DEFAULT_BPM};
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let bpm: f64 = trimmed
                    .trim_end_matches("bpm")
                    .trim_end_matches("BPM")
                    .trim()
                    .parse()
                    .context("Enter BPM between 20 and 300")?;
                ensure!(
                    bpm.is_finite() && (MIN_DEFAULT_BPM..=MAX_DEFAULT_BPM).contains(&bpm),
                    "Default tempo must be {MIN_DEFAULT_BPM}..{MAX_DEFAULT_BPM} BPM"
                );
                self.config.default_bpm = bpm;
                self.settings_ui.config_dirty = true;
                self.status = format!("Default new-song BPM: {:.0}", self.config.default_bpm);
                Ok(Outcome::default())
            }
            ValueInputTarget::SettingsDefaultRoot { .. } => {
                use cadence_core::config::{MAX_DEFAULT_ROOT_MIDI, MIN_DEFAULT_ROOT_MIDI};
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let midi = if let Ok(n) = trimmed.parse::<u8>() {
                    n
                } else {
                    parse_root_midi(trimmed)
                        .context("Enter note name (e.g. D4, F3) or MIDI 24–84")?
                };
                ensure!(
                    (MIN_DEFAULT_ROOT_MIDI..=MAX_DEFAULT_ROOT_MIDI).contains(&midi),
                    "Default root must be MIDI {MIN_DEFAULT_ROOT_MIDI}..{MAX_DEFAULT_ROOT_MIDI} (C1–C6)"
                );
                self.config.default_root_midi = midi;
                self.settings_ui.config_dirty = true;
                self.status = format!("Default new-song key: {}", note_name(midi));
                Ok(Outcome::default())
            }
            ValueInputTarget::SettingsLateTolerance { .. } => {
                if trimmed.is_empty() {
                    return Ok(Outcome::default());
                }
                let steps = parse_late_tolerance_steps(trimmed)?;
                let mut timing = self.doc.project().timing;
                timing.late_tolerance_steps = steps;
                self.commit(
                    vec![Edit::SetLiveTiming { timing }],
                    format!(
                        "Late tolerance → {steps} step{}",
                        if steps == 1 { "" } else { "s" }
                    ),
                )
            }
            ValueInputTarget::SaveFilterGroove {
                device, compiled, ..
            } => self.commit_save_filter_groove(*device, *compiled, trimmed),
            ValueInputTarget::RackConnectCable {
                instrument,
                zone,
                from_module,
                to_module,
                to_port,
                to_kind,
                default_amount,
                ..
            } => {
                let amount = if trimmed.is_empty() {
                    *default_amount
                } else {
                    cadence_core::rack::parse_cable_amount(*to_kind, *to_port, trimmed)?
                };
                self.apply(Action::RackConnectCable {
                    instrument: *instrument,
                    zone: *zone,
                    from_module: *from_module,
                    to_module: *to_module,
                    to_port: *to_port,
                    amount,
                })
            }
            ValueInputTarget::RackRenameModule {
                instrument,
                zone,
                module,
                ..
            } => {
                if trimmed.is_empty() {
                    self.status = "Kept current module name".into();
                    return Ok(Outcome::default());
                }
                let clean = drafts::form::validate_printable_name("Module", trimmed, 32)?;
                self.apply(Action::RackRenameModule {
                    instrument: *instrument,
                    zone: *zone,
                    module: *module,
                    name: clean,
                })
            }
            ValueInputTarget::RackRenamePin {
                instrument,
                pin_index,
                target,
                ..
            } => {
                if trimmed.is_empty() {
                    self.status = "Kept current Quick Pin label".into();
                    return Ok(Outcome::default());
                }
                let clean = drafts::form::validate_printable_name("Pin label", trimmed, 32)?;
                let pin = cadence_core::rack::PanelPin {
                    target: *target,
                    label: clean.clone(),
                };
                self.commit(
                    vec![Edit::PinPanelControl {
                        instrument: *instrument,
                        pin,
                        index: Some(*pin_index),
                    }],
                    format!("Renamed Quick Pin → '{clean}'"),
                )
            }
        }
    }

    pub(super) fn count_digit(&mut self, key: KeyEvent) -> bool {
        let accepts_count = self.focused_canvas().is_some()
            || matches!(
                self.focused_deck_kind(),
                Some(layout::DeckKind::Take | layout::DeckKind::Graph | layout::DeckKind::Midi)
            );
        if !accepts_count || !key.modifiers.is_empty() {
            return false;
        }
        let KeyCode::Char(c) = key.code else {
            return false;
        };
        let Some(digit) = c.to_digit(10) else {
            return false;
        };
        if digit == 0 && self.input.count.is_none() {
            return false;
        }
        if let Some(next) = self
            .input
            .count
            .unwrap_or(0)
            .checked_mul(10)
            .and_then(|n| n.checked_add(digit as usize))
            .filter(|&n| n <= MAX_COUNT)
        {
            self.input.count = Some(next);
            self.status = if self.mixer_canvas_focused() {
                format!("Count {next} · h/l channels · j/k row · Esc clears")
            } else if self.evaluated_arrange_canvas() {
                format!("Count {next} · h/l time · w/b note events · Esc clears")
            } else {
                format!("Count {next} · h/l time · w/b points · Esc clears")
            };
        } else {
            self.status = format!("Count is limited to {MAX_COUNT}; Esc clears it");
        }
        true
    }

    pub(super) fn move_point(&mut self, forward: bool, count: usize) -> Result<Outcome> {
        ensure!(
            self.pattern_canvas_focused() && self.scope == EditScope::Definition,
            "Open a literal source to navigate authored points"
        );
        let points: Vec<usize> = match &self
            .selected_pattern_object()
            .context("Select a source")?
            .kind
        {
            PatternKind::Values { points, .. } => points.iter().map(|p| p.step as usize).collect(),
            PatternKind::Notes { steps } => steps
                .iter()
                .enumerate()
                .filter_map(|(i, s)| s.active.then_some(i))
                .collect(),
            PatternKind::Rhythm { steps } => steps
                .iter()
                .enumerate()
                .filter_map(|(i, &on)| on.then_some(i))
                .collect(),
            _ => bail!("This preview has no authored points; follow one of its sources"),
        };
        if points.is_empty() {
            self.status = "No authored points yet".into();
            return Ok(Outcome::default());
        }
        if forward {
            let first = points.partition_point(|&p| p <= self.cursor);
            if first < points.len() {
                self.cursor = points[first
                    .saturating_add(count.saturating_sub(1))
                    .min(points.len() - 1)];
            }
        } else {
            let after = points.partition_point(|&p| p < self.cursor);
            if after > 0 {
                self.cursor = points[after.saturating_sub(count.max(1))];
            }
        }
        self.status = format!(
            "Step {} / offset {} · navigation only",
            self.cursor + 1,
            self.cursor
        );
        Ok(Outcome::default())
    }

    pub(super) fn nudge_value(
        &mut self,
        direction: i8,
        count: usize,
        coarse: bool,
        coalesce: bool,
    ) -> Result<Outcome> {
        let point = self.value_cursor().context("Open a literal value source")?;
        let step = match (point.unit, coarse) {
            (ValueUnit::Degrees, false) => 1.0,
            (ValueUnit::Degrees, true) => 7.0,
            (ValueUnit::Normalized, false) => 0.01,
            (ValueUnit::Normalized, true) => 0.1,
        };
        let value = point.value + f32::from(direction) * step * count.min(MAX_COUNT) as f32;
        let value = match point.unit {
            ValueUnit::Degrees => value.clamp(-63.0, 63.0),
            ValueUnit::Normalized => value.clamp(0.0, 1.0),
        };
        let label = format!("Step {} value → {value:.2}", point.offset + 1);
        self.commit_coalesced(
            vec![Edit::SetValue {
                pattern: point.pattern,
                step: point.offset as u32,
                value: Some(value),
            }],
            label,
            document::CoalesceKey::PatternValue {
                pattern: point.pattern,
                step: point.offset,
            },
            coalesce,
        )
    }

    pub(super) fn delete_value_point(&mut self) -> Result<Outcome> {
        let point = self.value_cursor().context("Open a literal value source")?;
        if !point.authored {
            self.status = "This is a held/interpolated value, not an authored point".into();
            return Ok(Outcome::default());
        }
        ensure!(
            point.point_count > 1,
            "A value pattern needs one point; edit its value instead of deleting the last point"
        );
        self.apply(Action::Edit {
            edit: Edit::SetValue {
                pattern: point.pattern,
                step: point.offset as u32,
                value: None,
            },
        })
    }

    pub(super) fn begin_value_mode(&mut self) -> Result<Outcome> {
        let point = self.value_cursor().context("Open a literal value source")?;
        if point.unit == ValueUnit::Degrees {
            self.status = "Degree offsets use Hold; normalized values also support Ramp".into();
        } else {
            self.input.mode = Mode::Interpolation;
        }
        Ok(Outcome::default())
    }

    pub(crate) fn set_value_mode(
        &mut self,
        pattern_id: Id,
        mode: Option<ValueMode>,
    ) -> Result<Outcome> {
        let mut pattern = self
            .pattern(pattern_id)
            .context("Select a value source")?
            .clone();
        let name = pattern.name.clone();
        let PatternKind::Values {
            unit,
            mode: interpolation,
            ..
        } = &mut pattern.kind
        else {
            bail!(
                "Value mode (Hold/Ramp) applies to literal normalized Values patterns; '{}' is {}",
                name,
                kind_label(&pattern.kind)
            );
        };
        ensure!(
            *unit == ValueUnit::Normalized,
            "Degree offsets use Hold; normalized Values support Hold and Ramp"
        );
        let next = mode.unwrap_or(match *interpolation {
            ValueMode::Hold => ValueMode::Ramp,
            ValueMode::Ramp => ValueMode::Hold,
        });
        *interpolation = next;
        let out = self.apply(Action::Edit {
            edit: Edit::PutPattern { pattern },
        })?;
        self.status = format!(
            "Value mode for '{name}' → {}",
            match next {
                ValueMode::Hold => "Hold",
                ValueMode::Ramp => "Ramp",
            }
        );
        Ok(out)
    }

    pub(super) fn handle_value_mode(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.kind == KeyEventKind::Repeat {
            return Ok(Outcome::default());
        }
        let mode = match key.code {
            KeyCode::Char('h') => Some(Some(ValueMode::Hold)),
            KeyCode::Char('r') => Some(Some(ValueMode::Ramp)),
            KeyCode::Char('m') => Some(None),
            KeyCode::Enter | KeyCode::Esc => {
                self.input.mode = Mode::Normal;
                None
            }
            _ => None,
        };
        let Some(mode) = mode else {
            return Ok(Outcome::default());
        };
        let id = self.selected_pattern.context("Select a value source")?;
        let outcome = self.set_value_mode(id, mode)?;
        self.input.mode = Mode::Normal;
        Ok(outcome)
    }

    /// Slide an authored breakpoint in time (`Alt+h/l` ±1, `Alt+H/L` ±4), or move the step cursor
    /// when parked on an interpolated step.
    pub(super) fn slide_value_point(&mut self, delta: isize, coalesce: bool) -> Result<Outcome> {
        let point = self.value_cursor().context("Open a literal value source")?;
        let mut pattern = self
            .pattern(point.pattern)
            .context("Value pattern missing")?
            .clone();
        let PatternKind::Values { period, points, .. } = &mut pattern.kind else {
            bail!("Open a literal value source");
        };
        let max_step = (*period as isize).saturating_sub(1).max(0);
        let target = (self.cursor as isize + delta).clamp(0, max_step) as u32;
        if target as usize == self.cursor {
            return Ok(Outcome::default());
        }
        if !point.authored {
            self.cursor = target as usize;
            self.status = format!("Step {} / offset {}", self.cursor + 1, self.cursor);
            return Ok(Outcome::default());
        }
        if points.iter().any(|p| p.step == target) {
            self.status = format!("Step {} already has a breakpoint", target + 1);
            return Ok(Outcome::default());
        }
        let Some(entry) = points.iter_mut().find(|p| p.step as usize == self.cursor) else {
            return Ok(Outcome::default());
        };
        entry.step = target;
        points.sort_by_key(|p| p.step);
        let out = self.commit_coalesced(
            vec![Edit::PutPattern { pattern }],
            format!("Moved breakpoint → step {}", target + 1),
            document::CoalesceKey::PatternValueSlide {
                pattern: point.pattern,
            },
            coalesce,
        )?;
        self.cursor = target as usize;
        Ok(out)
    }
}
