//! Unified GPU Devices Deck (`gd` / `Ctrl+d` — `Audio | MIDI` tabs):
//! - Top `Audio | MIDI` mode tab bar matching the Rack tab bar style 1:1.
//! - `Audio` tab: Navigable `AUDIO INTERFACE` + `ENGINE & RECOVERY` inspector (60%) alongside
//!   live `ENGINE HEALTH` telemetry card (40%: status pill, CPU meter + peak tick, xruns, active
//!   stream summary, and wrapped diagnostic/recovery guidance).
//! - `MIDI` tab: 8 RGB performance pad tiles + 8 Focused/Project Smart Macros, unified
//!   `DEVICE & PROFILE` + `MAPPINGS` inspector (with MIDI OUT error alert), and `LIVE MIDI MONITOR`.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::decks::output::{db_norm, db_to_meter_norm, dsp_cpu_color, fmt_db_fixed};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::{
    AudioDeckRowView, AudioDeckView, AudioInputView, DevicesTab, MidiDeckView, MidiDeviceRowView,
    MidiMappingRowView, UiModeKind,
};
use cadence_core::runtime::{AudioInputStatus, AudioStreamStatus, CapturePhase};

pub fn draw_midi(
    dl: &mut DisplayList,
    area: Rect,
    view: &MidiDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let (title, subtitle, accent) = match view.active_tab {
        DevicesTab::Audio => {
            let sub = match &view.audio.active_output {
                Some(dev) => format!("{dev}  ·  {}", view.audio.status.label()),
                None => format!("{}  ·  Ctrl+s save", view.audio.status.label()),
            };
            let accent = if view.audio.failed {
                theme.accent.alert
            } else if view.audio.overloaded {
                theme.accent.warning_key
            } else {
                theme.accent.focus
            };
            ("DEVICES  ·  AUDIO", sub, accent)
        }
        DevicesTab::Midi => {
            let mut sub = if view.custom_profile_path.is_some() {
                format!("{} [CUSTOM]  ·  {}", view.profile_name, view.feedback_label)
            } else {
                format!("{}  ·  {}", view.profile_name, view.feedback_label)
            };
            if view.midi_out_error.is_some() {
                sub.push_str("  ·  !OUT ERR");
            }
            ("DEVICES  ·  MIDI", sub, theme.accent.modulation)
        }
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title,
            right_badge: Some(&subtitle),
            accent,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 160.0 || inner.h < 90.0 {
        return;
    }
    dl.push_clip(inner);

    // Top mode tab bar (`Audio | MIDI`) matching Rack's `Panel | Patchbay | Zones` 1:1.
    let (tab_bar, rest) = inner.split_top(26.0);
    draw_devices_mode_bar(dl, tab_bar, view, accent, theme);

    let content = Rect::new(rest.x, rest.y + 4.0, rest.w, (rest.h - 4.0).max(0.0));
    if content.h >= 40.0 {
        match view.active_tab {
            DevicesTab::Audio => draw_audio_tab(dl, content, &view.audio, theme),
            DevicesTab::Midi => draw_midi_tab(dl, content, view, theme),
        }
    }

    dl.pop_clip();
}

fn draw_devices_mode_bar(
    dl: &mut DisplayList,
    bar: Rect,
    view: &MidiDeckView,
    active_accent: Color,
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
    for &(label, active) in &view.tabs {
        let tw = label.chars().count() as f32 * cw + 18.0;
        let tr = Rect::new(tx, pill_y, tw, pill_h);
        dl.panel(
            tr,
            if active {
                active_accent.with_alpha(0.22)
            } else {
                theme.surface.recessed
            },
            if active {
                active_accent
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
                active_accent
            } else {
                theme.text.secondary
            },
            TextAlign::Center,
        );
        tx += tw + 5.0;
    }

    // Compact alert indicator on the Audio tab pill area when MIDI tab is active and audio is alerted,
    // or right-aligned contextual shortcut hints when horizontal space allows.
    let hint = match view.active_tab {
        DevicesTab::Audio => {
            "H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/run · r rescan"
        }
        DevicesTab::Midi => {
            "H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/learn · a add · r rescan"
        }
    };
    let rem_w = (bar.right() - tx - 8.0).max(0.0);
    if rem_w >= 110.0 {
        let hfont = theme.font.small - 1.0;
        let hcw = theme.char_width(hfont);
        let max_c = (rem_w / hcw).floor().max(6.0) as usize;
        dl.text(
            Rect::new(tx + 6.0, pill_y, rem_w - 6.0, pill_h),
            trim_label(hint, max_c),
            hfont,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
    }

    dl.fill(
        Rect::new(bar.x, bar.bottom() - 2.0, bar.w, 1.0),
        theme.border.subtle.with_alpha(0.7),
    );
    dl.pop_clip();
}

fn draw_audio_tab(dl: &mut DisplayList, area: Rect, audio: &AudioDeckView, theme: &Theme) {
    let gap = 10.0;
    let inspector_w = if area.w >= 460.0 {
        (area.w * 0.60).clamp(280.0, (area.w - 200.0).max(280.0))
    } else {
        area.w
    };
    let inspector_rect = Rect::new(area.x, area.y, inspector_w.min(area.w), area.h);
    draw_audio_inspector_panel(dl, inspector_rect, audio, theme);

    let health_x = inspector_rect.right() + gap;
    let health_w = (area.right() - health_x).max(0.0);
    if health_w >= 180.0 {
        let health_rect = Rect::new(health_x, area.y, health_w, area.h);
        draw_engine_health_panel(dl, health_rect, audio, theme);
    }
}

enum AudioInspectorItem<'a> {
    Header { title: &'static str, accent: Color },
    Row(&'a AudioDeckRowView),
}

fn draw_audio_inspector_panel(
    dl: &mut DisplayList,
    area: Rect,
    audio: &AudioDeckView,
    theme: &Theme,
) {
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(8.0, 6.0);
    let (body, footer) = inner.split_bottom(18.0);
    let body = Rect::new(body.x, body.y, body.w, (body.h - 4.0).max(0.0));

    let hdr_h = 18.0_f32;
    let row_h = 30.0_f32;
    let mut items: Vec<(f32, f32, AudioInspectorItem<'_>)> =
        Vec::with_capacity(audio.rows.len() + 3);
    let mut y_acc = 0.0_f32;
    let mut sel_top = 0.0_f32;
    let mut sel_bot = row_h;
    let mut last_group: Option<&'static str> = None;

    for row in &audio.rows {
        let is_group_start = last_group != Some(row.group);
        let group_top = if is_group_start {
            if last_group.is_some() {
                y_acc += 8.0;
            }
            let top = y_acc;
            let accent = if row.group == "AUDIO INTERFACE" {
                theme.accent.focus
            } else if row.group == "AUDIO INPUT & RECORDING" {
                theme.accent.playhead
            } else {
                theme.accent.warning_key
            };
            items.push((
                y_acc,
                hdr_h,
                AudioInspectorItem::Header {
                    title: row.group,
                    accent,
                },
            ));
            y_acc += hdr_h + 3.0;
            last_group = Some(row.group);
            top
        } else {
            y_acc
        };
        if row.selected {
            sel_top = if is_group_start { group_top } else { y_acc };
            sel_bot = y_acc + row_h;
        }
        items.push((y_acc, row_h, AudioInspectorItem::Row(row)));
        y_acc += row_h + 3.0;
    }

    let total_h = y_acc;
    let max_scroll = (total_h - body.h).max(0.0);
    let scroll_y = if max_scroll <= 0.0 {
        0.0
    } else {
        let desired = ((sel_top + sel_bot) * 0.5 - body.h * 0.5).clamp(0.0, max_scroll);
        if sel_bot - desired > body.h {
            (sel_bot - body.h + 4.0).clamp(0.0, max_scroll)
        } else if sel_top < desired {
            sel_top.clamp(0.0, max_scroll)
        } else {
            desired
        }
    };

    let scrollbar_w = if max_scroll > 0.5 { 6.0 } else { 0.0 };
    let content_w = (body.w - scrollbar_w - if scrollbar_w > 0.0 { 4.0 } else { 0.0 }).max(80.0);
    let cw = theme.char_width(theme.font.small);

    dl.push_clip(body);
    for (iy, ih, item) in items {
        let y = body.y + iy - scroll_y;
        if y + ih < body.y - 2.0 || y > body.bottom() + 2.0 {
            continue;
        }
        let r = Rect::new(body.x, y, content_w, ih);
        match item {
            AudioInspectorItem::Header { title, accent } => {
                dl.text(
                    Rect::new(r.x + 2.0, r.y, r.w - 4.0, r.h),
                    title,
                    theme.font.small - 1.0,
                    true,
                    accent,
                    TextAlign::Left,
                );
            }
            AudioInspectorItem::Row(row) => {
                draw_audio_row(dl, r, row, cw, theme);
            }
        }
    }

    if max_scroll > 0.5 {
        let track = Rect::new(body.right() - scrollbar_w, body.y, scrollbar_w, body.h);
        dl.rounded(track, theme.meter.track_bg, 3.0);
        let frac = (body.h / total_h).clamp(0.12, 1.0);
        let thumb_h = (body.h * frac).max(18.0);
        let thumb_y = body.y + (scroll_y / max_scroll) * (body.h - thumb_h);
        dl.rounded(
            Rect::new(track.x, thumb_y, track.w, thumb_h),
            theme.accent.focus.with_alpha(0.65),
            3.0,
        );
    }
    dl.pop_clip();

    let pos_text = format!("{}/{}", audio.cursor + 1, audio.rows.len().max(1));
    let (hint_left, pos_right) = footer.split_right(48.0);
    let max_h = (hint_left.w / theme.char_width(theme.font.small - 1.0)).max(12.0) as usize;
    dl.text(
        hint_left,
        trim_label(
            "H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Alt+Bksp reset · Enter pick/run · r rescan · Ctrl+s save",
            max_h,
        ),
        theme.font.small - 1.0,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.text(
        pos_right,
        pos_text,
        theme.font.small - 1.0,
        false,
        theme.text.muted,
        TextAlign::Right,
    );
}

fn draw_audio_row(dl: &mut DisplayList, r: Rect, row: &AudioDeckRowView, cw: f32, theme: &Theme) {
    if row.selected {
        dl.panel(
            r,
            theme.surface.selection,
            theme.accent.focus.with_alpha(0.85),
            theme.radii.badge,
            1.1,
            0.12,
        );
    } else {
        dl.panel(
            r,
            theme.surface.elevated.with_alpha(0.60),
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
    }

    let lbl_w = 114.0_f32.min(r.w * 0.28);
    let val_w = (r.w * 0.35)
        .clamp(146.0, 240.0)
        .min((r.w - lbl_w - 20.0).max(80.0));
    let det_x = r.x + lbl_w + val_w + 18.0;
    let det_w = (r.right() - det_x - 6.0).max(0.0);

    dl.text(
        Rect::new(r.x + 8.0, r.y, lbl_w, r.h),
        trim_label(row.label, (lbl_w / cw).max(6.0) as usize),
        theme.font.small,
        true,
        if row.selected {
            theme.accent.focus
        } else {
            theme.text.secondary
        },
        TextAlign::Left,
    );

    let val_pill = Rect::new(r.x + lbl_w + 8.0, r.y + 4.0, val_w, (r.h - 8.0).max(16.0));
    let (pill_bg, pill_border, pill_text, pill_glow) = if row.actionable {
        let action_accent = if row.label == "Audio Panic" {
            theme.accent.alert
        } else if row.label == "Restart Audio" {
            if row.active {
                theme.accent.alert
            } else {
                theme.accent.warning_key
            }
        } else {
            theme.accent.routing
        };
        if row.selected || row.active {
            (
                action_accent.with_alpha(0.20),
                action_accent.with_alpha(0.90),
                action_accent,
                if row.selected { 0.14 } else { 0.08 },
            )
        } else {
            (
                action_accent.with_alpha(0.09),
                action_accent.with_alpha(0.45),
                theme.text.primary,
                0.0,
            )
        }
    } else if row.selected {
        (
            theme.accent.focus.with_alpha(0.18),
            theme.accent.focus.with_alpha(0.85),
            theme.accent.focus,
            0.10,
        )
    } else if row.active {
        (
            theme.accent.playhead.with_alpha(0.10),
            theme.accent.playhead.with_alpha(0.42),
            theme.text.primary,
            0.0,
        )
    } else {
        (
            theme.surface.recessed,
            theme.border.subtle,
            theme.text.secondary,
            0.0,
        )
    };

    dl.panel(
        val_pill,
        pill_bg,
        pill_border,
        theme.radii.badge,
        1.0,
        pill_glow,
    );
    let val_cw = theme.char_width(theme.font.small - 0.5);
    dl.text(
        val_pill.inset(6.0, 0.0),
        trim_label(&row.value, ((val_pill.w - 12.0) / val_cw).max(6.0) as usize),
        theme.font.small - 0.5,
        row.selected || row.active || row.actionable,
        pill_text,
        if row.actionable {
            TextAlign::Center
        } else {
            TextAlign::Left
        },
    );

    if det_w >= 64.0 {
        let det_cw = theme.char_width(theme.font.small - 1.5);
        dl.text(
            Rect::new(det_x, r.y, det_w, r.h),
            trim_label(&row.detail, (det_w / det_cw).max(8.0) as usize),
            theme.font.small - 1.5,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
    }
}

fn wrap_lines(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let width = max_chars.max(12);
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        let clen = cur.chars().count();
        if clen == 0 {
            cur.push_str(&trim_label(word, width));
        } else if clen + 1 + wlen <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(cur);
            if lines.len() >= max_lines {
                return lines;
            }
            cur = trim_label(word, width);
        }
    }
    if !cur.is_empty() && lines.len() < max_lines {
        lines.push(cur);
    }
    lines
}

fn draw_engine_health_panel(
    dl: &mut DisplayList,
    area: Rect,
    audio: &AudioDeckView,
    theme: &Theme,
) {
    let (status_accent, panel_border, panel_glow) = match audio.status {
        AudioStreamStatus::Running => (theme.accent.playhead, theme.border.subtle, 0.0),
        AudioStreamStatus::Silent => (theme.text.muted, theme.border.subtle, 0.0),
        AudioStreamStatus::Overloaded => (
            theme.accent.warning_key,
            theme.accent.warning_key.with_alpha(0.75),
            0.14,
        ),
        AudioStreamStatus::Failed => (
            theme.accent.alert,
            theme.accent.alert.with_alpha(0.85),
            0.18,
        ),
    };
    dl.panel(
        area,
        theme.surface.recessed,
        panel_border,
        theme.radii.panel,
        if panel_glow > 0.0 { 1.2 } else { 1.0 },
        panel_glow,
    );
    let inner = area.inset(10.0, 8.0);
    if inner.w < 100.0 || inner.h < 60.0 {
        return;
    }
    dl.push_clip(inner);

    // 1. Header row: ENGINE HEALTH + semantic status badge pill.
    let (hdr, rest) = inner.split_top(22.0);
    let badge_label = audio.status.label();
    let bfont = theme.font.small - 1.0;
    let bw = (badge_label.chars().count() as f32 * theme.char_width(bfont) + 16.0)
        .clamp(64.0, (hdr.w * 0.58).max(64.0));
    let badge_rect = Rect::new(hdr.right() - bw, hdr.y + 1.0, bw, hdr.h - 2.0);
    dl.text(
        Rect::new(hdr.x, hdr.y, (hdr.w - bw - 8.0).max(0.0), hdr.h),
        "ENGINE HEALTH",
        theme.font.small - 0.5,
        true,
        theme.accent.focus,
        TextAlign::Left,
    );
    dl.panel(
        badge_rect,
        status_accent.with_alpha(0.20),
        status_accent.with_alpha(0.90),
        theme.radii.badge,
        1.0,
        if matches!(
            audio.status,
            AudioStreamStatus::Overloaded | AudioStreamStatus::Failed
        ) {
            0.20
        } else {
            0.08
        },
    );
    dl.text(
        badge_rect.inset(4.0, 0.0),
        trim_label(
            badge_label,
            ((badge_rect.w - 8.0) / theme.char_width(bfont)).max(6.0) as usize,
        ),
        bfont,
        true,
        status_accent,
        TextAlign::Center,
    );

    let mut y = rest.y + 8.0;

    // 2. Live Audio CPU Meter card.
    let cpu_h = 58.0_f32;
    if y + cpu_h <= inner.bottom() {
        let cpu_rect = Rect::new(inner.x, y, inner.w, cpu_h);
        dl.panel(
            cpu_rect,
            theme.surface.elevated,
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
        let ci = cpu_rect.inset(8.0, 6.0);
        let cpu_pct = (audio.cpu_load.max(0.0) * 100.0).min(999.9);
        let peak_pct = (audio.peak_cpu_load.max(0.0) * 100.0).min(999.9);
        let cpu_color = dsp_cpu_color(audio.cpu_load, audio.overloaded, theme);
        let top_row = Rect::new(ci.x, ci.y, ci.w, 15.0);
        dl.text(
            top_row,
            "AUDIO CPU",
            theme.font.small - 1.0,
            true,
            theme.text.secondary,
            TextAlign::Left,
        );
        dl.text(
            top_row,
            format!("{cpu_pct:.1}%  ·  peak {peak_pct:.1}%"),
            theme.font.small - 1.0,
            true,
            cpu_color,
            TextAlign::Right,
        );

        let bar = Rect::new(ci.x, ci.y + 20.0, ci.w, 11.0);
        dl.rounded(bar, theme.meter.track_bg, 3.0);
        // Subtle 80% caution threshold tick.
        dl.fill(
            Rect::new(bar.x + bar.w * 0.80, bar.y, 1.0, bar.h),
            theme.border.hairline,
        );
        let fill_w = bar.w * audio.cpu_load.clamp(0.0, 1.0);
        if fill_w > 0.5 {
            dl.rounded_glow(
                Rect::new(bar.x, bar.y, fill_w, bar.h),
                cpu_color,
                3.0,
                if audio.cpu_load >= 0.70 { 0.30 } else { 0.16 },
            );
        }
        if audio.peak_cpu_load > 0.005 {
            let tick_x = (bar.x + bar.w * audio.peak_cpu_load.clamp(0.0, 1.0) - 1.0)
                .clamp(bar.x, bar.right() - 2.0);
            dl.fill(
                Rect::new(tick_x, bar.y - 1.0, 2.0, bar.h + 2.0),
                theme.meter.hold_tick,
            );
        }

        let scale_row = Rect::new(ci.x, ci.y + 34.0, ci.w, 12.0);
        let sfont = theme.font.small - 2.0;
        dl.text(
            scale_row,
            "0%",
            sfont,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        dl.text(
            scale_row,
            "50%",
            sfont,
            false,
            theme.text.muted,
            TextAlign::Center,
        );
        dl.text(
            scale_row,
            "100%",
            sfont,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
        y += cpu_h + 8.0;
    }

    // 3. Xruns & Active Stream telemetry summary card.
    let stream_h = 134.0_f32;
    if y + 72.0 <= inner.bottom() {
        let card_h = stream_h.min(inner.bottom() - y);
        let stream_rect = Rect::new(inner.x, y, inner.w, card_h);
        dl.panel(
            stream_rect,
            theme.surface.elevated,
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
        let si = stream_rect.inset(8.0, 6.0);
        let (shdr, sbody) = si.split_top(18.0);
        dl.text(
            shdr,
            "ACTIVE STREAM",
            theme.font.small - 1.0,
            true,
            theme.accent.routing,
            TextAlign::Left,
        );
        let xrun_accent = if audio.xrun_count > 0 {
            theme.accent.alert
        } else {
            theme.accent.playhead
        };
        let xrun_text = format!("XRUNS {}", audio.xrun_count);
        let xfont = theme.font.small - 1.5;
        let xw = (xrun_text.chars().count() as f32 * theme.char_width(xfont) + 12.0).max(58.0);
        let xrect = Rect::new(shdr.right() - xw, shdr.y, xw, 16.0);
        dl.panel(
            xrect,
            xrun_accent.with_alpha(0.16),
            xrun_accent.with_alpha(0.75),
            theme.radii.badge,
            1.0,
            0.0,
        );
        dl.text(
            xrect,
            xrun_text,
            xfont,
            true,
            xrun_accent,
            TextAlign::Center,
        );

        let device_str = match &audio.active_output {
            Some(name) => name.clone(),
            None if audio.silent_mode => "Silent clock".into(),
            None => "System Default".into(),
        };
        let buffer_str = cadence_app::format_buffer_latency(audio.buffer_frames, audio.sample_rate);
        let channels_str = match audio.channels {
            0 => "—".into(),
            1 => "1 ch (Mono)".into(),
            2 => "2 ch (Stereo)".into(),
            n => format!("{n} ch"),
        };
        let fields = [
            ("Device", device_str),
            ("Rate", cadence_app::format_sample_rate(audio.sample_rate)),
            ("Buffer", buffer_str),
            ("Format", audio.format_label.clone()),
            ("Channels", channels_str),
        ];
        let row_h = 20.0_f32;
        let lbl_w = 68.0_f32;
        let val_w = (sbody.w - lbl_w - 6.0).max(40.0);
        let vcw = theme.char_width(theme.font.small - 0.5);
        for (i, (lbl, val)) in fields.iter().enumerate() {
            let ry = sbody.y + 3.0 + i as f32 * row_h;
            if ry + row_h > sbody.bottom() + 1.0 {
                break;
            }
            dl.text(
                Rect::new(sbody.x, ry, lbl_w, row_h),
                *lbl,
                theme.font.small - 1.0,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
            dl.text(
                Rect::new(sbody.x + lbl_w + 6.0, ry, val_w, row_h),
                trim_label(val, (val_w / vcw).max(6.0) as usize),
                theme.font.small - 0.5,
                i == 0,
                theme.text.primary,
                TextAlign::Right,
            );
        }
        y += card_h + 8.0;
    }

    // 4. Live Audio Input Metering & Capture Telemetry card.
    let rem_h = inner.bottom() - y;
    let has_out_alert = audio.last_error.is_some() || audio.overloaded || audio.failed;
    let alert_reserve = if has_out_alert { 78.0 } else { 0.0 };
    let input_avail = rem_h - alert_reserve;
    if input_avail >= 78.0 {
        let input_h = 116.0_f32.min(input_avail);
        let input_rect = Rect::new(inner.x, y, inner.w, input_h);
        draw_audio_input_card(dl, input_rect, &audio.input, theme);
        y += input_h + 8.0;
    }

    // 5. Diagnostic Error / Overload Guidance Box (or quiet healthy summary when space permits).
    let rem_h = inner.bottom() - y;
    if has_out_alert {
        if rem_h >= 52.0 {
            let alert_col = if audio.failed {
                theme.accent.alert
            } else {
                theme.accent.warning_key
            };
            let msg_font = theme.font.small - 1.0;
            let text_w = (inner.w - 16.0).max(80.0);
            let max_c = (text_w / theme.char_width(msg_font)).floor().max(14.0) as usize;
            let body_text = audio.last_error.as_deref().unwrap_or(if audio.overloaded {
                "Callback execution exceeded hardware buffer budget; transport paused safely."
            } else {
                "OS audio stream stopped unexpectedly."
            });
            let wrapped = wrap_lines(body_text, max_c, 4);
            let ideal_h = 16.0 + 18.0 + wrapped.len() as f32 * 15.0 + 8.0 + 16.0;
            let box_rect = Rect::new(inner.x, y, inner.w, ideal_h.min(rem_h).max(52.0));
            dl.panel(
                box_rect,
                alert_col.with_alpha(0.14),
                alert_col.with_alpha(0.85),
                theme.radii.badge,
                1.1,
                0.15,
            );
            let bi = box_rect.inset(8.0, 6.0);
            let heading = if audio.failed {
                "! STREAM ERROR"
            } else if audio.overloaded {
                "! OVERLOAD PAUSED"
            } else {
                "! AUDIO DIAGNOSTIC"
            };
            dl.text(
                Rect::new(bi.x, bi.y, bi.w, 16.0),
                heading,
                theme.font.small - 0.5,
                true,
                alert_col,
                TextAlign::Left,
            );
            let mut ly = bi.y + 18.0;
            for line in wrapped {
                if ly + 14.0 > bi.bottom() - 16.0 {
                    break;
                }
                dl.text(
                    Rect::new(bi.x, ly, bi.w, 15.0),
                    line,
                    msg_font,
                    false,
                    theme.text.primary,
                    TextAlign::Left,
                );
                ly += 15.0;
            }
            let tip_y = (ly + 6.0).min(bi.bottom() - 16.0);
            if tip_y >= bi.y + 18.0 {
                dl.text(
                    Rect::new(bi.x, tip_y, bi.w, 16.0),
                    trim_label("Press Enter on Restart Audio or raise Buffer Size", max_c),
                    msg_font,
                    true,
                    alert_col,
                    TextAlign::Left,
                );
            }
        }
    } else if rem_h >= 36.0 {
        let nfont = theme.font.small - 1.0;
        let text_w = (inner.w - 16.0).max(80.0);
        let max_c = (text_w / theme.char_width(nfont)).floor().max(14.0) as usize;
        let line = if audio.silent_mode {
            "Silent clock active · switch Output or Restart Audio to open hardware stream"
        } else if audio.xrun_count == 0 {
            "Stream healthy · 0 callback overruns · Audio Panic silences tails instantly"
        } else {
            "Stream recovered · raise Buffer Size if xruns continue under heavy voices"
        };
        let wrapped = wrap_lines(line, max_c, 2);
        let ideal_h = 14.0 + wrapped.len() as f32 * 16.0;
        let note_rect = Rect::new(inner.x, y, inner.w, ideal_h.min(rem_h).max(32.0));
        dl.panel(
            note_rect,
            theme.surface.elevated.with_alpha(0.45),
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
        let ni = note_rect.inset(8.0, 6.0);
        for (idx, wrapped_line) in wrapped.into_iter().enumerate() {
            dl.text(
                Rect::new(ni.x, ni.y + idx as f32 * 16.0, ni.w, 16.0),
                wrapped_line,
                nfont,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
        }
    }

    dl.pop_clip();
}

fn draw_audio_input_card(dl: &mut DisplayList, area: Rect, input: &AudioInputView, theme: &Theme) {
    let (badge_label, badge_accent, glow) = match input.capture_phase {
        CapturePhase::Recording => (
            format!("● REC {:.1}s", input.captured_sec),
            theme.accent.alert,
            0.24,
        ),
        CapturePhase::Armed => ("○ ARM".into(), theme.accent.warning_key, 0.16),
        CapturePhase::Finishing => ("SAVING…".into(), theme.accent.routing, 0.12),
        CapturePhase::Idle => match input.status {
            AudioInputStatus::Running if input.digital_silence => {
                ("0.0 SILENCE".into(), theme.accent.warning_key, 0.14)
            }
            AudioInputStatus::Running => ("ACTIVE".into(), theme.accent.playhead, 0.08),
            AudioInputStatus::Failed => ("INPUT ERR".into(), theme.accent.alert, 0.18),
            AudioInputStatus::Inactive => ("INACTIVE".into(), theme.text.muted, 0.0),
        },
    };
    let card_border = if input.capture_phase == CapturePhase::Recording
        || input.status == AudioInputStatus::Failed
    {
        theme.accent.alert.with_alpha(0.75)
    } else if input.capture_phase == CapturePhase::Armed || input.digital_silence {
        theme.accent.warning_key.with_alpha(0.70)
    } else {
        theme.border.hairline
    };
    dl.panel(
        area,
        theme.surface.elevated,
        card_border,
        theme.radii.badge,
        1.0,
        if input.capture_phase == CapturePhase::Recording {
            0.10
        } else {
            0.0
        },
    );
    let inner = area.inset(8.0, 6.0);
    if inner.w < 80.0 || inner.h < 40.0 {
        return;
    }
    dl.push_clip(inner);

    // Header: AUDIO INPUT + optional MON pill + status pill.
    let (hdr, body) = inner.split_top(18.0);
    let bfont = theme.font.small - 1.5;
    let bcw = theme.char_width(bfont);
    let bw = (badge_label.chars().count() as f32 * bcw + 12.0).clamp(52.0, hdr.w * 0.50);
    let brect = Rect::new(hdr.right() - bw, hdr.y, bw, 16.0);
    dl.panel(
        brect,
        badge_accent.with_alpha(0.18),
        badge_accent.with_alpha(0.85),
        theme.radii.badge,
        1.0,
        glow,
    );
    dl.text(
        brect,
        trim_label(&badge_label, ((bw - 8.0) / bcw).max(5.0) as usize),
        bfont,
        true,
        badge_accent,
        TextAlign::Center,
    );

    let mut right_edge = brect.x - 4.0;
    if input.monitoring && right_edge - hdr.x >= 110.0 {
        let mw = 38.0_f32;
        let mrect = Rect::new(right_edge - mw, hdr.y, mw, 16.0);
        dl.panel(
            mrect,
            theme.accent.playhead.with_alpha(0.16),
            theme.accent.playhead.with_alpha(0.75),
            theme.radii.badge,
            1.0,
            0.06,
        );
        dl.text(
            mrect,
            "MON",
            bfont,
            true,
            theme.accent.playhead,
            TextAlign::Center,
        );
        right_edge = mrect.x - 4.0;
    }
    dl.text(
        Rect::new(hdr.x, hdr.y, (right_edge - hdr.x).max(40.0), 16.0),
        "AUDIO INPUT",
        theme.font.small - 1.0,
        true,
        theme.accent.playhead,
        TextAlign::Left,
    );

    let mut cy = body.y + 4.0;

    // Live stereo L/R input peak + hold meter bars.
    let zero_norm = db_to_meter_norm(0.0);
    let readout_w = 50.0_f32;
    let ch_lbl_w = 14.0_f32;
    let bar_x = body.x + ch_lbl_w + 2.0;
    let bar_w = (body.w - ch_lbl_w - readout_w - 8.0).max(40.0);
    let bar_h = 8.0_f32;

    for (ch, ch_lbl) in ["L", "R"].into_iter().enumerate() {
        if cy + 12.0 > inner.bottom() {
            break;
        }
        let peak = input.peak[ch];
        let hold = input.peak_hold[ch];
        dl.text(
            Rect::new(body.x, cy - 1.0, ch_lbl_w, 11.0),
            ch_lbl,
            theme.font.small - 2.0,
            true,
            theme.text.muted,
            TextAlign::Left,
        );
        let track = Rect::new(bar_x, cy + 1.0, bar_w, bar_h);
        dl.rounded(track, theme.meter.track_bg, 2.0);
        // 0 dBFS reference tick.
        dl.fill(
            Rect::new(track.x + track.w * zero_norm, track.y, 1.0, track.h),
            theme.border.hairline,
        );
        let norm = db_norm(peak);
        let fw = track.w * norm;
        if fw > 0.5 {
            let col = if input.clipped || peak >= 0.999 {
                theme.meter.clip
            } else if peak >= 0.70 {
                theme.meter.hot
            } else if peak >= 0.25 {
                theme.meter.mid
            } else {
                theme.meter.low
            };
            dl.rounded_glow(
                Rect::new(track.x, track.y, fw, track.h),
                col,
                2.0,
                if peak >= 0.70 { 0.22 } else { 0.10 },
            );
        }
        if hold > 1e-4 {
            let hx = (track.x + track.w * db_norm(hold) - 1.0).clamp(track.x, track.right() - 2.0);
            dl.fill(
                Rect::new(hx, track.y - 1.0, 2.0, track.h + 2.0),
                theme.meter.hold_tick,
            );
        }
        let db_str = if ch == 0 && input.clipped {
            format!("{:>6}", "CLIP!")
        } else {
            fmt_db_fixed(hold.max(peak))
        };
        dl.text(
            Rect::new(body.right() - readout_w, cy - 1.0, readout_w, 11.0),
            db_str,
            theme.font.small - 2.0,
            ch == 0 && input.clipped,
            if ch == 0 && input.clipped {
                theme.meter.clip
            } else {
                theme.text.secondary
            },
            TextAlign::Right,
        );
        cy += 13.0;
    }

    // Active input summary / diagnostic guidance lines.
    cy += 3.0;
    let mfont = theme.font.small - 1.5;
    let max_c = (body.w / theme.char_width(mfont)).floor().max(12.0) as usize;
    if let Some(err) = &input.last_error {
        for line in wrap_lines(err, max_c, 3) {
            if cy + 13.0 > inner.bottom() {
                break;
            }
            dl.text(
                Rect::new(body.x, cy, body.w, 13.0),
                line,
                mfont,
                false,
                theme.accent.alert,
                TextAlign::Left,
            );
            cy += 13.5;
        }
    } else if let Some(dev) = &input.active_input {
        let fmt_line = if input.sample_rate > 0 {
            format!(
                "{dev}  ·  {} · {} ch · {} fr",
                cadence_app::format_sample_rate(input.sample_rate),
                input.channels.max(1),
                input.buffer_frames
            )
        } else {
            format!("{dev}  ·  ready (select live audio stream)")
        };
        if cy + 14.0 <= inner.bottom() {
            dl.text(
                Rect::new(body.x, cy, body.w, 14.0),
                trim_label(&fmt_line, max_c),
                mfont,
                true,
                theme.text.primary,
                TextAlign::Left,
            );
            cy += 15.0;
        }
        let rt_str = match input.round_trip_latency_ms {
            Some(rt) => format!("RT ~{rt:.1} ms"),
            None => format!("Trim {:+.1} ms", input.latency_trim_ms),
        };
        let route_line = format!(
            "{}  ·  Mon: {}  ·  {rt_str}",
            input.mode_label, input.monitor_label
        );
        if cy + 14.0 <= inner.bottom() {
            dl.text(
                Rect::new(body.x, cy, body.w, 14.0),
                trim_label(&route_line, max_c),
                mfont,
                false,
                theme.text.secondary,
                TextAlign::Left,
            );
            cy += 15.0;
        }
        if input.digital_silence {
            let silence_tip = if cfg!(target_os = "macos") {
                "! Pure 0.0 silence · check macOS Privacy & Security → Microphone"
            } else {
                "! Pure 0.0 silence · check OS Microphone permission or input gain"
            };
            if cy + 14.0 <= inner.bottom() {
                dl.text(
                    Rect::new(body.x, cy, body.w, 14.0),
                    trim_label(silence_tip, max_c),
                    mfont,
                    true,
                    theme.accent.warning_key,
                    TextAlign::Left,
                );
            }
        } else if cy + 14.0 <= inner.bottom() {
            dl.text(
                Rect::new(body.x, cy, body.w, 14.0),
                trim_label("t a record/stop · t A dialog · t i cycle monitor", max_c),
                mfont,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
        }
    } else {
        for line in wrap_lines(
            "Select an Input device on the left to enable live level metering, monitoring (t i), and 24-bit WAV recording (t a / t A).",
            max_c,
            3,
        ) {
            if cy + 13.0 > inner.bottom() {
                break;
            }
            dl.text(
                Rect::new(body.x, cy, body.w, 13.0),
                line,
                mfont,
                false,
                theme.text.muted,
                TextAlign::Left,
            );
            cy += 13.5;
        }
    }

    dl.pop_clip();
}

fn draw_midi_tab(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    // 1. Live ARM / LEARN status strip (rendered only when recording or MIDI Learn is active).
    let rest = if view.recording || view.learn_target.is_some() {
        let (banner, lower) = area.split_top(28.0);
        draw_live_status_banner(dl, banner, view, theme);
        Rect::new(lower.x, lower.y + 6.0, lower.w, (lower.h - 6.0).max(0.0))
    } else {
        area
    };

    if rest.h < 40.0 {
        return;
    }

    // 2. Upper visualizers (8 RGB Pads + 8 Focused Smart Macros) when tall enough to keep
    //    `DEVICE & PROFILE` + `MAPPINGS` spacious below.
    let body = if rest.h >= 270.0 {
        let vis_h = (rest.h * 0.30).clamp(112.0, 146.0);
        let (vis_row, lower) = rest.split_top(vis_h);
        let gap = 10.0;
        let left_w = (vis_row.w * 0.46).max(200.0);
        let pads_rect = Rect::new(vis_row.x, vis_row.y, left_w, vis_row.h);
        let macros_rect = Rect::new(
            pads_rect.right() + gap,
            vis_row.y,
            (vis_row.w - left_w - gap).max(0.0),
            vis_row.h,
        );
        draw_pads_panel(dl, pads_rect, view, theme);
        draw_macros_panel(dl, macros_rect, view, theme);
        Rect::new(lower.x, lower.y + 8.0, lower.w, (lower.h - 8.0).max(0.0))
    } else {
        rest
    };

    // 3. Lower split: Unified `DEVICE & PROFILE` + `MAPPINGS` inspector (66%) + Live Monitor (34%).
    if body.h >= 40.0 {
        let gap = 10.0;
        let inspector_w = (body.w * 0.66).max(260.0).min(body.w);
        let inspector_rect = Rect::new(body.x, body.y, inspector_w, body.h);
        let log_rect = Rect::new(
            inspector_rect.right() + gap,
            body.y,
            (body.w - inspector_w - gap).max(0.0),
            body.h,
        );
        draw_inspector_panel(dl, inspector_rect, view, theme);
        if log_rect.w >= 140.0 {
            draw_log_panel(dl, log_rect, view, theme);
        }
    }
}

struct BannerBadge<'a> {
    key: &'a str,
    label: &'a str,
    accent: Color,
}

fn draw_banner_badge(
    dl: &mut DisplayList,
    x: &mut f32,
    row: Rect,
    badge: BannerBadge<'_>,
    theme: &Theme,
) {
    let cw = theme.char_width(theme.font.small);
    let text = if badge.key.is_empty() {
        badge.label.to_string()
    } else {
        format!("[{}] {}", badge.key, badge.label)
    };
    let w = (text.chars().count() as f32 * cw + 14.0).min((row.right() - *x).max(0.0));
    if w < 24.0 || *x + w > row.right() + 0.5 {
        return;
    }
    let rect = Rect::new(*x, row.y + 2.0, w, 24.0);
    dl.panel(
        rect,
        badge.accent.with_alpha(0.18),
        badge.accent.with_alpha(0.85),
        theme.radii.pill,
        1.0,
        0.24,
    );
    dl.text(
        rect.inset(6.0, 0.0),
        trim_label(&text, ((w - 12.0) / cw) as usize),
        theme.font.small,
        true,
        badge.accent,
        TextAlign::Center,
    );
    *x += w + 6.0;
}

fn draw_live_status_banner(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    let mut x = area.x;
    if view.recording {
        draw_banner_badge(
            dl,
            &mut x,
            area,
            BannerBadge {
                key: "R",
                label: "● REC · NOTE+CC",
                accent: theme.accent.alert,
            },
            theme,
        );
        if let Some(target) = &view.cc_last_recorded {
            let cc_lbl = format!("CC● {}", trim_label(target, 22));
            draw_banner_badge(
                dl,
                &mut x,
                area,
                BannerBadge {
                    key: "",
                    label: &cc_lbl,
                    accent: theme.accent.modulation,
                },
                theme,
            );
        }
    }
    if let Some(target) = &view.learn_target {
        let learn_lbl = format!("LEARN: {}", trim_label(target, 28));
        draw_banner_badge(
            dl,
            &mut x,
            area,
            BannerBadge {
                key: "M",
                label: &learn_lbl,
                accent: theme.accent.warning_key,
            },
            theme,
        );
    }
}

fn draw_pads_panel(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(8.0, 6.0);
    let (hdr, grid) = inner.split_top(18.0);
    dl.text(
        hdr,
        format!(
            "PERFORMANCE PADS  ·  {}",
            view.pad_mode_label.to_uppercase()
        ),
        theme.font.small - 1.0,
        true,
        theme.accent.modulation,
        TextAlign::Left,
    );
    let cols = 4usize;
    let rows = 2usize;
    let gap = 6.0;
    let tile_w = ((grid.w - gap * (cols - 1) as f32) / cols as f32).max(20.0);
    let tile_h = ((grid.h - gap * (rows - 1) as f32) / rows as f32).max(20.0);
    let cw = theme.char_width(theme.font.small);

    for (idx, pad) in view.pads.iter().take(8).enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let rect = Rect::new(
            grid.x + col as f32 * (tile_w + gap),
            grid.y + row as f32 * (tile_h + gap),
            tile_w,
            tile_h,
        );
        let led = theme.hardware_led(pad.rgb.0, pad.rgb.1, pad.rgb.2, 1.0);
        let bg = theme.hardware_led(
            pad.rgb.0,
            pad.rgb.1,
            pad.rgb.2,
            if pad.lit { 0.24 } else { 0.11 },
        );
        let border = theme.hardware_led(
            pad.rgb.0,
            pad.rgb.1,
            pad.rgb.2,
            if pad.lit { 0.92 } else { 0.45 },
        );
        dl.panel(
            rect,
            bg,
            border,
            theme.radii.badge,
            if pad.lit { 1.5 } else { 1.0 },
            if pad.lit { 0.35 } else { 0.05 },
        );
        // Crisp top RGB LED bar mirroring hardware pad illumination.
        dl.rounded_glow(
            Rect::new(rect.x + 5.0, rect.y + 4.0, (rect.w - 10.0).max(4.0), 3.0),
            led,
            1.5,
            if pad.lit { 0.55 } else { 0.15 },
        );
        let top_row = Rect::new(rect.x + 6.0, rect.y + 10.0, rect.w - 12.0, 14.0);
        dl.text(
            top_row,
            format!("P{}", pad.index + 1),
            theme.font.small - 1.0,
            true,
            theme.text.primary,
            TextAlign::Left,
        );
        dl.text(
            top_row,
            &pad.note_badge,
            theme.font.small - 1.5,
            false,
            theme.text.muted,
            TextAlign::Right,
        );
        let max_c = ((rect.w - 12.0) / cw).max(3.0) as usize;
        dl.text(
            Rect::new(rect.x + 6.0, rect.bottom() - 18.0, rect.w - 12.0, 16.0),
            trim_label(&pad.label, max_c),
            theme.font.small - 0.5,
            pad.lit,
            if pad.lit {
                theme.text.primary
            } else {
                theme.text.secondary
            },
            TextAlign::Left,
        );
    }
}

fn draw_macros_panel(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(8.0, 6.0);
    let (hdr, grid) = inner.split_top(18.0);
    let hdr_text = if view.project_macro_count > 0 {
        format!(
            "PROJECT + SMART MACROS ({}/8 PROJECT  ·  KNOBS 1–8)",
            view.project_macro_count
        )
    } else {
        "FOCUSED SMART MACROS (KNOBS 1–8)".into()
    };
    dl.text(
        hdr,
        hdr_text,
        theme.font.small - 1.0,
        true,
        if view.project_macro_count > 0 {
            theme.accent.modulation
        } else {
            theme.accent.focus
        },
        TextAlign::Left,
    );
    let cols = 4usize;
    let rows = 2usize;
    let gap = 6.0;
    let tile_w = ((grid.w - gap * (cols - 1) as f32) / cols as f32).max(20.0);
    let tile_h = ((grid.h - gap * (rows - 1) as f32) / rows as f32).max(20.0);
    let cw = theme.char_width(theme.font.small);

    for (idx, mac) in view.macros.iter().take(8).enumerate() {
        let col = idx % cols;
        let row = idx / cols;
        let rect = Rect::new(
            grid.x + col as f32 * (tile_w + gap),
            grid.y + row as f32 * (tile_h + gap),
            tile_w,
            tile_h,
        );
        let mac_accent = if mac.automated {
            theme.accent.modulation
        } else if mac.is_project_macro {
            theme.track_color(mac.slot)
        } else {
            theme.accent.focus
        };
        let border = if mac.is_project_macro {
            mac_accent.with_alpha(if mac.automated { 0.88 } else { 0.65 })
        } else {
            theme.border.subtle
        };
        dl.panel(
            rect,
            if mac.is_project_macro {
                mac_accent.with_alpha(0.08)
            } else {
                theme.surface.elevated
            },
            border,
            theme.radii.badge,
            if mac.is_project_macro { 1.2 } else { 1.0 },
            if mac.automated { 0.18 } else { 0.0 },
        );
        let top = Rect::new(rect.x + 6.0, rect.y + 4.0, rect.w - 12.0, 14.0);
        let slot_tag = if mac.automated {
            format!("M{}·MOD", mac.slot + 1)
        } else if mac.is_project_macro {
            format!("M{}·{}t", mac.slot + 1, mac.binding_count)
        } else {
            format!("M{}", mac.slot + 1)
        };
        dl.text(
            top,
            slot_tag,
            theme.font.small - 1.0,
            true,
            mac_accent,
            TextAlign::Left,
        );
        if let Some(cc) = &mac.cc_badge {
            dl.text(
                top,
                cc,
                theme.font.small - 1.5,
                false,
                theme.accent.warning_key,
                TextAlign::Right,
            );
        }
        let mid_y = rect.y + 18.0;
        let val_font = theme.font.small - 1.5;
        let val_cw = theme.char_width(val_font);
        let max_val_w = ((rect.w - 12.0) * 0.48).max(36.0);
        let val_w =
            (mac.value_display.chars().count() as f32 * val_cw + 6.0).clamp(36.0, max_val_w);
        let lbl_w = (rect.w - 12.0 - val_w - 4.0).max(20.0);
        let max_c = (lbl_w / cw).max(4.0) as usize;
        dl.text(
            Rect::new(rect.x + 6.0, mid_y, lbl_w, 15.0),
            trim_label(&mac.label, max_c),
            theme.font.small - 1.0,
            mac.is_project_macro,
            theme.text.primary,
            TextAlign::Left,
        );
        let max_val_c = (val_w / val_cw).max(4.0) as usize;
        dl.text(
            Rect::new(rect.right() - 6.0 - val_w, mid_y, val_w, 15.0),
            trim_label(&mac.value_display, max_val_c),
            val_font,
            false,
            if mac.automated {
                theme.accent.modulation
            } else {
                theme.text.secondary
            },
            TextAlign::Right,
        );
        // Smooth horizontal normalized fill bar at bottom of macro card.
        let bar = Rect::new(
            rect.x + 6.0,
            rect.bottom() - 10.0,
            (rect.w - 12.0).max(4.0),
            5.0,
        );
        dl.rounded(bar, theme.meter.track_bg, 2.5);
        let fw = bar.w * mac.normalized.clamp(0.0, 1.0);
        if fw > 0.5 {
            dl.rounded_glow(
                Rect::new(bar.x, bar.y, fw, bar.h),
                mac_accent,
                2.5,
                if mac.automated { 0.36 } else { 0.22 },
            );
        }
    }
}

enum InspectorItem<'a> {
    Header {
        title: String,
        accent: Color,
    },
    Device(&'a MidiDeviceRowView),
    Mapping {
        idx: usize,
        row: &'a MidiMappingRowView,
    },
    EmptyMappings,
}

fn draw_inspector_panel(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(8.0, 6.0);
    let (body, footer) = inner.split_bottom(18.0);
    let body = Rect::new(body.x, body.y, body.w, (body.h - 4.0).max(0.0));

    let hdr_h = 18.0_f32;
    let dev_h = 26.0_f32;
    let map_h = 24.0_f32;

    let mut items: Vec<(f32, f32, InspectorItem<'_>)> =
        Vec::with_capacity(view.device_rows.len() + view.mappings.len() + 3);
    let mut y_acc = 0.0_f32;
    let mut sel_top = 0.0_f32;
    let mut sel_bot = dev_h;

    let dev_group_top = y_acc;
    items.push((
        y_acc,
        hdr_h,
        InspectorItem::Header {
            title: "DEVICE & PROFILE".into(),
            accent: theme.accent.focus,
        },
    ));
    y_acc += hdr_h + 2.0;

    for row in &view.device_rows {
        if row.selected {
            sel_top = if row.label == "IN" {
                dev_group_top
            } else {
                y_acc
            };
            sel_bot = y_acc + dev_h;
        }
        items.push((y_acc, dev_h, InspectorItem::Device(row)));
        y_acc += dev_h + 2.0;
    }

    y_acc += 6.0;
    let map_group_top = y_acc;
    let map_title = if view.user_mapping_count > 0 {
        format!(
            "MAPPINGS ({}  ·  {} USER)",
            view.mappings.len(),
            view.user_mapping_count
        )
    } else {
        format!("MAPPINGS ({})", view.mappings.len())
    };
    items.push((
        y_acc,
        hdr_h,
        InspectorItem::Header {
            title: map_title,
            accent: theme.accent.routing,
        },
    ));
    y_acc += hdr_h + 2.0;

    if view.mappings.is_empty() {
        items.push((y_acc, 24.0, InspectorItem::EmptyMappings));
        y_acc += 26.0;
    } else {
        for (idx, row) in view.mappings.iter().enumerate() {
            if row.selected {
                sel_top = if idx == 0 { map_group_top } else { y_acc };
                sel_bot = y_acc + map_h;
            }
            items.push((y_acc, map_h, InspectorItem::Mapping { idx, row }));
            y_acc += map_h + 2.0;
        }
    }

    let total_h = y_acc;
    let max_scroll = (total_h - body.h).max(0.0);
    let scroll_y = if max_scroll <= 0.0 {
        0.0
    } else {
        let desired = ((sel_top + sel_bot) * 0.5 - body.h * 0.5).clamp(0.0, max_scroll);
        if sel_bot - desired > body.h {
            (sel_bot - body.h + 4.0).clamp(0.0, max_scroll)
        } else if sel_top < desired {
            sel_top.clamp(0.0, max_scroll)
        } else {
            desired
        }
    };

    let scrollbar_w = if max_scroll > 0.5 { 6.0 } else { 0.0 };
    let content_w = (body.w - scrollbar_w - if scrollbar_w > 0.0 { 4.0 } else { 0.0 }).max(80.0);
    let cw = theme.char_width(theme.font.small);

    dl.push_clip(body);
    for (iy, ih, item) in items {
        let y = body.y + iy - scroll_y;
        if y + ih < body.y - 2.0 || y > body.bottom() + 2.0 {
            continue;
        }
        let r = Rect::new(body.x, y, content_w, ih);
        match item {
            InspectorItem::Header { title, accent } => {
                dl.text(
                    Rect::new(r.x + 2.0, r.y, r.w - 4.0, r.h),
                    title,
                    theme.font.small - 1.0,
                    true,
                    accent,
                    TextAlign::Left,
                );
            }
            InspectorItem::Device(row) => {
                draw_device_row(dl, r, row, cw, theme);
            }
            InspectorItem::Mapping { idx, row } => {
                draw_mapping_row(dl, r, idx, row, cw, theme);
            }
            InspectorItem::EmptyMappings => {
                let max_c = ((r.w - 12.0) / cw).max(16.0) as usize;
                dl.text(
                    Rect::new(r.x + 6.0, r.y, r.w - 12.0, r.h),
                    trim_label(
                        "No mappings · press a to add a MIDI mapping, M on any parameter to Learn, or switch Profile",
                        max_c,
                    ),
                    theme.font.small,
                    false,
                    theme.text.muted,
                    TextAlign::Left,
                );
            }
        }
    }

    if max_scroll > 0.5 {
        let track = Rect::new(body.right() - scrollbar_w, body.y, scrollbar_w, body.h);
        dl.rounded(track, theme.meter.track_bg, 3.0);
        let frac = (body.h / total_h).clamp(0.12, 1.0);
        let thumb_h = (body.h * frac).max(18.0);
        let thumb_y = body.y + (scroll_y / max_scroll) * (body.h - thumb_h);
        dl.rounded(
            Rect::new(track.x, thumb_y, track.w, thumb_h),
            theme.accent.focus.with_alpha(0.65),
            3.0,
        );
    }
    dl.pop_clip();

    // Footer action cue bar (zero :cmd leaks, 100% aligned with modeless spec).
    let total_rows = view.device_rows.len() + view.mappings.len();
    let pos_text = format!("{}/{}", view.cursor + 1, total_rows.max(1));
    let (hint_left, pos_right) = footer.split_right(52.0);
    let max_h = (hint_left.w / theme.char_width(theme.font.small - 1.0)).max(12.0) as usize;
    dl.text(
        hint_left,
        trim_label(
            "H/L tab · j/k row · w/b group · Alt+h/l ,/. cycle · Enter pick/learn · a add · v inv · r rescan · x/X del/clear · Ctrl+s save",
            max_h,
        ),
        theme.font.small - 1.0,
        true,
        theme.accent.warning_key,
        TextAlign::Left,
    );
    dl.text(
        pos_right,
        pos_text,
        theme.font.small - 1.0,
        false,
        theme.text.muted,
        TextAlign::Right,
    );
}

fn draw_device_row(dl: &mut DisplayList, r: Rect, row: &MidiDeviceRowView, cw: f32, theme: &Theme) {
    if row.selected {
        dl.panel(
            r,
            theme.surface.selection,
            theme.accent.focus.with_alpha(0.85),
            theme.radii.badge,
            1.1,
            0.12,
        );
    } else {
        dl.panel(
            r,
            theme.surface.elevated.with_alpha(0.60),
            theme.border.hairline,
            theme.radii.badge,
            1.0,
            0.0,
        );
    }

    let lbl_w = 66.0_f32;
    let val_w = (r.w * 0.38)
        .clamp(130.0, 240.0)
        .min((r.w - lbl_w - 20.0).max(80.0));
    let det_x = r.x + lbl_w + val_w + 18.0;
    let det_w = (r.right() - det_x - 6.0).max(0.0);

    dl.text(
        Rect::new(r.x + 8.0, r.y, lbl_w, r.h),
        row.label,
        theme.font.small,
        true,
        if row.selected {
            theme.accent.focus
        } else {
            theme.text.secondary
        },
        TextAlign::Left,
    );

    let val_pill = Rect::new(r.x + lbl_w + 8.0, r.y + 3.0, val_w, (r.h - 6.0).max(16.0));
    let val_accent = if row.alert {
        theme.accent.alert
    } else if row.selected {
        theme.accent.focus
    } else if row.active {
        theme.accent.playhead
    } else {
        theme.text.muted
    };
    dl.panel(
        val_pill,
        if row.alert {
            theme.accent.alert.with_alpha(0.18)
        } else if row.selected {
            theme.accent.focus.with_alpha(0.18)
        } else if row.active {
            theme.accent.playhead.with_alpha(0.10)
        } else {
            theme.surface.recessed
        },
        val_accent.with_alpha(if row.selected || row.alert {
            0.90
        } else {
            0.40
        }),
        theme.radii.badge,
        1.0,
        if row.selected || row.alert { 0.14 } else { 0.0 },
    );
    let val_display = if row.alert {
        format!("! {}", row.value)
    } else {
        row.value.clone()
    };
    dl.text(
        val_pill.inset(6.0, 0.0),
        trim_label(&val_display, ((val_pill.w - 12.0) / cw).max(6.0) as usize),
        theme.font.small - 0.5,
        row.selected || row.active || row.alert,
        if row.alert {
            theme.accent.alert
        } else if row.selected {
            theme.accent.focus
        } else if row.active {
            theme.text.primary
        } else {
            theme.text.secondary
        },
        TextAlign::Left,
    );

    if det_w >= 64.0 {
        let det_cw = theme.char_width(theme.font.small - 1.5);
        dl.text(
            Rect::new(det_x, r.y, det_w, r.h),
            trim_label(&row.detail, (det_w / det_cw).max(8.0) as usize),
            theme.font.small - 1.5,
            row.alert,
            if row.alert {
                theme.accent.alert
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
    }
}

fn draw_mapping_row(
    dl: &mut DisplayList,
    r: Rect,
    idx: usize,
    row: &MidiMappingRowView,
    cw: f32,
    theme: &Theme,
) {
    if row.learning {
        dl.panel(
            r,
            theme.accent.warning_key.with_alpha(0.20),
            theme.accent.warning_key,
            theme.radii.badge,
            1.2,
            0.25,
        );
    } else if row.selected {
        dl.panel(
            r,
            theme.surface.selection,
            theme.accent.focus.with_alpha(0.85),
            theme.radii.badge,
            1.1,
            0.12,
        );
    } else if idx % 2 == 1 {
        dl.rounded(
            r,
            theme.surface.elevated.with_alpha(0.45),
            theme.radii.badge,
        );
    }

    let src_w = 112.0_f32.min(r.w * 0.25);
    let mode_w = 48.0;
    let flags_w = 104.0_f32.min(r.w * 0.22);
    let bar_w = if r.w >= 460.0 { 72.0 } else { 0.0 };
    let tgt_w = (r.w - src_w - mode_w - flags_w - bar_w - 22.0).max(60.0);

    dl.text(
        Rect::new(r.x + 6.0, r.y, src_w, r.h),
        if row.learning {
            "◉ LEARN…".to_string()
        } else {
            trim_label(&row.source, (src_w / cw) as usize)
        },
        theme.font.small,
        row.selected || row.learning,
        if row.learning {
            theme.accent.warning_key
        } else {
            theme.accent.focus
        },
        TextAlign::Left,
    );
    dl.text(
        Rect::new(r.x + 8.0 + src_w, r.y, mode_w, r.h),
        row.mode,
        theme.font.small - 0.5,
        false,
        theme.accent.modulation,
        TextAlign::Center,
    );
    dl.text(
        Rect::new(r.x + 12.0 + src_w + mode_w, r.y, tgt_w, r.h),
        trim_label(&row.target, (tgt_w / cw) as usize),
        theme.font.small,
        row.selected,
        theme.text.primary,
        TextAlign::Left,
    );

    if bar_w > 0.0 {
        if let Some(norm) = row.live_norm {
            let bx = r.right() - flags_w - bar_w - 10.0;
            let track = Rect::new(bx, r.y + r.h * 0.5 - 3.0, bar_w, 6.0);
            dl.rounded(track, theme.meter.track_bg, 3.0);
            let fw = track.w * norm.clamp(0.0, 1.0);
            if fw > 0.5 {
                dl.rounded(
                    Rect::new(track.x, track.y, fw, track.h),
                    theme.accent.routing,
                    3.0,
                );
            }
        }
    }
    dl.text(
        Rect::new(r.right() - flags_w - 6.0, r.y, flags_w, r.h),
        trim_label(&row.flags, (flags_w / cw) as usize),
        theme.font.small - 1.0,
        row.is_user,
        if row.is_user {
            theme.accent.warning_key
        } else {
            theme.text.muted
        },
        TextAlign::Right,
    );
}

fn draw_log_panel(dl: &mut DisplayList, area: Rect, view: &MidiDeckView, theme: &Theme) {
    dl.panel(
        area,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = area.inset(8.0, 6.0);
    let (hdr, body) = inner.split_top(20.0);
    let hdr_text = if view.recording {
        match &view.cc_last_recorded {
            Some(target) => format!("LIVE MIDI MONITOR  ·  ● REC CC: {}", trim_label(target, 20)),
            None => "LIVE MIDI MONITOR  ·  ● REC · NOTE+CC".into(),
        }
    } else {
        "LIVE MIDI MONITOR".into()
    };
    let max_hdr = ((hdr.w - 4.0) / theme.char_width(theme.font.small - 1.0)).max(10.0) as usize;
    dl.text(
        hdr,
        trim_label(&hdr_text, max_hdr),
        theme.font.small - 1.0,
        true,
        if view.recording {
            theme.accent.alert
        } else {
            theme.accent.playhead
        },
        TextAlign::Left,
    );
    let body = if let Some(err) = &view.midi_out_error {
        let efont = theme.font.small - 1.0;
        let text_w = (body.w - 14.0).max(80.0);
        let max_c = (text_w / theme.char_width(efont)).floor().max(12.0) as usize;
        let wrapped = wrap_lines(err, max_c, 2);
        let ideal_h = 10.0 + 16.0 + wrapped.len() as f32 * 14.0 + 16.0;
        let err_h = ideal_h.min(body.h);
        let (err_box, lower) = body.split_top(err_h);
        if err_box.h >= 34.0 {
            dl.panel(
                err_box,
                theme.accent.alert.with_alpha(0.14),
                theme.accent.alert.with_alpha(0.85),
                theme.radii.badge,
                1.0,
                0.12,
            );
            let ei = err_box.inset(7.0, 4.0);
            dl.text(
                Rect::new(ei.x, ei.y, ei.w, 15.0),
                "! MIDI OUT ERROR",
                efont,
                true,
                theme.accent.alert,
                TextAlign::Left,
            );
            let mut ly = ei.y + 16.0;
            for line in wrapped {
                if ly + 13.0 > ei.bottom() + 1.0 {
                    break;
                }
                dl.text(
                    Rect::new(ei.x, ly, ei.w, 14.0),
                    line,
                    efont,
                    false,
                    theme.text.primary,
                    TextAlign::Left,
                );
                ly += 14.0;
            }
            if ly + 14.0 <= ei.bottom() + 2.0 {
                dl.text(
                    Rect::new(ei.x, ly + 2.0, ei.w, 14.0),
                    trim_label("Press r to rescan or Enter on OUT to reconnect", max_c),
                    efont,
                    false,
                    theme.accent.alert,
                    TextAlign::Left,
                );
            }
        }
        Rect::new(lower.x, lower.y + 6.0, lower.w, (lower.h - 6.0).max(0.0))
    } else {
        body
    };
    if body.h < 18.0 {
        return;
    }
    if view.log.is_empty() {
        dl.text(
            body,
            "Waiting for MIDI input…\nMove a knob, fader, pad,\nor play a note.",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Left,
        );
        return;
    }
    let row_h = 20.0;
    let max_rows = (body.h / row_h).floor().max(1.0) as usize;
    let cw = theme.char_width(theme.font.small - 0.5);
    for (idx, entry) in view.log.iter().take(max_rows).enumerate() {
        let r = Rect::new(body.x, body.y + idx as f32 * row_h, body.w, row_h - 1.0);
        let src_w = 88.0_f32.min(r.w * 0.34);
        let val_w = 54.0_f32.min(r.w * 0.22);
        let act_w = (r.w - src_w - val_w - 8.0).max(40.0);
        let alpha = if idx == 0 {
            1.0
        } else {
            (1.0 - idx as f32 * 0.06).max(0.45)
        };
        dl.text(
            Rect::new(r.x, r.y, src_w, r.h),
            trim_label(&entry.source, (src_w / cw) as usize),
            theme.font.small - 0.5,
            idx == 0,
            theme.accent.playhead.with_alpha(alpha),
            TextAlign::Left,
        );
        dl.text(
            Rect::new(r.x + src_w + 4.0, r.y, val_w, r.h),
            trim_label(&entry.value, (val_w / cw) as usize),
            theme.font.small - 0.5,
            false,
            theme.accent.warning_key.with_alpha(alpha),
            TextAlign::Left,
        );
        dl.text(
            Rect::new(r.x + src_w + val_w + 8.0, r.y, act_w, r.h),
            trim_label(&entry.action, (act_w / cw) as usize),
            theme.font.small - 0.5,
            false,
            theme.text.secondary.with_alpha(alpha),
            TextAlign::Left,
        );
    }
}
