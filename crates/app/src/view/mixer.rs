//! Semantic Mixer deck view: channels, trim/pan, M/S, live stereo meters, routing, sends, FX, ducks.
use super::format_db;
use crate::App;
use cadence_core::material::ModulationTarget;
use cadence_core::mixer::{ChannelKind, ChannelParam, ChannelRef, EffectDevice, EffectKind};
use cadence_core::params::{format_pan, ParamAddress};
use cadence_core::runtime::{AudioSnapshot, ChannelMeter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerRole {
    Master,
    Audio,
    Group,
    Return,
}

impl MixerRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::Master => "MASTER",
            Self::Audio => "AUDIO",
            Self::Group => "GROUP",
            Self::Return => "RETURN",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Master => "MST",
            Self::Audio => "CH",
            Self::Group => "GRP",
            Self::Return => "RET",
        }
    }
}

/// Inspect active non-bypassed Compressor/Limiter inserts on one mixer bus and return
/// `(gr_db, gr_hold_db, tag)` for the primary/heaviest-reducing dynamics device.
pub(super) fn strip_dynamics_gr(
    inserts: &[EffectDevice],
    snapshot: &AudioSnapshot,
) -> (f32, f32, Option<&'static str>) {
    let mut best: Option<(f32, f32, &'static str)> = None;
    for dev in inserts {
        if dev.bypassed {
            continue;
        }
        let tag = match dev.kind {
            EffectKind::Compressor(_) => "CMP",
            EffectKind::Limiter(_) => "LIM",
            _ => continue,
        };
        let gr = snapshot.effect_gain_reduction_db(dev.id);
        let hold = snapshot.effect_gr_hold_db(dev.id);
        let replace = match best {
            None => true,
            Some((best_gr, best_hold, best_tag)) => {
                gr > best_gr + 1e-4
                    || ((gr - best_gr).abs() <= 1e-4 && hold > best_hold + 1e-4)
                    || ((gr - best_gr).abs() <= 1e-4
                        && (hold - best_hold).abs() <= 1e-4
                        && tag == "LIM"
                        && best_tag != "LIM")
            }
        };
        if replace {
            best = Some((gr, hold, tag));
        }
    }
    match best {
        Some((gr, hold, tag)) => (gr, hold, Some(tag)),
        None => (0.0, 0.0, None),
    }
}

/// Compute the leftmost visible bank strip index for smooth horizontal Mixer scrolling.
///
/// - When `bank_len <= visible_slots`, returns `0`.
/// - When `selected_bank_idx` is `Some(sel)`, shifts the viewport minimally so `sel` stays
///   within the `[first + scrolloff, first + visible_slots - scrolloff)` band (`scrolloff = 1`
///   whenever `visible_slots >= 4`, `0` otherwise).
/// - When `selected_bank_idx` is `None` (e.g. pinned `Master` is focused via `$`/`G`), preserves
///   `prev_first` clamped to `bank_len - visible_slots` so the bank doesn't jump unexpectedly.
pub fn mixer_bank_scroll_window(
    selected_bank_idx: Option<usize>,
    bank_len: usize,
    visible_slots: usize,
    prev_first: usize,
) -> usize {
    if visible_slots == 0 || bank_len <= visible_slots {
        return 0;
    }
    let max_first = bank_len - visible_slots;
    let mut first = prev_first.min(max_first);
    if let Some(sel) = selected_bank_idx {
        let sel = sel.min(bank_len - 1);
        let scrolloff = if visible_slots >= 4 { 1 } else { 0 };
        if sel < first + scrolloff {
            first = sel.saturating_sub(scrolloff);
        } else if sel + scrolloff >= first + visible_slots {
            first = (sel + scrolloff + 1)
                .saturating_sub(visible_slots)
                .min(max_first);
        }
    }
    first
}

/// Compute the top visible row index for smooth edge-following vertical scrolling inside the
/// Mixer's fixed-height channel detail stack (`FX / SND / OUT / DUCK`).
pub fn mixer_detail_scroll_window(
    focused_row: Option<usize>,
    total_rows: usize,
    visible_rows: usize,
    prev_first: usize,
) -> usize {
    if visible_rows == 0 || total_rows <= visible_rows {
        return 0;
    }
    let max_first = total_rows - visible_rows;
    let mut first = prev_first.min(max_first);
    if let Some(idx) = focused_row {
        let idx = idx.min(total_rows - 1);
        if idx < first {
            first = idx;
        } else if idx >= first + visible_rows {
            first = (idx + 1).saturating_sub(visible_rows).min(max_first);
        }
    }
    first
}

#[derive(Clone, Debug)]
pub struct MixerDeckView {
    pub channels: Vec<MixerChannelView>,
    pub selected_index: usize,
    pub focused_slot: crate::MixerSlot,
    /// Leftmost visible non-Master bank strip index (smoothly tracked across frames).
    pub bank_scroll: usize,
    /// Top visible row index inside the fixed-height bottom channel detail stack.
    pub detail_scroll: usize,
    pub pending: bool,
    pub solo_audition: bool,
}

#[derive(Clone, Debug)]
pub struct MixerChannelView {
    pub channel: ChannelRef,
    pub index: usize,
    pub name: String,
    pub role: MixerRole,
    pub selected: bool,
    pub gain: f32,
    pub gain_db: String,
    pub pan: f32,
    pub pan_label: String,
    pub muted: bool,
    pub solo: bool,
    pub route_name: String,
    pub sends: Vec<(String, f32)>,
    /// 1:1 with `sends`: true when modulated or bound to a Project Macro.
    pub send_controlled: Vec<bool>,
    pub inserts: Vec<(String, bool)>,
    pub ducks: Vec<String>,
    /// 1:1 with `ducks`: `(depth, recovery_ms, source_part_name)` for the vertical detail stack.
    pub duck_details: Vec<(f32, f32, String)>,
    pub level_modulators: Vec<String>,
    /// Live automated gain scale (`0.0..=1.0`) when `ModulationTarget::ChannelGain` is active.
    pub automated_gain_scale: Option<f32>,
    /// Live effective automated gain (`strip.gain * gain_scale`) when modulated.
    pub effective_gain: Option<f32>,
    /// Formatted dB string for `effective_gain` when modulated.
    pub effective_gain_db: Option<String>,
    pub pan_modulators: Vec<String>,
    /// Live automated pan (`-1.0..=1.0`) when `ModulationTarget::ChannelPan` is active.
    pub effective_pan: Option<f32>,
    /// Formatted pan string for `effective_pan` when modulated.
    pub effective_pan_label: Option<String>,
    pub meter: Option<ChannelMeter>,
    /// Live dynamics gain reduction in dB (`>= 0.0`) from the primary active Compressor/Limiter.
    pub gr_db: f32,
    /// Peak-held dynamics gain reduction in dB (`>= 0.0`).
    pub gr_hold_db: f32,
    /// Primary active dynamics tag (`Some("CMP")` or `Some("LIM")`), if any non-bypassed dynamics insert exists.
    pub gr_tag: Option<&'static str>,
}

impl App {
    pub(super) fn build_mixer_view(&self, snapshot: &AudioSnapshot) -> MixerDeckView {
        let project = self.project();
        let all = self.channels();
        let solo_audition = self.is_solo_auditioning();
        let selected_index = all
            .iter()
            .position(|c| *c == self.selected_channel)
            .unwrap_or(0);
        let query_time = self.live_query_time(snapshot);

        let channels: Vec<MixerChannelView> = all
            .iter()
            .enumerate()
            .map(|(index, &ch)| {
                let (strip, role, route_name, sends, send_controlled, inserts, raw_inserts) =
                    match ch {
                        ChannelRef::Master => (
                            project.mixer.master.strip,
                            MixerRole::Master,
                            "DAC / WAV output".into(),
                            Vec::new(),
                            Vec::new(),
                            project
                                .mixer
                                .master
                                .inserts
                                .iter()
                                .map(|d| (d.name.clone(), d.bypassed))
                                .collect(),
                            project.mixer.master.inserts.as_slice(),
                        ),
                        ChannelRef::Channel(id) => self.channel(id).map_or_else(
                            || {
                                (
                                    Default::default(),
                                    MixerRole::Audio,
                                    String::new(),
                                    Vec::new(),
                                    Vec::new(),
                                    Vec::new(),
                                    &[][..],
                                )
                            },
                            |c| {
                                let role = match c.kind {
                                    ChannelKind::Audio => MixerRole::Audio,
                                    ChannelKind::Group => MixerRole::Group,
                                    ChannelKind::Return => MixerRole::Return,
                                };
                                let route = self.channel_name(c.output).to_string();
                                let send_targets = self.channel_send_targets(id);
                                let mut sends = Vec::with_capacity(send_targets.len());
                                let mut send_controlled = Vec::with_capacity(send_targets.len());
                                for (target, explicit) in send_targets {
                                    let addr = ParamAddress::Send {
                                        channel: ch,
                                        target,
                                    };
                                    let amount = self
                                        .evaluate_live_param_at(addr, query_time)
                                        .unwrap_or_else(|| explicit.unwrap_or(0.0));
                                    sends.push((
                                        self.channel_name(ChannelRef::Channel(target)).to_string(),
                                        amount,
                                    ));
                                    send_controlled.push(self.param_is_controlled(addr));
                                }
                                let inserts = c
                                    .inserts
                                    .iter()
                                    .map(|d| (d.name.clone(), d.bypassed))
                                    .collect();
                                (
                                    c.strip,
                                    role,
                                    route,
                                    sends,
                                    send_controlled,
                                    inserts,
                                    c.inserts.as_slice(),
                                )
                            },
                        ),
                    };

                let mut level_modulators: Vec<String> = project
                    .modulations
                    .iter()
                    .filter(|m| {
                        matches!(m.target, ModulationTarget::ChannelGain { channel } if channel == ch)
                    })
                    .map(|m| {
                        self.pattern(m.pattern)
                            .map_or_else(|| "missing".into(), |p| p.name.clone())
                    })
                    .collect();
                let gain_addr = ParamAddress::Channel {
                    channel: ch,
                    param: ChannelParam::Gain,
                };
                level_modulators.extend(self.macro_modulator_labels(gain_addr));
                let macro_gain = self
                    .evaluate_live_param_at(gain_addr, query_time)
                    .map(|v| ChannelParam::Gain.denormalize(v));
                let automated_gain_scale = self.evaluate_channel_gain_scale_at(ch, query_time);
                let effective_gain = match (macro_gain, automated_gain_scale) {
                    (Some(mg), Some(scale)) => {
                        Some((mg * scale).clamp(0.0, cadence_core::dsp::MAX_CHANNEL_GAIN))
                    }
                    (Some(mg), None) => Some(mg.clamp(0.0, cadence_core::dsp::MAX_CHANNEL_GAIN)),
                    (None, Some(scale)) => {
                        Some((strip.gain * scale).clamp(0.0, cadence_core::dsp::MAX_CHANNEL_GAIN))
                    }
                    (None, None) => None,
                };
                let effective_gain_db = effective_gain.map(format_db);

                let mut pan_modulators: Vec<String> = project
                    .modulations
                    .iter()
                    .filter(|m| {
                        matches!(m.target, ModulationTarget::ChannelPan { channel } if channel == ch)
                    })
                    .map(|m| {
                        self.pattern(m.pattern)
                            .map_or_else(|| "missing".into(), |p| p.name.clone())
                    })
                    .collect();
                let pan_addr = ParamAddress::Channel {
                    channel: ch,
                    param: ChannelParam::Pan,
                };
                pan_modulators.extend(self.macro_modulator_labels(pan_addr));
                let effective_pan = self
                    .evaluate_live_param_at(pan_addr, query_time)
                    .map(|v| ChannelParam::Pan.denormalize(v));
                let effective_pan_label = effective_pan.map(format_pan);

                let mut ducks = Vec::new();
                let mut duck_details = Vec::new();
                for d in project.ducks.iter().filter(|d| d.target == ch) {
                    ducks.push(d.name.clone());
                    let src_name = self
                        .part(d.source_part)
                        .map_or("Missing", |p| p.name.as_str())
                        .to_string();
                    duck_details.push((d.depth, d.recovery_ms, src_name));
                }

                let meter = if solo_audition {
                    None
                } else {
                    snapshot.channel_meter(ch).copied()
                };
                let (gr_db, gr_hold_db, gr_tag) = if solo_audition {
                    (0.0, 0.0, None)
                } else {
                    strip_dynamics_gr(raw_inserts, snapshot)
                };

                MixerChannelView {
                    channel: ch,
                    index,
                    name: self.channel_name(ch).to_string(),
                    role,
                    selected: ch == self.selected_channel,
                    gain: strip.gain,
                    gain_db: format_db(strip.gain),
                    pan: strip.pan,
                    pan_label: format_pan(strip.pan),
                    muted: self.flags().channel_muted(ch),
                    solo: self.flags().channel_solo(ch),
                    route_name,
                    sends,
                    send_controlled,
                    inserts,
                    ducks,
                    duck_details,
                    level_modulators,
                    automated_gain_scale,
                    effective_gain,
                    effective_gain_db,
                    pan_modulators,
                    effective_pan,
                    effective_pan_label,
                    meter,
                    gr_db,
                    gr_hold_db,
                    gr_tag,
                }
            })
            .collect();

        let valid_slots = self.mixer_slots_for(self.selected_channel);
        let focused_slot = if valid_slots.contains(&self.view_state.mixer_slot) {
            self.view_state.mixer_slot
        } else {
            crate::MixerSlot::Fader
        };

        // Smooth horizontal bank scroll (non-Master channels live in `0..bank_len`, Master is last).
        let has_master = channels
            .last()
            .is_some_and(|ch| ch.role == MixerRole::Master);
        let bank_len = if has_master {
            channels.len().saturating_sub(1)
        } else {
            channels.len()
        };
        let selected_bank_idx = if has_master && selected_index >= bank_len {
            None
        } else {
            Some(selected_index)
        };
        let bank_scroll = mixer_bank_scroll_window(
            selected_bank_idx,
            bank_len,
            self.view_state.mixer_page_size.max(1),
            self.view_state.mixer_scroll.get(),
        );
        self.view_state.mixer_scroll.set(bank_scroll);

        // Smooth vertical detail scroll (reset whenever switching selected channel).
        if self.view_state.mixer_detail_channel.get() != Some(self.selected_channel) {
            self.view_state
                .mixer_detail_channel
                .set(Some(self.selected_channel));
            self.view_state.mixer_detail_scroll.set(0);
        }
        let detail_scroll = if let Some(sel_ch) = channels.get(selected_index) {
            let has_out = !matches!(sel_ch.role, MixerRole::Master | MixerRole::Return);
            let ins = sel_ch.inserts.len();
            let snd = sel_ch.sends.len();
            let out = usize::from(has_out);
            let total_rows = ins + snd + out + sel_ch.ducks.len();
            let focused_row = match focused_slot {
                crate::MixerSlot::Fader | crate::MixerSlot::Pan => None,
                crate::MixerSlot::Insert(i) => (i < ins).then_some(i),
                crate::MixerSlot::Send(i) => (i < snd).then_some(ins + i),
                crate::MixerSlot::Output => has_out.then_some(ins + snd),
                crate::MixerSlot::Duck(i) => {
                    (i < sel_ch.ducks.len()).then_some(ins + snd + out + i)
                }
            };
            let next = mixer_detail_scroll_window(
                focused_row,
                total_rows,
                self.view_state.mixer_detail_rows.max(1),
                self.view_state.mixer_detail_scroll.get(),
            );
            self.view_state.mixer_detail_scroll.set(next);
            next
        } else {
            0
        };

        MixerDeckView {
            channels,
            selected_index,
            focused_slot,
            bank_scroll,
            detail_scroll,
            pending: snapshot.pending,
            solo_audition,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::view::DeckView;
    use crate::App;
    use cadence_core::material::{
        Modulation, ModulationTarget, Pattern, PatternKind, Project, ValuePoint, ValueUnit,
    };
    use cadence_core::mixer::ChannelRef;
    use cadence_core::params::ValueMode;
    use cadence_core::runtime::AudioSnapshot;

    #[test]
    fn mixer_view_projects_live_automated_effective_gain() {
        let mut project = Project::demo();
        let target_ch = project.mixer.channels[0].id;
        project.mixer.channels[0].strip.gain = 0.8;
        let pat_id = project.patterns.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        project.patterns.push(Pattern {
            id: pat_id,
            name: "Fade In".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Ramp,
                points: vec![
                    ValuePoint {
                        step: 0,
                        value: 0.25,
                    },
                    ValuePoint {
                        step: 8,
                        value: 0.75,
                    },
                ],
            },
        });
        let mod_id = project.modulations.iter().map(|m| m.id).max().unwrap_or(0) + 1;
        project.modulations.push(Modulation {
            id: mod_id,
            name: "Ch0 Fade".into(),
            pattern: pat_id,
            target: ModulationTarget::ChannelGain {
                channel: ChannelRef::Channel(target_ch),
            },
        });

        let mut app = App::new(project, "demo.json".into()).unwrap();
        app.press_keys("gm").unwrap();

        let snap_mid = AudioSnapshot {
            playing: true,
            absolute_step: 4,
            step_fraction: 0.0,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap_mid, "silent");
        let mixer = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Mixer(m) => Some(m),
                _ => None,
            })
            .expect("MixerDeckView");
        let ch = mixer
            .channels
            .iter()
            .find(|c| c.channel == ChannelRef::Channel(target_ch))
            .expect("target channel");
        // Midpoint of 0.25 -> 0.75 ramp at step 4 is 0.50; 0.8 * 0.50 = 0.40!
        assert!((ch.automated_gain_scale.unwrap() - 0.5).abs() < 1e-3);
        assert!((ch.effective_gain.unwrap() - 0.4).abs() < 1e-3);
        assert!(ch.effective_gain_db.is_some());

        // Unmodulated channel has None for automated gain fields.
        let master = mixer
            .channels
            .iter()
            .find(|c| c.channel == ChannelRef::Master)
            .expect("master channel");
        assert!(master.automated_gain_scale.is_none());
        assert!(master.effective_gain.is_none());
    }

    #[test]
    fn mixer_view_preserves_plus_6db_headroom_for_macro_and_scaled_boosted_strips() {
        use cadence_core::dsp::{db_to_lin, MAX_CHANNEL_GAIN};
        use cadence_core::macros::{MacroBinding, MacroControl};
        use cadence_core::mixer::ChannelParam;
        use cadence_core::params::ParamAddress;

        let mut project = Project::demo();
        let ch0_id = project.mixer.channels[0].id;
        let ch1_id = project.mixer.channels[1].id;
        let ch0_ref = ChannelRef::Channel(ch0_id);
        let ch1_ref = ChannelRef::Channel(ch1_id);

        // ch0: boosted strip (+3.5 dB = ~1.496) * ChannelGain scale (0.9) = ~1.347 > 1.0 (never clamped at 1.0!)
        let boost = db_to_lin(3.5);
        project.mixer.channels[0].strip.gain = boost;
        let pat_id = project.max_id() + 1;
        project.patterns.push(Pattern {
            id: pat_id,
            name: "Scale90".into(),
            kind: PatternKind::Values {
                unit: ValueUnit::Normalized,
                period: 16,
                mode: ValueMode::Hold,
                points: vec![ValuePoint {
                    step: 0,
                    value: 0.9,
                }],
            },
        });
        let mod_id = project.max_id() + 2;
        project.modulations.push(Modulation {
            id: mod_id,
            name: "Ch0 Scale".into(),
            pattern: pat_id,
            target: ModulationTarget::ChannelGain { channel: ch0_ref },
        });

        // ch1: static Macro driving ChannelParam::Gain to norm 1.0 (+6.02 dB = 2.0) without ChannelGain scale.
        project.macros.push(MacroControl {
            index: 0,
            name: "HotBoost".into(),
            value: 1.0,
            bindings: vec![MacroBinding::new(
                ParamAddress::Channel {
                    channel: ch1_ref,
                    param: ChannelParam::Gain,
                },
                0.0,
                1.0,
            )],
        });

        let mut app = App::new(project, "headroom.json".into()).unwrap();
        app.press_keys("gm").unwrap();
        let snap = AudioSnapshot {
            playing: true,
            absolute_step: 2,
            step_fraction: 0.0,
            sample_rate: 48_000,
            ..AudioSnapshot::default()
        };
        let fv = app.frame_view(&snap, "silent");
        let mixer = fv
            .decks
            .iter()
            .find_map(|d| match &d.view {
                DeckView::Mixer(m) => Some(m),
                _ => None,
            })
            .expect("MixerDeckView");

        let v0 = mixer
            .channels
            .iter()
            .find(|c| c.channel == ch0_ref)
            .unwrap();
        assert!((v0.effective_gain.unwrap() - boost * 0.9).abs() < 1e-3);
        assert!(v0.effective_gain.unwrap() > 1.3);

        let v1 = mixer
            .channels
            .iter()
            .find(|c| c.channel == ch1_ref)
            .unwrap();
        assert!(v1.automated_gain_scale.is_none());
        assert!((v1.effective_gain.unwrap() - MAX_CHANNEL_GAIN).abs() < 1e-4);
        assert_eq!(v1.effective_gain_db.as_deref(), Some("+6.0"));
    }

    #[test]
    fn mixer_bank_scroll_window_slides_smoothly_with_scrolloff_and_preserves_master_jump() {
        use super::{mixer_bank_scroll_window, mixer_detail_scroll_window};

        // 13 bank strips, 6 visible slots -> scrolloff = 1 (since visible_slots >= 4).
        assert_eq!(mixer_bank_scroll_window(Some(0), 13, 6, 0), 0);
        assert_eq!(mixer_bank_scroll_window(Some(4), 13, 6, 0), 0);
        // Reaching index 5 (rightmost visible slot) slides by 1 to expose lookahead index 6.
        assert_eq!(mixer_bank_scroll_window(Some(5), 13, 6, 0), 1);
        assert_eq!(mixer_bank_scroll_window(Some(6), 13, 6, 1), 2);
        // Stepping back keeps scroll until hitting leading scrolloff margin (first + 1).
        assert_eq!(mixer_bank_scroll_window(Some(4), 13, 6, 2), 2);
        assert_eq!(mixer_bank_scroll_window(Some(3), 13, 6, 2), 2);
        assert_eq!(mixer_bank_scroll_window(Some(2), 13, 6, 2), 1);
        // Clamped at max_first = 13 - 6 = 7 when at end of bank.
        assert_eq!(mixer_bank_scroll_window(Some(12), 13, 6, 2), 7);
        // Focusing pinned Master (`None`) preserves the current bank scroll without jumping.
        assert_eq!(mixer_bank_scroll_window(None, 13, 6, 2), 2);
        // Narrow viewport (< 4 visible slots) uses 0 scrolloff (strict edge-following).
        assert_eq!(mixer_bank_scroll_window(Some(2), 10, 3, 0), 0);
        assert_eq!(mixer_bank_scroll_window(Some(3), 10, 3, 0), 1);

        // Detail vertical scroll slides smoothly edge-by-edge.
        assert_eq!(mixer_detail_scroll_window(None, 6, 3, 0), 0);
        assert_eq!(mixer_detail_scroll_window(Some(2), 6, 3, 0), 0);
        assert_eq!(mixer_detail_scroll_window(Some(3), 6, 3, 0), 1);
        assert_eq!(mixer_detail_scroll_window(Some(5), 6, 3, 1), 3);
        assert_eq!(mixer_detail_scroll_window(Some(3), 6, 3, 3), 3);
        assert_eq!(mixer_detail_scroll_window(Some(2), 6, 3, 3), 2);
    }

    #[test]
    fn mixer_view_tracks_smooth_bank_scroll_and_resets_detail_scroll_on_channel_switch() {
        let mut project = Project::demo();
        // Add 6 extra audio channels so we have > 6 bank channels + Master.
        for i in 0..6 {
            let id = project.max_id() + 1;
            project
                .mixer
                .channels
                .push(cadence_core::mixer::Channel::new(
                    id,
                    cadence_core::mixer::ChannelKind::Audio,
                    format!("Aux {i}"),
                ));
        }
        // Add 4 FX inserts onto channel 0 (+ OUT = 5 detail rows > 3 visible).
        let ch0_id = project.mixer.channels[0].id;
        project.mixer.channels[0].inserts.clear();
        for _ in 0..4 {
            let dev_id = project.max_id() + 1;
            project.mixer.channels[0]
                .inserts
                .push(cadence_core::mixer::EffectDevice::eq(dev_id));
        }

        let mut app = App::new(project, "scroll.json".into()).unwrap();
        app.press_keys("gm").unwrap();
        app.view_state.mixer_page_size = 5;
        app.view_state.mixer_detail_rows = 3;
        app.selected_channel = ChannelRef::Channel(ch0_id);

        let snap = AudioSnapshot::default();
        let get_mixer = |a: &App| -> super::MixerDeckView {
            a.frame_view(&snap, "silent")
                .decks
                .into_iter()
                .find_map(|d| match d.view {
                    DeckView::Mixer(m) => Some(m),
                    _ => None,
                })
                .expect("MixerDeckView")
        };

        // Step down into channel 0 detail slots past 3 visible rows -> detail_scroll advances cleanly.
        assert_eq!(get_mixer(&app).detail_scroll, 0);
        app.press_keys("jjjjjj").unwrap(); // Fader -> Pan -> FX0 -> FX1 -> FX2 -> FX3 -> OUT (row 4)
        assert_eq!(get_mixer(&app).detail_scroll, 2);

        // Jump straight to Master with `$`: bank_scroll stays 0 while Master is selected, detail_scroll resets!
        app.press_keys("$").unwrap();
        let m_master = get_mixer(&app);
        assert_eq!(
            m_master.channels[m_master.selected_index].role,
            super::MixerRole::Master
        );
        assert_eq!(m_master.bank_scroll, 0);
        assert_eq!(m_master.detail_scroll, 0);

        // Press `h` from Master: lands on last non-Master strip and slides bank to the right end!
        app.press_keys("h").unwrap();
        let m_end = get_mixer(&app);
        let bank_len = m_end.channels.len() - 1;
        assert_eq!(m_end.selected_index, bank_len - 1);
        assert_eq!(m_end.bank_scroll, bank_len - 5);

        // Jump back to start (`0`) -> bank_scroll returns cleanly to 0.
        app.press_keys("0").unwrap();
        let m_start = get_mixer(&app);
        assert_eq!(m_start.selected_index, 0);
        assert_eq!(m_start.bank_scroll, 0);
    }
}
