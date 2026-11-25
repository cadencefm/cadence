//! Top transport chrome bar, deck frames, breadcrumb trail, and bottom command/hints/status strip.
use crate::decks::output::{
    db_norm, dsp_cpu_color, fmt_db_fixed, METER_ZERO_DB_NORM, UNOBSERVED_DB_FIXED,
};
use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{ChromeView, UiModeKind};
use cadence_core::follow::FollowMode;
use cadence_core::runtime::CapturePhase;

#[allow(clippy::too_many_arguments)]
fn draw_chrome_pill(
    dl: &mut DisplayList,
    row: Rect,
    cursor_x: &mut f32,
    max_right: f32,
    small_cw: f32,
    label: &str,
    accent: Color,
    glow: f32,
    theme: &Theme,
) {
    let w = (label.chars().count() as f32 * small_cw + 16.0).max(48.0);
    if *cursor_x + w > max_right {
        return;
    }
    let rect = Rect::new(*cursor_x, row.y + 2.0, w, row.h - 4.0);
    dl.panel(
        rect,
        accent.with_alpha(0.22),
        accent,
        theme.radii.pill,
        1.0,
        glow,
    );
    dl.text(
        rect.inset(6.0, 0.0),
        label,
        theme.font.small,
        true,
        accent,
        TextAlign::Center,
    );
    *cursor_x += w + 8.0;
}

