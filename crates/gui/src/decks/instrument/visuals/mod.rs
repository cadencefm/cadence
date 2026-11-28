//! GPU visualizers for all `ModuleVisualView` variants (`PinnedOverview`, `Vco`, `Sampler`,
//! `Noise`, `Env`, `Lfo`, `Filter`, `Vca`, `Drive`, `LoFi`, `Resonator`, `Out`).
mod plots;

use self::plots::{
    draw_bipolar_transfer_plot, draw_env_plot, draw_filter_plot, draw_noise_band_plot,
    draw_out_summary_card, draw_resonator_plot, draw_sampler_plot, draw_vca_transfer_plot,
    draw_wave_plot, OutSummarySpec, SamplerPlotSpec,
};
use super::{
    draw_badge_header, draw_card_panel, draw_live_scope_card, draw_simple_card_header,
    module_tag_accent, split_main_and_scope,
};
use crate::display::{DisplayList, Rect};
use crate::theme::{Color, Theme};
use cadence_app::view::ModuleVisualView;
use cadence_core::mixer::DriveMode;
use cadence_core::params::{format_hz, format_pan};
use cadence_core::rack::ModuleKindTag;
use cadence_core::sampler::SampleLoopMode;

pub(super) fn draw_module_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &ModuleVisualView,
    theme: &Theme,
) {
    if area.w < 120.0 || area.h < 48.0 {
        return;
    }
    match visual {
        ModuleVisualView::PinnedOverview { .. } => draw_pinned_overview(dl, area, visual, theme),
        ModuleVisualView::Vco { .. } => draw_vco_visual(dl, area, visual, theme),
        ModuleVisualView::Sampler { .. } => draw_sampler_visual(dl, area, visual, theme),
        ModuleVisualView::Noise { .. } => draw_noise_visual(dl, area, visual, theme),
        ModuleVisualView::Env { .. } => draw_env_visual(dl, area, visual, theme),
        ModuleVisualView::Lfo { .. } => draw_lfo_visual(dl, area, visual, theme),
        ModuleVisualView::Filter { .. } => draw_filter_visual(dl, area, visual, theme),
        ModuleVisualView::Vca { .. } => draw_vca_visual(dl, area, visual, theme),
        ModuleVisualView::Drive { .. } => draw_drive_visual(dl, area, visual, theme),
        ModuleVisualView::LoFi { .. } => draw_lofi_visual(dl, area, visual, theme),
        ModuleVisualView::Resonator { .. } => draw_resonator_visual(dl, area, visual, theme),
        ModuleVisualView::Out { .. } => draw_out_visual(dl, area, visual, theme),
    }
}

/// Shared primary card + optional right-hand live dry scope layout used by module visualizers.
fn draw_module_card_with_scope(
    dl: &mut DisplayList,
    area: Rect,
    live_waveform: &[f32],
    theme: &Theme,
    draw_main: impl FnOnce(&mut DisplayList, Rect, Rect),
) {
    let (main_card, scope_card) = split_main_and_scope(area);
    let inner = draw_card_panel(dl, main_card, theme);
    let (hdr, plot) = inner.split_top(18.0);
    draw_main(dl, hdr, plot);
    if let Some(sc) = scope_card {
        draw_live_scope_card(dl, sc, "DRY SCOPE", None, live_waveform, theme);
    }
}