pub fn draw_header(dl: &mut DisplayList, area: Rect, chrome: &ChromeView, theme: &Theme) {
    dl.fill(area, theme.surface.header);
    // Crisp 1px hairline separator below header.
    dl.fill(
        Rect::new(area.x, area.bottom() - 1.0, area.w, 1.0),
        theme.border.hairline,
    );

    let inner = area.inset(12.0, 5.0);
    let (row1, row2) = inner.split_top(22.0);

    // Row 1: Brand + Project title + dirty badge on left; audio status + follow badge on right.
    let title = format!(
        "CADENCE  /  {}{}",
        chrome.title,
        if chrome.dirty { "  [+]" } else { "" }
    );
    dl.text(
        Rect::new(row1.x, row1.y, row1.w * 0.55, row1.h),
        title,
        theme.font.title,
        true,
        theme.accent.focus,
        TextAlign::Left,
    );

    let follow_color = match chrome.follow_mode {
        FollowMode::Off => theme.text.muted,
        FollowMode::Active => theme.accent.playhead,
        FollowMode::Suspended => theme.accent.warning_key,
    };
    let follow_w = 132.0;
    let follow_rect = Rect::new(
        row1.right() - follow_w,
        row1.y + 1.0,
        follow_w,
        row1.h - 2.0,
    );
    dl.panel(
        follow_rect,
        theme.surface.recessed,
        follow_color.with_alpha(0.55),
        theme.radii.badge,
        1.0,
        if chrome.follow_mode == FollowMode::Active {
            0.12
        } else {
            0.0
        },
    );
    dl.text(
        follow_rect.inset(6.0, 0.0),
        chrome.follow_mode.label(),
        theme.font.small,
        true,
        follow_color,
        TextAlign::Center,
    );

    let health_pill: Option<(String, Color, f32)> = if chrome.audio_health.failed {
        Some((
            "!AUDIO ERR".into(),
            theme.accent.alert,
            theme.glow.clip * 0.45,
        ))
    } else if chrome.audio_health.overloaded {
        Some(("!OVERLOAD".into(), theme.accent.warning_key, 0.28))
    } else if chrome.audio_health.xrun_count > 0 {
        Some((
            format!("XRUN {}", chrome.audio_health.xrun_count),
            theme.accent.warning_key,
            0.16,
        ))
    } else {
        None
    };
    let health_reserve = if let Some((label, accent, glow)) = health_pill {
        let pfont = theme.font.small - 1.0;
        let pw = (label.chars().count() as f32 * theme.char_width(pfont) + 16.0).clamp(64.0, 120.0);
        let prect = Rect::new(follow_rect.x - pw - 8.0, row1.y + 1.0, pw, row1.h - 2.0);
        dl.panel(
            prect,
            accent.with_alpha(0.22),
            accent,
            theme.radii.badge,
            1.0,
            glow,
        );
        dl.text(prect, label, pfont, true, accent, TextAlign::Center);
        pw + 10.0
    } else {
        0.0
    };

    let readiness = if chrome.ready {
        "ready"
    } else {
        "ASSETS MISSING"
    };
    let info = format!("{}  ·  {readiness}", chrome.audio_status);
    let info_left = row1.x + row1.w * 0.45;
    let info_w = (follow_rect.x - health_reserve - info_left - 10.0).max(0.0);
    let max_info_chars = (info_w / theme.char_width(theme.font.small)).floor() as usize;
    dl.text(
        Rect::new(info_left, row1.y, info_w, row1.h),
        crate::display::trim_label(&info, max_info_chars),
        theme.font.small,
        false,
        if chrome.ready {
            theme.text.muted
        } else {
            theme.accent.alert
        },
        TextAlign::Right,
    );

    // Row 2: Live transport status pill + Staging/REC pills + BPM + bar.beat.step + key/scale + loop + master dBFS.
    let play_badge_w = 24.0;
    let play_rect = Rect::new(row2.x, row2.y + 2.0, play_badge_w, row2.h - 4.0);
    let play_color = if chrome.playing {
        theme.accent.playhead
    } else {
        theme.text.muted
    };
    dl.rounded_glow(
        play_rect,
        play_color.with_alpha(if chrome.playing { 0.22 } else { 0.08 }),
        theme.radii.badge,
        if chrome.playing {
            theme.glow.playhead * 0.5
        } else {
            0.0
        },
    );
    dl.text(
        play_rect,
        if chrome.playing { "▶" } else { "■" },
        theme.font.small,
        true,
        play_color,
        TextAlign::Center,
    );

    let small_cw = theme.char_width(theme.font.small);
    let status_pill: Option<(&'static str, Color, f32, f32)> = if chrome.master_clipped {
        Some(("!CLIP", theme.meter.clip, theme.glow.clip * 0.45, 50.0))
    } else if chrome.master_hot {
        Some(("HOT", theme.meter.hot, 0.18, 40.0))
    } else {
        None
    };
    let status_pill_reserve = status_pill.map_or(0.0, |(_, _, _, w)| w + 6.0);
    // Exact fixed width for `"FINAL  L -118.2  R -118.2 dBFS"` (30 monospace glyphs) so numbers never jitter.
    let final_readout_w = (30.0 * small_cw + 4.0).ceil();
    let show_compact_meter = chrome.show_compact_meter && row2.w >= 860.0;
    let compact_meter_w = if show_compact_meter { 64.0 } else { 0.0 };
    let compact_meter_reserve = if show_compact_meter {
        compact_meter_w + 8.0
    } else {
        0.0
    };
    let show_dsp_cpu = chrome.show_dsp_cpu && row2.w >= 760.0;
    let cpu_pill_w = if show_dsp_cpu { 72.0 } else { 0.0 };
    let cpu_pill_reserve = if show_dsp_cpu { cpu_pill_w + 8.0 } else { 0.0 };
    let right_cluster_w =
        status_pill_reserve + final_readout_w + compact_meter_reserve + cpu_pill_reserve;
    let max_meta_right = (row2.right() - right_cluster_w - 8.0).max(row2.x + play_badge_w + 8.0);
    let mut cursor_x = row2.x + play_badge_w + 8.0;

    if let Some(badge) = &chrome.recording_badge {
        let (accent, glow) = match chrome.recording_phase {
            CapturePhase::Recording => (theme.accent.alert, 0.36),
            CapturePhase::Armed => (theme.accent.warning_key, 0.24),
            CapturePhase::Finishing => (theme.accent.routing, 0.22),
            CapturePhase::Idle => (theme.accent.focus, 0.14),
        };
        draw_chrome_pill(
            dl,
            row2,
            &mut cursor_x,
            max_meta_right,
            small_cw,
            badge,
            accent,
            glow,
            theme,
        );
        if chrome.input_active && cursor_x + 48.0 <= max_meta_right {
            let mw = 42.0_f32;
            let mrect = Rect::new(cursor_x, row2.y + 4.0, mw, (row2.h - 8.0).max(10.0));
            dl.panel(
                mrect,
                theme.surface.recessed,
                if chrome.input_clipped {
                    theme.accent.alert
                } else {
                    theme.border.hairline
                },
                theme.radii.badge,
                1.0,
                0.0,
            );
            let mi = mrect.inset(3.0, 2.0);
            let ch_h = ((mi.h - 1.5) * 0.5).max(2.0);
            for (idx, peak) in chrome.input_peak.into_iter().enumerate() {
                let by = mi.y + idx as f32 * (ch_h + 1.5);
                let track = Rect::new(mi.x, by, mi.w, ch_h);
                dl.fill(track, theme.meter.track_bg);
                let norm = db_norm(peak);
                let fw = track.w * norm;
                if fw > 0.5 {
                    let col = if chrome.input_clipped || peak >= 0.999 {
                        theme.meter.clip
                    } else if peak >= 0.70 {
                        theme.meter.hot
                    } else if peak >= 0.25 {
                        theme.meter.mid
                    } else {
                        theme.meter.low
                    };
                    dl.fill(Rect::new(track.x, track.y, fw, track.h), col);
                }
            }
            cursor_x += mw + 8.0;
        }
        if chrome.input_monitoring {
            draw_chrome_pill(
                dl,
                row2,
                &mut cursor_x,
                max_meta_right,
                small_cw,
                "◉ MON",
                theme.accent.playhead,
                0.16,
                theme,
            );
        }
    } else if chrome.input_monitoring {
        draw_chrome_pill(
            dl,
            row2,
            &mut cursor_x,
            max_meta_right,
            small_cw,
            "◉ IN MON",
            theme.accent.playhead,
            0.16,
            theme,
        );
    }

    let mut draw_status_pill = |label: String, accent: Color, glow: f32| {
        draw_chrome_pill(
            dl,
            row2,
            &mut cursor_x,
            max_meta_right,
            small_cw,
            &label,
            accent,
            glow,
            theme,
        );
    };
    if chrome.take_armed {
        if let Some(n) = chrome.take_recording {
            if chrome.take_capped {
                draw_status_pill(format!("● TAKE CAPPED ({n})"), theme.accent.alert, 0.36);
            } else if chrome.playing {
                draw_status_pill(format!("● REC TAKE ({n})"), theme.accent.alert, 0.30);
            } else {
                draw_status_pill(format!("● REC TAKE ({n})"), theme.accent.warning_key, 0.22);
            }
        } else if chrome.playing {
            draw_status_pill("● REC TAKE (0)".into(), theme.accent.alert, 0.30);
        } else {
            draw_status_pill("TAKE ARM".into(), theme.accent.warning_key, 0.18);
        }
    }
    if let Some(n) = chrome.staged_count {
        draw_status_pill(format!("◇ STAGED ({n})"), theme.accent.modulation, 0.30);
    }
    if chrome.midi_recording {
        draw_status_pill("● REC · NOTE+CC".into(), theme.accent.alert, 0.32);
        if let Some(target) = &chrome.midi_cc_last_recorded {
            let trimmed = crate::display::trim_label(target, 18);
            draw_status_pill(format!("CC● {trimmed}"), theme.accent.modulation, 0.20);
        }
    }
    if chrome.active_macro_count > 0 {
        let label = if chrome.active_macro_automated > 0 {
            format!(
                "◈ MACROS {} · {} MOD",
                chrome.active_macro_count, chrome.active_macro_automated
            )
        } else {
            format!("◈ MACROS {}", chrome.active_macro_count)
        };
        let accent = if chrome.active_macro_automated > 0 {
            theme.accent.modulation
        } else {
            theme.accent.focus
        };
        draw_status_pill(label, accent, 0.18);
    }

    if chrome.live_timing_cycle || chrome.live_edits_on_launch {
        let q_label = if chrome.live_edits_on_launch {
            format!("Q: {} · EDITS", chrome.live_timing_label)
        } else {
            format!("Q: {}", chrome.live_timing_label)
        };
        let accent = if chrome.live_timing_cycle {
            theme.accent.modulation
        } else {
            theme.accent.routing
        };
        draw_status_pill(crate::display::trim_label(&q_label, 24), accent, 0.20);
    }

    let mut meta = vec![
        chrome.source_label.to_string(),
        format!("{:.1} BPM", chrome.bpm),
        format!("{:03}.{}.{}", chrome.bar, chrome.beat, chrome.substep),
        format!("{} {}", chrome.root_note, chrome.scale_name),
        chrome.loop_label.clone(),
    ];
    if !(chrome.live_timing_cycle || chrome.live_edits_on_launch) {
        meta.push(format!("Q: {}", chrome.live_timing_label));
    }
    if chrome.override_count > 0 {
        meta.push(format!(
            "{} override{}",
            chrome.override_count,
            if chrome.override_count == 1 { "" } else { "s" }
        ));
    }
    if chrome.pending_count > 0 {
        meta.push(format!("{} pending", chrome.pending_count));
    }
    if let Some(end) = chrome.end_rule_label {
        meta.push(format!("end: {end}"));
    }
    if chrome.from_here {
        meta.push("' FROM HERE".into());
    }
    if chrome.jam_active {
        meta.push("JAM".into());
    }
    if chrome.midi_learning {
        meta.push("◉ MIDI LEARN".into());
    }
    if let Some(port) = &chrome.midi_port {
        let fb = if chrome.midi_out_port.is_some() {
            "↔"
        } else {
            ""
        };
        let last = chrome
            .midi_last
            .as_ref()
            .map_or_else(String::new, |m| format!(" {m}"));
        meta.push(format!("MIDI{fb} {port}{last}"));
    } else if let Some(out_port) = &chrome.midi_out_port {
        meta.push(format!("MIDI→ {out_port}"));
    }

    dl.text(
        Rect::new(
            cursor_x,
            row2.y,
            (max_meta_right - cursor_x).max(0.0),
            row2.h,
        ),
        meta.join("  ·  "),
        theme.font.body,
        false,
        theme.text.primary,
        TextAlign::Left,
    );

    let level_text = |peak: f32| -> String {
        if !chrome.output_observed {
            UNOBSERVED_DB_FIXED.into()
        } else {
            fmt_db_fixed(peak)
        }
    };
    let mut right_x = row2.right();
    if let Some((label, accent, glow, w)) = status_pill {
        let prect = Rect::new(right_x - w, row2.y + 2.0, w, row2.h - 4.0);
        dl.panel(
            prect,
            accent.with_alpha(0.22),
            accent,
            theme.radii.badge,
            1.0,
            glow,
        );
        dl.text(
            prect,
            label,
            theme.font.small - 1.0,
            true,
            accent,
            TextAlign::Center,
        );
        right_x = prect.x - 6.0;
    }

    let meter_str = format!(
        "FINAL  L {}  R {} dBFS",
        level_text(chrome.peak_left),
        level_text(chrome.peak_right),
    );
    let meter_col = if chrome.master_clipped {
        theme.meter.clip
    } else if chrome.master_hot {
        theme.meter.hot
    } else {
        theme.accent.routing
    };
    let final_rect = Rect::new(right_x - final_readout_w, row2.y, final_readout_w, row2.h);
    dl.text(
        final_rect,
        meter_str,
        theme.font.small,
        chrome.master_clipped,
        meter_col,
        TextAlign::Right,
    );
    right_x = final_rect.x;

    // Optional compact stereo bar meter pill beside FINAL (Auto when Output Scope hidden / Always / Off).
    if show_compact_meter {
        let mrect = Rect::new(
            right_x - 8.0 - compact_meter_w,
            row2.y + 2.0,
            compact_meter_w,
            (row2.h - 4.0).max(12.0),
        );
        let border = if chrome.master_clipped {
            theme.meter.clip
        } else if chrome.master_hot {
            theme.meter.hot
        } else {
            theme.border.hairline
        };
        dl.panel(
            mrect,
            theme.surface.recessed,
            border,
            theme.radii.badge,
            1.0,
            if chrome.master_clipped {
                theme.glow.clip * 0.25
            } else {
                0.0
            },
        );
        let mi = mrect.inset(4.0, 3.0);
        let ch_h = ((mi.h - 2.0) * 0.5).max(2.0);
        let zero_x = mi.x + mi.w * METER_ZERO_DB_NORM;
        for (idx, peak) in [chrome.peak_left, chrome.peak_right]
            .into_iter()
            .enumerate()
        {
            let by = mi.y + idx as f32 * (ch_h + 2.0);
            let track = Rect::new(mi.x, by, mi.w, ch_h);
            dl.fill(track, theme.meter.track_bg);
            if track.right() > zero_x + 0.5 {
                dl.fill(
                    Rect::new(zero_x, track.y, (track.right() - zero_x).max(0.0), track.h),
                    theme.meter.clip.with_alpha(0.14),
                );
            }
            if chrome.output_observed {
                let fw = (track.w * db_norm(peak)).clamp(0.0, track.w);
                if fw > 0.5 {
                    let col = if peak > 1.0 {
                        theme.meter.clip
                    } else if peak >= 0.5 {
                        theme.meter.high
                    } else {
                        theme.meter.mid
                    };
                    dl.fill(Rect::new(track.x, track.y, fw, track.h), col);
                }
            }
        }
        // Unity 0 dBFS hairline across both L/R bars.
        dl.fill(
            Rect::new(zero_x, mi.y - 0.5, 1.0, mi.h + 1.0),
            theme.meter.zero_db.with_alpha(0.65),
        );
        right_x = mrect.x;
    }

    // Fixed-width top-bar DSP CPU pill (e.g. `"DSP  14%"`, `"DSP 100%"`) + mini bottom bar & peak tick.
    if show_dsp_cpu {
        let crect = Rect::new(
            right_x - 8.0 - cpu_pill_w,
            row2.y + 2.0,
            cpu_pill_w,
            (row2.h - 4.0).max(12.0),
        );
        let health = chrome.audio_health;
        let has_cpu = chrome.output_observed || health.cpu_load > 0.0 || health.overloaded;
        let cpu_pct = (health.cpu_load.max(0.0) * 100.0).round().min(999.0);
        let cpu_col = if has_cpu {
            dsp_cpu_color(health.cpu_load, health.overloaded, theme)
        } else {
            theme.text.muted
        };
        let cpu_label = if has_cpu {
            format!("DSP {:3.0}%", cpu_pct)
        } else {
            "DSP  --%".into()
        };
        let warn = health.overloaded || health.cpu_load >= 0.70;
        dl.panel(
            crect,
            if warn {
                cpu_col.with_alpha(0.18)
            } else {
                theme.surface.recessed
            },
            if warn { cpu_col } else { theme.border.hairline },
            theme.radii.badge,
            1.0,
            if health.overloaded || health.cpu_load >= 0.85 {
                0.22
            } else {
                0.0
            },
        );
        dl.text(
            Rect::new(crect.x + 2.0, crect.y, crect.w - 4.0, crect.h - 3.0),
            cpu_label,
            theme.font.small - 1.0,
            warn,
            cpu_col,
            TextAlign::Center,
        );
        // Subtle 2px load bar + peak hold tick along the bottom inside edge of the pill.
        let bar = Rect::new(crect.x + 4.0, crect.bottom() - 3.5, crect.w - 8.0, 2.0);
        dl.fill(bar, theme.meter.track_bg);
        if has_cpu {
            let fw = bar.w * health.cpu_load.clamp(0.0, 1.0);
            if fw > 0.5 {
                dl.fill(Rect::new(bar.x, bar.y, fw, bar.h), cpu_col);
            }
            if health.peak_cpu_load > 0.01 {
                let px = (bar.x + bar.w * health.peak_cpu_load.clamp(0.0, 1.0) - 0.75)
                    .clamp(bar.x, bar.right() - 1.5);
                dl.fill(Rect::new(px, bar.y, 1.5, bar.h), theme.meter.hold_tick);
            }
        }
    }
}

pub fn draw_trail(dl: &mut DisplayList, area: Rect, trail: &[String], theme: &Theme) {
    if trail.is_empty() {
        return;
    }
    let text = format!("  {}", trail.join("  ›  "));
    dl.rounded(area, theme.surface.recessed, theme.radii.badge);
    dl.text(
        area.inset(6.0, 0.0),
        text,
        theme.font.small,
        false,
        theme.text.secondary,
        TextAlign::Left,
    );
}

pub fn draw_footer(dl: &mut DisplayList, area: Rect, chrome: &ChromeView, theme: &Theme) {
    dl.fill(area, theme.surface.header);
    dl.fill(
        Rect::new(area.x, area.y, area.w, 1.0),
        theme.border.hairline,
    );

    let inner = area.inset(10.0, 4.0);
    let (top_row, bot_row) = inner.split_top(26.0);

    // Mode pill on left of top row.
    let mode_text = match chrome.pending_key_count {
        Some(n) => format!("{} · {n}", chrome.mode_name),
        None => chrome.mode_name.to_string(),
    };
    let mode_w = (mode_text.chars().count() as f32 * theme.char_width(theme.font.small) + 20.0)
        .clamp(78.0, 220.0);
    let mode_rect = Rect::new(top_row.x, top_row.y + 2.0, mode_w, top_row.h - 4.0);
    let (mode_accent, mode_border_w, mode_glow) = match chrome.mode_kind {
        UiModeKind::Jam => (theme.accent.jam, 1.5, theme.glow.jam_border * 0.75),
        UiModeKind::Normal | UiModeKind::Overlay => match chrome.mode_name {
            "STEP INSERT" => (theme.accent.playhead, 1.4, 0.26),
            "VALUE ENTRY" | "MINI EDITOR" => {
                (theme.accent.edit, 1.4, theme.glow.edit_border * 0.65)
            }
            "STAGE" => (theme.accent.jam, 1.2, 0.22),
            "PALETTE" | "PRESETS" | "SONG PROPS" | "SEARCH" => {
                (theme.accent.warning_key, 1.2, 0.18)
            }
            _ => (theme.accent.focus, 1.0, 0.16),
        },
    };
    dl.panel(
        mode_rect,
        mode_accent.with_alpha(0.18),
        mode_accent,
        theme.radii.badge,
        mode_border_w,
        mode_glow,
    );
    dl.text(
        mode_rect,
        mode_text,
        theme.font.small,
        true,
        mode_accent,
        TextAlign::Center,
    );

    let right_rect = Rect::new(
        mode_rect.right() + 10.0,
        top_row.y,
        (top_row.w - mode_w - 10.0).max(0.0),
        top_row.h,
    );

    if let Some(search) = &chrome.search_line {
        dl.panel(
            right_rect.inset(0.0, 1.0),
            theme.surface.recessed,
            theme.accent.focus,
            theme.radii.badge,
            1.0,
            0.14,
        );
        dl.text(
            right_rect.inset(8.0, 0.0),
            format!("/{search}▏"),
            theme.font.body,
            true,
            theme.accent.focus,
            TextAlign::Left,
        );
    } else {
        // Contextual keyboard hints strip.
        let mut x = right_rect.x;
        let cw = theme.char_width(theme.font.small);
        for (key, desc) in &chrome.hints {
            let kw = key.chars().count() as f32 * cw + 10.0;
            let dw = desc.chars().count() as f32 * cw + 14.0;
            if x + kw + dw > right_rect.right() {
                break;
            }
            let krect = Rect::new(x, right_rect.y + 3.0, kw, right_rect.h - 6.0);
            dl.rounded(krect, theme.surface.elevated, theme.radii.badge);
            dl.text(
                krect,
                key.as_str(),
                theme.font.small,
                true,
                theme.accent.warning_key,
                TextAlign::Center,
            );
            dl.text(
                Rect::new(krect.right() + 5.0, right_rect.y, dw, right_rect.h),
                desc.as_str(),
                theme.font.small,
                false,
                theme.text.secondary,
                TextAlign::Left,
            );
            x += kw + dw + 4.0;
        }
    }

    // Bottom row: chain breadcrumb on left, status message on right.
    let (chain_rect, status_rect) = bot_row.split_left(bot_row.w * 0.44);
    dl.text(
        chain_rect,
        chrome.chain.as_str(),
        theme.font.small,
        false,
        theme.text.muted,
        TextAlign::Left,
    );
    dl.text(
        status_rect,
        chrome.status.as_str(),
        theme.font.small,
        chrome.status_alert,
        if chrome.status_alert {
            theme.accent.alert
        } else {
            theme.text.secondary
        },
        TextAlign::Right,
    );
}