fn draw_pinned_overview(
    dl: &mut DisplayList,
    area: Rect,
    visual: &ModuleVisualView,
    theme: &Theme,
) {
    let ModuleVisualView::PinnedOverview {
        pin_count,
        osc_label,
        osc_curve,
        filter_label,
        filter_curve,
        filter_cutoff_norm,
        filter_mod_cutoff_norm,
        env_label,
        env_curve,
        live_waveform,
    } = visual
    else {
        return;
    };

    #[derive(Clone, Copy)]
    enum Slot {
        Osc,
        Filter,
        Env,
        Scope,
    }
    let mut slots = Vec::with_capacity(4);
    if osc_label.is_some() && osc_curve.len() >= 2 {
        slots.push(Slot::Osc);
    }
    if filter_label.is_some() && filter_curve.len() >= 2 {
        slots.push(Slot::Filter);
    }
    if env_label.is_some() && env_curve.len() >= 2 {
        slots.push(Slot::Env);
    }
    slots.push(Slot::Scope);

    let gap = 6.0;
    let count = slots.len();
    let card_w = (area.w - gap * count.saturating_sub(1) as f32) / count as f32;
    let pin_badge = (*pin_count > 0).then(|| format!("{pin_count} PINS"));

    for (idx, slot) in slots.into_iter().enumerate() {
        let rect = Rect::new(area.x + idx as f32 * (card_w + gap), area.y, card_w, area.h);
        match slot {
            Slot::Osc => {
                let inner = draw_card_panel(dl, rect, theme);
                let (hdr, plot) = inner.split_top(16.0);
                draw_simple_card_header(
                    dl,
                    hdr,
                    osc_label.as_deref().unwrap_or("VCO"),
                    None,
                    theme.accent.routing,
                    theme,
                );
                draw_wave_plot(
                    dl,
                    plot,
                    osc_curve,
                    [-1.2, 1.2],
                    theme.accent.routing,
                    true,
                    theme,
                );
            }
            Slot::Filter => {
                let inner = draw_card_panel(dl, rect, theme);
                let (hdr, plot) = inner.split_top(16.0);
                draw_simple_card_header(
                    dl,
                    hdr,
                    filter_label.as_deref().unwrap_or("FILTER"),
                    None,
                    theme.accent.warning_key,
                    theme,
                );
                draw_filter_plot(
                    dl,
                    plot,
                    filter_curve,
                    *filter_cutoff_norm,
                    *filter_mod_cutoff_norm,
                    false,
                    theme,
                );
            }
            Slot::Env => {
                let inner = draw_card_panel(dl, rect, theme);
                let (hdr, plot) = inner.split_top(16.0);
                draw_simple_card_header(
                    dl,
                    hdr,
                    env_label.as_deref().unwrap_or("ENV"),
                    None,
                    theme.accent.playhead,
                    theme,
                );
                draw_env_plot(dl, plot, env_curve, theme);
            }
            Slot::Scope => {
                draw_live_scope_card(
                    dl,
                    rect,
                    "DRY SCOPE",
                    pin_badge.as_deref(),
                    live_waveform,
                    theme,
                );
            }
        }
    }
}

fn draw_vco_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Vco {
        label,
        waveform_label,
        curve,
        fm_active,
        unison_voices,
        active,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let wave_col = if !*active {
            theme.text.muted
        } else if *fm_active {
            theme.accent.modulation
        } else {
            theme.accent.routing
        };
        let uni_text = format!("UNI {unison_voices}x");
        let mut badges: Vec<(&str, Color)> = vec![(waveform_label, wave_col)];
        if *fm_active {
            badges.push(("FM", theme.accent.modulation));
        }
        if *unison_voices > 1 {
            badges.push((&uni_text, theme.accent.warning_key));
        }
        if !*active {
            badges.push(("MUTED", theme.text.muted));
        }
        draw_badge_header(dl, hdr, &badges, label, None, wave_col, theme);
        draw_wave_plot(dl, plot, curve, [-1.2, 1.2], wave_col, *active, theme);
    });
}

fn draw_sampler_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Sampler {
        name,
        sample_name,
        duration_sec,
        peaks,
        frames,
        sample_start,
        sample_end,
        loop_mode,
        loop_start,
        loop_end,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = if sample_name.is_some() {
            theme.accent.focus
        } else {
            theme.accent.alert
        };
        let mut badges: Vec<(&str, Color)> = vec![("SMP", accent)];
        match loop_mode {
            SampleLoopMode::Off => {}
            SampleLoopMode::Forward => badges.push(("FWD LOOP", theme.accent.playhead)),
            SampleLoopMode::PingPong => badges.push(("PING-PONG", theme.accent.playhead)),
        }
        let title = match sample_name {
            Some(sn) => format!("{name}  ·  {sn}"),
            None => format!("{name}  ·  NO SAMPLE  ·  press B to browse"),
        };
        let right_sum = sample_name
            .as_ref()
            .map(|_| format!("{duration_sec:.2}s · {frames} fr"));
        draw_badge_header(
            dl,
            hdr,
            &badges,
            &title,
            right_sum.as_deref(),
            accent,
            theme,
        );
        draw_sampler_plot(
            dl,
            plot,
            &SamplerPlotSpec {
                peaks,
                sample_start: *sample_start,
                sample_end: *sample_end,
                loop_mode: *loop_mode,
                loop_start: *loop_start,
                loop_end: *loop_end,
            },
            theme,
        );
    });
}