/// Total horizontal logical pixel inset added by `draw_deck_frame` (`1.5*2 + 6.0*2`).
pub const DECK_FRAME_INSET_X: f32 = 15.0;
/// Total vertical logical pixel inset added by `draw_deck_frame` (`1.5*2 + 24.0 + 5.0*2`).
pub const DECK_FRAME_INSET_Y: f32 = 37.0;

/// Configuration for [`draw_deck_frame`].
#[derive(Clone, Copy, Debug)]
pub struct DeckFrameSpec<'a> {
    pub title: &'a str,
    pub right_badge: Option<&'a str>,
    pub accent: Color,
    pub focused: bool,
    pub mode: UiModeKind,
}

/// Draw a deck's outer panel + header strip, returning the inner content `Rect`.
///
/// Mode signaling is applied centrally here so every deck shares identical visual grammar:
/// - `NORMAL`: `accent` (cyan), `1.5px` border, subtle `focus_border` glow.
/// - `JAM`: `theme.accent.jam` (electric violet/magenta), `2.2px` glowing border (`jam_border`).
pub fn draw_deck_frame(
    dl: &mut DisplayList,
    area: Rect,
    spec: &DeckFrameSpec<'_>,
    theme: &Theme,
) -> Rect {
    let DeckFrameSpec {
        title,
        right_badge,
        accent,
        focused,
        mode,
    } = *spec;
    let is_jam = focused && mode == UiModeKind::Jam;
    let (border_col, border_w, glow, header_accent, header_alpha) = if is_jam {
        (
            theme.accent.jam,
            2.2,
            theme.glow.jam_border,
            theme.accent.jam,
            0.15,
        )
    } else if focused {
        (accent, 1.5, theme.glow.focus_border, accent, 0.11)
    } else {
        (theme.border.hairline, 1.0, 0.0, accent, 0.0)
    };
    dl.panel(
        area,
        theme.surface.elevated,
        border_col,
        theme.radii.panel,
        border_w,
        glow,
    );

    let header_h = 24.0;
    let inset_b = 1.5;
    let (hdr, body) = area.inset(inset_b, inset_b).split_top(header_h);
    dl.fill(
        hdr,
        if focused {
            header_accent.with_alpha(header_alpha)
        } else {
            theme.surface.header
        },
    );
    dl.fill(
        Rect::new(hdr.x, hdr.bottom() - 1.0, hdr.w, 1.0),
        if is_jam {
            theme.accent.jam.with_alpha(0.45)
        } else {
            theme.border.subtle
        },
    );

    let title_x = hdr.x + 8.0;

    let small_cw = theme.char_width(theme.font.small).max(1.0);
    let title_min_w = (title.chars().count() as f32 * small_cw + 8.0).min(hdr.w * 0.45);
    let max_badge_w = (hdr.right() - title_x - title_min_w - 14.0).max(0.0);
    let badge_w = right_badge.map_or(0.0, |b| {
        (b.chars().count() as f32 * small_cw + 16.0).min(max_badge_w)
    });
    let title_w = if right_badge.is_some() {
        (hdr.right() - badge_w - title_x - 14.0).max(0.0)
    } else {
        (hdr.right() - title_x - 8.0).max(0.0)
    };
    let title_max_chars = (title_w / small_cw).floor() as usize;
    let trimmed_title = crate::display::trim_label(title, title_max_chars);
    if !trimmed_title.is_empty() {
        dl.text(
            Rect::new(title_x, hdr.y, title_w, hdr.h),
            trimmed_title,
            theme.font.small,
            true,
            if focused {
                header_accent
            } else {
                theme.text.secondary
            },
            TextAlign::Left,
        );
    }
    if let Some(badge) = right_badge {
        let badge_max_chars = ((badge_w - 4.0).max(0.0) / small_cw).floor() as usize;
        let trimmed_badge = crate::display::trim_label(badge, badge_max_chars);
        if !trimmed_badge.is_empty() {
            dl.text(
                Rect::new(hdr.right() - badge_w - 6.0, hdr.y, badge_w, hdr.h),
                trimmed_badge,
                theme.font.small,
                false,
                theme.text.muted,
                TextAlign::Right,
            );
        }
    }

    body.inset(6.0, 5.0)
}

/// Semantic specification for a shared top deck mode tab bar (`Rack`, `Devices`).
pub struct DeckTabBarSpec<'a> {
    pub tabs: &'a [(&'static str, bool)],
    pub active_accent: Color,
    /// Optional trailing context badge rendered immediately after the tab pills (e.g. Rack zone badge).
    pub badge: Option<(&'a str, Color)>,
    /// Optional right-aligned contextual shortcut hint rendered when horizontal space allows.
    pub right_hint: Option<&'a str>,
}

/// Shared deck mode tab bar painter used by tabbed decks (`Instrument Rack`, `Devices`).
pub fn draw_deck_tab_bar(
    dl: &mut DisplayList,
    bar: Rect,
    spec: &DeckTabBarSpec<'_>,
    theme: &Theme,
) {
    if bar.is_empty() {
        return;
    }
    dl.push_clip(bar);
    let font = theme.font.small;
    let cw = theme.char_width(font);
    let pill_h = (bar.h - 6.0).clamp(16.0, 20.0);
    let pill_y = bar.y + 1.0;

    let mut tx = bar.x;
    for &(label, active) in spec.tabs {
        let tw = label.chars().count() as f32 * cw + 18.0;
        let tr = Rect::new(tx, pill_y, tw, pill_h);
        dl.panel(
            tr,
            if active {
                spec.active_accent.with_alpha(0.22)
            } else {
                theme.surface.recessed
            },
            if active {
                spec.active_accent
            } else {
                theme.border.subtle
            },
            theme.radii.badge,
            if active { 1.2 } else { 1.0 },
            if active { 0.16 } else { 0.0 },
        );
        dl.text(
            tr,
            label,
            font,
            active,
            if active {
                spec.active_accent
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
        tx += tw + 5.0;
    }

    if let Some((badge_text, badge_accent)) = spec.badge {
        if !badge_text.is_empty() {
            let zfont = theme.font.small - 1.0;
            let zcw = theme.char_width(zfont);
            let avail = (bar.right() - tx - 8.0).max(0.0);
            let zw = (badge_text.chars().count() as f32 * zcw + 16.0).min(avail.min(220.0));
            if zw >= 56.0 {
                tx += 4.0;
                let zr = Rect::new(tx, pill_y, zw, pill_h);
                dl.panel(
                    zr,
                    badge_accent.with_alpha(0.16),
                    badge_accent.with_alpha(0.75),
                    theme.radii.badge,
                    1.0,
                    0.0,
                );
                let max_c = ((zr.w - 10.0) / zcw).floor().max(4.0) as usize;
                dl.text(
                    zr.inset(5.0, 0.0),
                    crate::display::trim_label(badge_text, max_c),
                    zfont,
                    true,
                    badge_accent,
                    TextAlign::Center,
                );
                tx += zw + 6.0;
            }
        }
    }

    if let Some(hint) = spec.right_hint {
        let rem_w = (bar.right() - tx - 8.0).max(0.0);
        if rem_w >= 110.0 {
            let hfont = theme.font.small - 1.0;
            let hcw = theme.char_width(hfont);
            let max_c = (rem_w / hcw).floor().max(6.0) as usize;
            dl.text(
                Rect::new(tx + 6.0, pill_y, rem_w - 6.0, pill_h),
                crate::display::trim_label(hint, max_c),
                hfont,
                false,
                theme.text.muted,
                TextAlign::Right,
            );
        }
    }

    dl.fill(
        Rect::new(bar.x, bar.bottom() - 2.0, bar.w, 1.0),
        theme.border.subtle.with_alpha(0.7),
    );
    dl.pop_clip();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deck_frame_trims_long_right_badge_to_prevent_title_overlap() {
        let theme = Theme::cadence_dark();
        let small_cw = theme.char_width(theme.font.small);
        let long_badge =
            "Notes · SHARED DEFINITION · 320 steps · zoom 100% · ▶ PLAY step 84 · 2 uses · local exception";

        // Narrow pane: badge must be trimmed with an ellipsis and stay strictly to the right of title.
        let mut narrow_dl = DisplayList::new(420.0, 300.0, 1.0);
        draw_deck_frame(
            &mut narrow_dl,
            Rect::new(0.0, 0.0, 420.0, 240.0),
            &DeckFrameSpec {
                title: "Moving lead",
                right_badge: Some(long_badge),
                accent: theme.accent.focus,
                focused: true,
                mode: UiModeKind::Normal,
            },
            &theme,
        );
        assert_eq!(narrow_dl.base.texts.len(), 2);
        let title_run = &narrow_dl.base.texts[0];
        let badge_run = &narrow_dl.base.texts[1];
        assert_eq!(title_run.text, "Moving lead");
        assert!(badge_run.text.ends_with('…'));
        assert!(badge_run.text.chars().count() < long_badge.chars().count());
        let badge_rendered_w = badge_run.text.chars().count() as f32 * small_cw;
        assert!(
            badge_rendered_w <= badge_run.rect.w,
            "trimmed badge ({badge_rendered_w}px) must fit its rect ({}px)",
            badge_run.rect.w
        );
        assert!(
            title_run.rect.right() < badge_run.rect.x,
            "title right ({}) must remain strictly left of badge start ({})",
            title_run.rect.right(),
            badge_run.rect.x
        );

        // Wide pane: full badge fits untruncated.
        let mut wide_dl = DisplayList::new(1200.0, 300.0, 1.0);
        draw_deck_frame(
            &mut wide_dl,
            Rect::new(0.0, 0.0, 1200.0, 240.0),
            &DeckFrameSpec {
                title: "Moving lead",
                right_badge: Some(long_badge),
                accent: theme.accent.focus,
                focused: true,
                mode: UiModeKind::Normal,
            },
            &theme,
        );
        assert_eq!(wide_dl.base.texts.len(), 2);
        assert_eq!(wide_dl.base.texts[1].text, long_badge);
    }

    #[test]
    fn deck_tab_bar_renders_tabs_optional_badge_and_right_hint_with_balanced_clips() {
        let theme = Theme::cadence_dark();

        // Empty bar returns early without emitting any draw commands.
        let mut empty_dl = DisplayList::new(640.0, 200.0, 1.0);
        draw_deck_tab_bar(
            &mut empty_dl,
            Rect::new(0.0, 0.0, 0.0, 24.0),
            &DeckTabBarSpec {
                tabs: &[("Audio", true), ("MIDI", false)],
                active_accent: theme.accent.focus,
                badge: None,
                right_hint: Some("H/L tab"),
            },
            &theme,
        );
        assert!(empty_dl.base.texts.is_empty());
        assert!(empty_dl.base.commands.is_empty());

        // Wide 3-tab bar with trailing zone badge + right hint renders all items strictly left-to-right.
        let mut rack_dl = DisplayList::new(900.0, 200.0, 1.0);
        draw_deck_tab_bar(
            &mut rack_dl,
            Rect::new(10.0, 10.0, 860.0, 24.0),
            &DeckTabBarSpec {
                tabs: &[("Panel", true), ("Patchbay", false), ("Zones", false)],
                active_accent: theme.accent.focus,
                badge: Some(("Z1/4 Kick · C1–C2", theme.accent.routing)),
                right_hint: Some("H/L tab · h/l mod · j/k param"),
            },
            &theme,
        );
        assert_eq!(rack_dl.base.texts.len(), 5);
        assert_eq!(rack_dl.base.texts[0].text, "Panel");
        assert_eq!(rack_dl.base.texts[1].text, "Patchbay");
        assert_eq!(rack_dl.base.texts[2].text, "Zones");
        assert!(rack_dl.base.texts[3].text.contains("Z1/4"));
        assert!(rack_dl.base.texts[4].text.contains("H/L tab"));
        for pair in rack_dl.base.texts.windows(2) {
            assert!(
                pair[0].rect.right() <= pair[1].rect.x + 1.0,
                "tab bar items must not overlap: {:?} vs {:?}",
                pair[0].rect,
                pair[1].rect
            );
        }

        // Narrow 2-tab bar omits right hint cleanly when remaining width < 110px.
        let mut narrow_dl = DisplayList::new(260.0, 200.0, 1.0);
        draw_deck_tab_bar(
            &mut narrow_dl,
            Rect::new(0.0, 0.0, 220.0, 24.0),
            &DeckTabBarSpec {
                tabs: &[("Audio", false), ("MIDI", true)],
                active_accent: theme.accent.modulation,
                badge: None,
                right_hint: Some("H/L switch tab · r rescan"),
            },
            &theme,
        );
        assert_eq!(narrow_dl.base.texts.len(), 2);
        assert_eq!(narrow_dl.base.texts[0].text, "Audio");
        assert_eq!(narrow_dl.base.texts[1].text, "MIDI");
    }
}