fn draw_noise_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Noise {
        name,
        color_label,
        highpass_hz,
        lowpass_hz,
        band_curve,
        highpass_norm,
        lowpass_norm,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = module_tag_accent(ModuleKindTag::Noise, theme);
        let badges = [("NOISE", accent), (*color_label, theme.accent.routing)];
        let right = format!(
            "HP {} · LP {}",
            format_hz(*highpass_hz),
            format_hz(*lowpass_hz)
        );
        draw_badge_header(dl, hdr, &badges, name, Some(&right), accent, theme);
        draw_noise_band_plot(
            dl,
            plot,
            band_curve,
            *highpass_norm,
            *lowpass_norm,
            accent,
            theme,
        );
    });
}

fn draw_env_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Env {
        label,
        mode_label,
        curve,
        bursts,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = theme.accent.playhead;
        let burst_txt = format!("{bursts}x BURST");
        let mut badges: Vec<(&str, Color)> = vec![("ENV", accent), (*mode_label, accent)];
        if *bursts > 1 {
            badges.push((&burst_txt, theme.accent.warning_key));
        }
        draw_badge_header(dl, hdr, &badges, label, None, accent, theme);
        draw_env_plot(dl, plot, curve, theme);
    });
}

fn draw_lfo_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Lfo {
        name,
        shape_label,
        bipolar,
        sync,
        rate_label,
        curve,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = theme.accent.modulation;
        let badges = [
            ("LFO", accent),
            (*shape_label, accent),
            (
                if *bipolar { "BIPOLAR" } else { "UNIPOLAR" },
                theme.accent.routing,
            ),
            (
                if *sync { "SYNC" } else { "FREE" },
                if *sync {
                    theme.accent.focus
                } else {
                    theme.text.muted
                },
            ),
        ];
        draw_badge_header(dl, hdr, &badges, name, Some(rate_label), accent, theme);
        let bounds = if *bipolar {
            [-1.15, 1.15]
        } else {
            [-0.08, 1.12]
        };
        draw_wave_plot(dl, plot, curve, bounds, accent, true, theme);
    });
}

fn draw_filter_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Filter {
        label,
        mode_label,
        curve,
        cutoff_norm,
        mod_cutoff_norm,
        kerr_semitones,
        spread,
        mix,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = theme.accent.warning_key;
        let kerr_txt = format!("KERR {:+.1}st", kerr_semitones);
        let spread_txt = format!("SPR {:.0}%", spread * 100.0);
        let mix_txt = format!("MIX {:.0}%", mix * 100.0);
        let mut badges: Vec<(&str, Color)> = vec![("FLT", accent), (*mode_label, accent)];
        if kerr_semitones.abs() >= 0.05 {
            badges.push((kerr_txt.as_str(), theme.accent.modulation));
        }
        if *spread >= 0.005 {
            badges.push((spread_txt.as_str(), theme.accent.focus));
        }
        badges.push((mix_txt.as_str(), theme.accent.routing));
        draw_badge_header(dl, hdr, &badges, label, None, accent, theme);
        draw_filter_plot(dl, plot, curve, *cutoff_norm, *mod_cutoff_norm, true, theme);
    });
}

fn draw_vca_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Vca {
        name,
        gain,
        bias,
        cv_patched,
        ring_mod,
        transfer_curve,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let (mode_tag, accent) = if *ring_mod {
            ("RING MOD", theme.accent.modulation)
        } else if *cv_patched {
            ("CV", theme.accent.playhead)
        } else {
            ("THRU", theme.text.muted)
        };
        let right = format!("gain {:.0}% · bias {:.0}%", gain * 100.0, bias * 100.0);
        draw_badge_header(
            dl,
            hdr,
            &[("VCA", accent), (mode_tag, accent)],
            name,
            Some(&right),
            accent,
            theme,
        );
        draw_vca_transfer_plot(dl, plot, transfer_curve, *ring_mod, accent, theme);
    });
}

fn draw_drive_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Drive {
        label,
        mode,
        transfer_curve,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = match *mode {
            DriveMode::AtomicJet => theme.accent.warning_key.mix(theme.accent.edit, 0.35),
            DriveMode::ShockPort => theme.accent.alert,
        };
        draw_badge_header(
            dl,
            hdr,
            &[("DRV", accent), (mode.label(), accent)],
            label,
            None,
            accent,
            theme,
        );
        draw_bipolar_transfer_plot(dl, plot, transfer_curve, [-1.5, 1.5], accent, theme);
    });
}

fn draw_lofi_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::LoFi {
        name,
        bits,
        rate_hz,
        mix,
        transfer_curve,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = module_tag_accent(ModuleKindTag::LoFi, theme);
        let bits_txt = format!("{bits:.1}b");
        let rate_txt = format_hz(*rate_hz);
        let mix_txt = format!("MIX {:.0}%", mix * 100.0);
        let badges = [
            ("LOFI", accent),
            (bits_txt.as_str(), theme.accent.warning_key),
            (rate_txt.as_str(), theme.accent.routing),
            (mix_txt.as_str(), theme.accent.focus),
        ];
        draw_badge_header(dl, hdr, &badges, name, None, accent, theme);
        draw_bipolar_transfer_plot(dl, plot, transfer_curve, [-1.2, 1.2], accent, theme);
    });
}

fn draw_resonator_visual(
    dl: &mut DisplayList,
    area: Rect,
    visual: &ModuleVisualView,
    theme: &Theme,
) {
    let ModuleVisualView::Resonator {
        name,
        mode_label,
        base_hz,
        decay_ms,
        brightness,
        mix,
        modes,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    draw_module_card_with_scope(dl, area, live_waveform, theme, |dl, hdr, plot| {
        let accent = module_tag_accent(ModuleKindTag::Resonator, theme);
        let freq_txt = format_hz(*base_hz);
        let dec_txt = if *decay_ms >= 1000.0 {
            format!("{:.2}s", decay_ms * 0.001)
        } else {
            format!("{decay_ms:.0}ms")
        };
        let bri_txt = format!("BRI {:.0}%", brightness * 100.0);
        let mix_txt = format!("MIX {:.0}%", mix * 100.0);
        let badges = [
            ("RES", accent),
            (*mode_label, theme.accent.focus),
            (freq_txt.as_str(), theme.accent.routing),
            (dec_txt.as_str(), theme.accent.playhead),
            (bri_txt.as_str(), theme.accent.warning_key),
            (mix_txt.as_str(), theme.text.secondary),
        ];
        draw_badge_header(dl, hdr, &badges, name, None, accent, theme);
        draw_resonator_plot(dl, plot, modes, accent, theme);
    });
}

fn draw_out_visual(dl: &mut DisplayList, area: Rect, visual: &ModuleVisualView, theme: &Theme) {
    let ModuleVisualView::Out {
        name,
        gain_db,
        pan,
        spread,
        voice_summary,
        live_waveform,
        ..
    } = visual
    else {
        return;
    };

    let (summary_card, scope_card) = if area.w >= 460.0 {
        let left_w = (area.w * 0.40).clamp(210.0, 380.0);
        let (l, r) = area.split_left(left_w);
        (
            Some(l),
            Rect::new(r.x + 6.0, r.y, (r.w - 6.0).max(0.0), r.h),
        )
    } else {
        (None, area)
    };
    if let Some(scard) = summary_card {
        draw_out_summary_card(
            dl,
            scard,
            &OutSummarySpec {
                label: name,
                gain_db: *gain_db,
                pan: *pan,
                spread: *spread,
                voice_summary,
            },
            theme,
        );
    }
    let right_badge = summary_card.is_none().then(|| {
        format!(
            "{name} · {gain_db:+.1} dB · {} · spread {:.0}%",
            format_pan(*pan),
            spread * 100.0
        )
    });
    draw_live_scope_card(
        dl,
        scope_card,
        "STEREO DRY SCOPE",
        right_badge.as_deref(),
        live_waveform,
        theme,
    );
}
