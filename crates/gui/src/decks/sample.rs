//! Dedicated GPU Sample / Waveform deck (`gw`):
//! zoomable high-res waveform canvas, overview minimap, beat/time ruler, crop & loop overlays,
//! numbered/labeled RIFF slice markers, edit cursor bar, and bottom slicing parameter toolbar.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{SampleDeckView, SampleSliceView, UiModeKind};
use cadence_app::SampleToolbarField;
use cadence_core::music::note_name;
use cadence_core::rack::SampleLoopMode;

const TOP_STRIP_H: f32 = 24.0;
const MINIMAP_H: f32 = 18.0;
const RULER_H: f32 = 18.0;
const TOOLBAR_H: f32 = 30.0;

fn fmt_time(frame: usize, sample_rate: u32) -> String {
    if sample_rate == 0 {
        return format!("{frame}f");
    }
    let secs = frame as f64 / f64::from(sample_rate);
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 10.0 {
        format!("{secs:.2}s")
    } else {
        format!("{secs:.1}s")
    }
}

fn fmt_time_precise(frame: usize, sample_rate: u32) -> String {
    if sample_rate == 0 {
        return format!("{frame}f");
    }
    let secs = frame as f64 / f64::from(sample_rate);
    if secs < 1.0 {
        format!("{:.1}ms", secs * 1000.0)
    } else {
        format!("{secs:.3}s")
    }
}

fn pick_ruler_interval_secs(visible_secs: f64, width_px: f32) -> f64 {
    if visible_secs <= 0.0 || width_px <= 1.0 {
        return 1.0;
    }
    let max_ticks = (f64::from(width_px) / 76.0).max(2.0);
    let raw = visible_secs / max_ticks;
    const CANDIDATES: &[f64] = &[
        0.001, 0.002, 0.005, 0.01, 0.02, 0.025, 0.05, 0.1, 0.2, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0,
        15.0, 30.0, 60.0,
    ];
    for &c in CANDIDATES {
        if c >= raw {
            return c;
        }
    }
    60.0
}

pub fn draw_sample(
    dl: &mut DisplayList,
    area: Rect,
    view: &SampleDeckView,
    focused: bool,
    mode: UiModeKind,
    theme: &Theme,
) {
    let badge = if view.loaded {
        let ch = match view.channels {
            1 => "MONO",
            2 => "ST",
            n => return_channels_tag(n),
        };
        let dur = fmt_time(view.total_frames, view.sample_rate);
        let sr_khz = f64::from(view.sample_rate) / 1000.0;
        let slice_part = if view.slices.is_empty() {
            String::new()
        } else {
            format!(
                " · {} slice{}",
                view.slices.len(),
                if view.slices.len() == 1 { "" } else { "s" }
            )
        };
        format!(
            "{} · {sr_khz:.1}kHz {ch} · {dur}{slice_part} · {:.1}×",
            trim_label(&view.sample_name, 26),
            view.zoom
        )
    } else if view.load_error.is_some() {
        format!("{} · load error", trim_label(&view.sample_name, 24))
    } else {
        "No sample loaded · B browse".into()
    };

    let accent = if view.load_error.is_some() {
        theme.accent.alert
    } else if view.riff_sync_dirty {
        theme.accent.warning_key
    } else {
        theme.accent.focus
    };

    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: "SAMPLE",
            right_badge: Some(&badge),
            accent,
            focused,
            mode,
        },
        theme,
    );
    if inner.w < 140.0 || inner.h < 68.0 {
        return;
    }

    dl.push_clip(inner);

    if !view.loaded {
        draw_empty_state(dl, inner, view, theme);
        dl.pop_clip();
        return;
    }

    let (top_strip, rest) = inner.split_top(TOP_STRIP_H);
    let (canvas_and_mini, toolbar_rect) = if rest.h >= TOOLBAR_H + 54.0 {
        let (top, bot) = rest.split_bottom(TOOLBAR_H);
        (
            Rect::new(top.x, top.y + 3.0, top.w, (top.h - 6.0).max(1.0)),
            Some(bot),
        )
    } else {
        (
            Rect::new(rest.x, rest.y + 3.0, rest.w, (rest.h - 3.0).max(1.0)),
            None,
        )
    };

    draw_top_strip(dl, top_strip, view, theme);

    let (minimap_rect, wave_frame) =
        if canvas_and_mini.h >= MINIMAP_H + 72.0 && !view.overview_peaks.is_empty() {
            let (m, w) = canvas_and_mini.split_top(MINIMAP_H);
            (
                Some(m),
                Rect::new(w.x, w.y + 4.0, w.w, (w.h - 4.0).max(1.0)),
            )
        } else {
            (None, canvas_and_mini)
        };

    if let Some(mini) = minimap_rect {
        draw_minimap(dl, mini, view, theme);
    }

    draw_waveform_canvas(dl, wave_frame, view, theme);

    if let Some(tb) = toolbar_rect {
        draw_toolbar(dl, tb, view, focused, theme);
    }

    dl.pop_clip();
}

fn return_channels_tag(ch: u8) -> &'static str {
    match ch {
        0 => "0CH",
        1 => "MONO",
        2 => "ST",
        _ => "MULTI",
    }
}

fn draw_empty_state(dl: &mut DisplayList, inner: Rect, view: &SampleDeckView, theme: &Theme) {
    let box_h = 84.0_f32.min(inner.h - 12.0).max(44.0);
    let box_w = 540.0_f32.min(inner.w - 16.0).max(120.0);
    let bx = inner.x + (inner.w - box_w) * 0.5;
    let by = inner.y + (inner.h - box_h) * 0.5;
    let card = Rect::new(bx, by, box_w, box_h);
    dl.panel(
        card,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let (hdr, body) = card.inset(14.0, 10.0).split_top(22.0);
    let crumb = if view.zone_name.eq_ignore_ascii_case(&view.module_name) {
        format!("{} › {}", view.instrument_name, view.zone_name)
    } else {
        format!(
            "{} › {} › {}",
            view.instrument_name, view.zone_name, view.module_name
        )
    };
    dl.text(
        hdr,
        crumb,
        theme.font.body,
        true,
        theme.text.primary,
        TextAlign::Center,
    );
    let msg = if let Some(err) = &view.load_error {
        format!("Sample error: {err}")
    } else {
        "No audio file assigned · press B to browse samples or :sample <PATH>".into()
    };
    dl.text(
        body,
        msg,
        theme.font.small,
        false,
        if view.load_error.is_some() {
            theme.accent.alert
        } else {
            theme.text.muted
        },
        TextAlign::Center,
    );
}

fn draw_top_strip(dl: &mut DisplayList, rect: Rect, view: &SampleDeckView, theme: &Theme) {
    // Right-aligned semantic status badges first so we know how much width remains for the breadcrumb.
    let cw = theme.char_width(theme.font.small - 0.5);
    let mut right_x = rect.right();

    let mut draw_right_pill = |dl: &mut DisplayList,
                               label: &str,
                               fg: crate::theme::Color,
                               bg: crate::theme::Color,
                               border: crate::theme::Color| {
        let w = (label.chars().count() as f32 * cw + 14.0).max(46.0);
        if right_x - w < rect.x + 120.0 {
            return;
        }
        right_x -= w;
        let pr = Rect::new(right_x, rect.y + 3.0, w, (rect.h - 6.0).max(14.0));
        dl.panel(pr, bg, border, theme.radii.badge, 1.0, 0.0);
        dl.text(
            pr.inset(5.0, 0.0),
            label,
            theme.font.small - 0.5,
            true,
            fg,
            TextAlign::Center,
        );
        right_x -= 6.0;
    };

    // RIFF WAV sync badge
    if view.is_wav_file {
        if view.riff_sync_dirty {
            draw_right_pill(
                dl,
                "RIFF MOD · W",
                theme.accent.warning_key,
                theme.accent.warning_key.with_alpha(0.16),
                theme.accent.warning_key.with_alpha(0.75),
            );
        } else if view.has_riff_cues {
            draw_right_pill(
                dl,
                "RIFF CUE ✓",
                theme.meter.low,
                theme.meter.low.with_alpha(0.14),
                theme.meter.low.with_alpha(0.65),
            );
        } else {
            draw_right_pill(
                dl,
                "WAV · W save",
                theme.text.muted,
                theme.surface.recessed,
                theme.border.subtle,
            );
        }
    }

    // Zero-crossing snap badge
    if view.snap_zero {
        draw_right_pill(
            dl,
            "SNAP ZC",
            theme.accent.routing,
            theme.accent.routing.with_alpha(0.14),
            theme.accent.routing.with_alpha(0.65),
        );
    } else {
        draw_right_pill(
            dl,
            "SNAP FREE",
            theme.text.muted,
            theme.surface.recessed,
            theme.border.subtle,
        );
    }

    // Loop mode badge
    let (loop_lbl, loop_active) = match view.loop_mode {
        SampleLoopMode::Off => ("LOOP OFF", false),
        SampleLoopMode::Forward => ("LOOP FWD", true),
        SampleLoopMode::PingPong => ("LOOP P-P", true),
    };
    if loop_active {
        draw_right_pill(
            dl,
            loop_lbl,
            theme.accent.playhead,
            theme.accent.playhead.with_alpha(0.16),
            theme.accent.playhead.with_alpha(0.75),
        );
    } else {
        draw_right_pill(
            dl,
            loop_lbl,
            theme.text.muted,
            theme.surface.recessed,
            theme.border.subtle,
        );
    }

    // Left breadcrumb + cropped musical length + active slice summary.
    let left_w = (right_x - rect.x - 4.0).max(0.0);
    if left_w > 40.0 {
        let crop_frames = view.sample_end.saturating_sub(view.sample_start);
        let crop_secs = if view.sample_rate > 0 {
            crop_frames as f64 / f64::from(view.sample_rate)
        } else {
            0.0
        };
        let bars = if view.bpm > 0.0 {
            crop_secs / (240.0 / view.bpm)
        } else {
            0.0
        };
        let active_str = view
            .active_slice_idx
            .and_then(|i| view.slices.get(i))
            .map(|sl| {
                let lbl = sl
                    .label
                    .as_deref()
                    .map(|s| format!(" '{}'", trim_label(s, 14)))
                    .unwrap_or_default();
                let note = sl
                    .midi_note
                    .map(|m| format!(" {}", note_name(m)))
                    .unwrap_or_default();
                let dur = fmt_time(sl.end_frame.saturating_sub(sl.frame), view.sample_rate);
                format!(
                    "  ·  SLICE {:02}/{:02}{lbl}{note} ({dur})",
                    sl.index + 1,
                    view.slices.len()
                )
            })
            .unwrap_or_default();
        let line = format!(
            "{} › {}  ·  {:.2}s ({bars:.2} bar @ {:.0} BPM)  ·  Root {}{active_str}",
            trim_label(&view.instrument_name, 16),
            trim_label(&view.zone_name, 14),
            crop_secs,
            view.bpm,
            note_name(view.root_midi),
        );
        let max_chars = (left_w / theme.char_width(theme.font.small))
            .floor()
            .max(8.0) as usize;
        dl.text(
            Rect::new(rect.x, rect.y, left_w, rect.h),
            trim_label(&line, max_chars),
            theme.font.small,
            false,
            theme.text.secondary,
            TextAlign::Left,
        );
    }
}

fn draw_minimap(dl: &mut DisplayList, rect: Rect, view: &SampleDeckView, theme: &Theme) {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.hairline,
        theme.radii.badge,
        1.0,
        0.0,
    );
    let plot = rect.inset(2.0, 2.0);
    if plot.is_empty() || view.total_frames == 0 {
        return;
    }
    dl.push_clip(plot);
    dl.peak_envelope(
        plot,
        &view.overview_peaks,
        theme.accent.focus.with_alpha(0.42),
        0.0,
    );
    let tf = view.total_frames as f32;
    let frame_x = |f: usize| plot.x + (f as f32 / tf).clamp(0.0, 1.0) * plot.w;

    // Dimmed crop regions on minimap
    let sx = frame_x(view.sample_start);
    let ex = frame_x(view.sample_end);
    if sx > plot.x {
        dl.fill(
            Rect::new(plot.x, plot.y, sx - plot.x, plot.h),
            theme.surface.canvas.with_alpha(0.55),
        );
    }
    if ex < plot.right() {
        dl.fill(
            Rect::new(ex, plot.y, plot.right() - ex, plot.h),
            theme.surface.canvas.with_alpha(0.55),
        );
    }

    // Tiny slice ticks on minimap
    for sl in &view.slices {
        let x = frame_x(sl.frame);
        let col = if sl.active {
            theme.accent.warning_key
        } else {
            theme.accent.modulation.with_alpha(0.70)
        };
        dl.fill(Rect::new(x, plot.y, 1.0, plot.h), col);
    }

    // Viewport lens highlight box
    let vx0 = frame_x(view.view_start_frame);
    let vx1 = frame_x(view.view_end_frame);
    let vw = (vx1 - vx0).max(4.0).min(plot.w);
    dl.panel(
        Rect::new(vx0, plot.y, vw, plot.h),
        theme
            .accent
            .focus
            .with_alpha(if view.zoom > 1.01 { 0.16 } else { 0.06 }),
        theme
            .accent
            .focus
            .with_alpha(if view.zoom > 1.01 { 0.90 } else { 0.40 }),
        1.5,
        1.0,
        0.0,
    );

    // Edit cursor hairline on minimap
    let cx = frame_x(view.cursor_frame);
    dl.fill(
        Rect::new((cx - 0.75).max(plot.x), plot.y, 1.5, plot.h),
        theme.accent.edit_cursor,
    );

    dl.pop_clip();
}

fn draw_waveform_canvas(dl: &mut DisplayList, rect: Rect, view: &SampleDeckView, theme: &Theme) {
    dl.panel(
        rect,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.panel,
        1.0,
        0.0,
    );
    let inner = rect.inset(2.0, 2.0);
    if inner.w < 24.0 || inner.h < 32.0 || view.total_frames == 0 {
        return;
    }

    let (ruler, wave_body) = if inner.h >= RULER_H + 30.0 {
        inner.split_top(RULER_H)
    } else {
        (Rect::new(inner.x, inner.y, inner.w, 0.0), inner)
    };

    let vis_start = view.view_start_frame;
    let vis_end = view.view_end_frame.max(vis_start + 1);
    let vis_span = (vis_end - vis_start) as f64;
    let frame_to_x = |frame: usize| -> f32 {
        let rel = (frame as f64 - vis_start as f64) / vis_span;
        wave_body.x + (rel as f32) * wave_body.w
    };

    dl.push_clip(inner);

    // 1. Ruler background + time ticks + grid verticals
    if !ruler.is_empty() {
        dl.fill(ruler, theme.surface.header);
        dl.fill(
            Rect::new(ruler.x, ruler.bottom() - 1.0, ruler.w, 1.0),
            theme.border.hairline,
        );
    }

    if view.sample_rate > 0 {
        let sr = f64::from(view.sample_rate);
        let vis_secs = vis_span / sr;
        let step_secs = pick_ruler_interval_secs(vis_secs, wave_body.w);
        let start_secs = vis_start as f64 / sr;
        let first_idx = (start_secs / step_secs).floor() as i64;
        let end_secs = vis_end as f64 / sr;
        let last_idx = (end_secs / step_secs).ceil() as i64;
        let mut last_lbl_right = wave_body.x - 100.0;
        let cw = theme.char_width(theme.font.small - 1.0);

        for tick_i in first_idx..=last_idx {
            let t = tick_i as f64 * step_secs;
            if t < 0.0 {
                continue;
            }
            let frame = (t * sr).round() as usize;
            let x = frame_to_x(frame);
            if x < wave_body.x - 1.0 || x > wave_body.right() + 1.0 {
                continue;
            }
            dl.fill(
                Rect::new(x, wave_body.y, 1.0, wave_body.h),
                theme.border.grid_step.with_alpha(0.45),
            );
            if !ruler.is_empty() {
                dl.fill(
                    Rect::new(x, ruler.bottom() - 5.0, 1.0, 5.0),
                    theme.border.grid_beat,
                );
                let lbl = if step_secs < 0.1 {
                    format!("{:.0}ms", t * 1000.0)
                } else if step_secs < 1.0 {
                    format!("{t:.2}s")
                } else {
                    format!("{t:.1}s")
                };
                let lw = lbl.chars().count() as f32 * cw + 6.0;
                if x + 3.0 >= last_lbl_right && x + 3.0 + lw <= ruler.right() {
                    dl.text(
                        Rect::new(x + 3.0, ruler.y, lw, ruler.h - 2.0),
                        lbl,
                        theme.font.small - 1.0,
                        false,
                        theme.text.muted,
                        TextAlign::Left,
                    );
                    last_lbl_right = x + 3.0 + lw + 6.0;
                }
            }
        }
    }

    // 2. Horizontal waveform reference lines (0 center + ±50%)
    let mid_y = wave_body.y + wave_body.h * 0.5;
    dl.fill(
        Rect::new(wave_body.x, mid_y, wave_body.w, 1.0),
        theme.border.grid_beat.with_alpha(0.75),
    );
    dl.fill(
        Rect::new(
            wave_body.x,
            wave_body.y + wave_body.h * 0.25,
            wave_body.w,
            1.0,
        ),
        theme.border.hairline.with_alpha(0.55),
    );
    dl.fill(
        Rect::new(
            wave_body.x,
            wave_body.y + wave_body.h * 0.75,
            wave_body.w,
            1.0,
        ),
        theme.border.hairline.with_alpha(0.55),
    );

    // 3. Active slice highlight band
    if let Some(active_sl) = view.active_slice_idx.and_then(|i| view.slices.get(i)) {
        let ax0 = frame_to_x(active_sl.frame).clamp(wave_body.x, wave_body.right());
        let ax1 = frame_to_x(active_sl.end_frame).clamp(wave_body.x, wave_body.right());
        if ax1 > ax0 {
            dl.fill(
                Rect::new(ax0, wave_body.y, ax1 - ax0, wave_body.h),
                theme.accent.focus.with_alpha(0.11),
            );
        }
    }

    // 4. Loop region overlay (when Forward or PingPong)
    if view.loop_mode != SampleLoopMode::Off && view.loop_end > view.loop_start {
        let raw_lx0 = frame_to_x(view.loop_start);
        let raw_lx1 = frame_to_x(view.loop_end);
        let lx0 = raw_lx0.clamp(wave_body.x, wave_body.right());
        let lx1 = raw_lx1.clamp(wave_body.x, wave_body.right());
        if lx1 > lx0 {
            dl.fill(
                Rect::new(lx0, wave_body.y, lx1 - lx0, wave_body.h),
                theme.accent.playhead.with_alpha(0.12),
            );
            dl.fill(
                Rect::new(lx0, wave_body.y, lx1 - lx0, 1.0),
                theme.accent.playhead.with_alpha(0.65),
            );
            dl.fill(
                Rect::new(lx0, wave_body.bottom() - 1.0, lx1 - lx0, 1.0),
                theme.accent.playhead.with_alpha(0.65),
            );
            let start_in_view = view.loop_start >= vis_start && view.loop_start <= vis_end;
            let end_in_view = view.loop_end >= vis_start && view.loop_end <= vis_end;
            if start_in_view {
                dl.fill(
                    Rect::new(lx0, wave_body.y, 1.5, wave_body.h),
                    theme.accent.playhead.with_alpha(0.85),
                );
                if lx0 < wave_body.right() - 24.0 {
                    let tag = Rect::new(lx0 + 2.0, wave_body.bottom() - 16.0, 22.0, 14.0);
                    dl.rounded(tag, theme.accent.playhead.with_alpha(0.28), 2.0);
                    dl.text(
                        tag,
                        "{L",
                        theme.font.small - 1.5,
                        true,
                        theme.accent.playhead,
                        TextAlign::Center,
                    );
                }
            }
            if end_in_view {
                dl.fill(
                    Rect::new((lx1 - 1.5).max(wave_body.x), wave_body.y, 1.5, wave_body.h),
                    theme.accent.playhead.with_alpha(0.85),
                );
                if lx1 > wave_body.x + 24.0 {
                    let tag = Rect::new(lx1 - 24.0, wave_body.bottom() - 16.0, 22.0, 14.0);
                    dl.rounded(tag, theme.accent.playhead.with_alpha(0.28), 2.0);
                    dl.text(
                        tag,
                        "L}",
                        theme.font.small - 1.5,
                        true,
                        theme.accent.playhead,
                        TextAlign::Center,
                    );
                }
            }
        }
    }

    // 5. High-res zoomed waveform envelope
    let wave_pad_top = if view.slices.is_empty() { 4.0 } else { 20.0 };
    let wave_plot = Rect::new(
        wave_body.x,
        wave_body.y + wave_pad_top,
        wave_body.w,
        (wave_body.h - wave_pad_top - 4.0).max(16.0),
    );
    if !view.peaks.is_empty() {
        dl.peak_envelope(
            wave_plot,
            &view.peaks,
            theme.accent.focus,
            theme.glow.scope * 0.55,
        );
    }

    // 6. Dimmed crop masks & [ / ] handles outside [sample_start..sample_end]
    let crop_sx = frame_to_x(view.sample_start);
    let crop_ex = frame_to_x(view.sample_end);
    if crop_sx > wave_body.x {
        let mask_w = (crop_sx - wave_body.x).min(wave_body.w);
        dl.fill(
            Rect::new(wave_body.x, wave_body.y, mask_w, wave_body.h),
            theme.surface.canvas.with_alpha(0.62),
        );
        if crop_sx <= wave_body.right() {
            dl.fill(
                Rect::new(crop_sx, wave_body.y, 1.5, wave_body.h),
                theme.accent.warning_key,
            );
            let flag = Rect::new(crop_sx, wave_body.bottom() - 16.0, 14.0, 14.0);
            dl.rounded(flag, theme.accent.warning_key.with_alpha(0.85), 2.0);
            dl.text(
                flag,
                "[",
                theme.font.small - 1.0,
                true,
                theme.text.inverted,
                TextAlign::Center,
            );
        }
    }
    if crop_ex < wave_body.right() {
        let left = crop_ex.max(wave_body.x);
        let mask_w = (wave_body.right() - left).max(0.0);
        dl.fill(
            Rect::new(left, wave_body.y, mask_w, wave_body.h),
            theme.surface.canvas.with_alpha(0.62),
        );
        if crop_ex >= wave_body.x {
            dl.fill(
                Rect::new(crop_ex - 1.5, wave_body.y, 1.5, wave_body.h),
                theme.accent.warning_key,
            );
            let flag = Rect::new(
                (crop_ex - 14.0).max(wave_body.x),
                wave_body.bottom() - 16.0,
                14.0,
                14.0,
            );
            dl.rounded(flag, theme.accent.warning_key.with_alpha(0.85), 2.0);
            dl.text(
                flag,
                "]",
                theme.font.small - 1.0,
                true,
                theme.text.inverted,
                TextAlign::Center,
            );
        }
    }

    // 7. Slice markers & collision-free top flag pills
    draw_slice_markers(dl, wave_body, &view.slices, frame_to_x, theme);

    // 8. Full-height Edit Cursor bar with ruler cap notch
    let cx = frame_to_x(view.cursor_frame);
    if cx >= wave_body.x - 1.0 && cx <= wave_body.right() + 1.0 {
        let clamped_cx = cx.clamp(wave_body.x, wave_body.right() - 2.0);
        dl.fill(
            Rect::new(clamped_cx, wave_body.y, 2.0, wave_body.h),
            theme.accent.edit_cursor,
        );
        if !ruler.is_empty() {
            let cap = Rect::new(
                (clamped_cx - 4.0).clamp(ruler.x, ruler.right() - 10.0),
                ruler.bottom() - 7.0,
                10.0,
                7.0,
            );
            dl.rounded(cap, theme.accent.edit_cursor, 2.0);
        }
    }

    dl.pop_clip();
}

fn draw_slice_markers(
    dl: &mut DisplayList,
    wave_body: Rect,
    slices: &[SampleSliceView],
    frame_to_x: impl Fn(usize) -> f32,
    theme: &Theme,
) {
    if slices.is_empty() {
        return;
    }
    let cw = theme.char_width(theme.font.small - 1.5);
    let mut last_pill_right = wave_body.x - 100.0;

    for sl in slices {
        let x = frame_to_x(sl.frame);
        if x < wave_body.x - 1.0 || x > wave_body.right() + 1.0 {
            continue;
        }
        let col = if sl.active {
            theme.accent.warning_key
        } else {
            theme.accent.modulation
        };
        let line_w = if sl.active { 1.75 } else { 1.0 };
        dl.fill(
            Rect::new(x, wave_body.y, line_w, wave_body.h),
            col.with_alpha(if sl.active { 0.95 } else { 0.72 }),
        );

        let next_x = frame_to_x(sl.end_frame).min(wave_body.right());
        let avail_w = (next_x - x - 4.0).max(0.0);
        let pill_text = if let Some(lbl) = &sl.label {
            let full = format!("{:02} {}", sl.index + 1, trim_label(lbl, 12));
            if full.chars().count() as f32 * cw + 10.0 <= avail_w || sl.active {
                full
            } else {
                format!("{:02}", sl.index + 1)
            }
        } else if let Some(midi) = sl.midi_note {
            let with_note = format!("{:02} {}", sl.index + 1, note_name(midi));
            if with_note.chars().count() as f32 * cw + 10.0 <= avail_w {
                with_note
            } else {
                format!("{:02}", sl.index + 1)
            }
        } else {
            format!("{:02}", sl.index + 1)
        };

        let pw = (pill_text.chars().count() as f32 * cw + 8.0).max(20.0);
        let px = x + 2.0;
        let overlaps_prev = px < last_pill_right + 2.0;
        if (!overlaps_prev || sl.active) && px + pw <= wave_body.right() {
            let pr = Rect::new(px, wave_body.y + 2.0, pw, 15.0);
            let bg = if sl.active {
                col.with_alpha(0.92)
            } else {
                theme.surface.elevated.with_alpha(0.90)
            };
            let fg = if sl.active { theme.text.inverted } else { col };
            dl.panel(
                pr,
                bg,
                col.with_alpha(if sl.active { 1.0 } else { 0.65 }),
                theme.radii.badge,
                1.0,
                0.0,
            );
            dl.text(
                pr.inset(3.0, 0.0),
                pill_text,
                theme.font.small - 1.5,
                true,
                fg,
                TextAlign::Center,
            );
            last_pill_right = pr.right();
        }
    }
}

fn draw_toolbar(
    dl: &mut DisplayList,
    rect: Rect,
    view: &SampleDeckView,
    focused: bool,
    theme: &Theme,
) {
    let kit_w = if rect.w >= 540.0 { 138.0 } else { 0.0 };
    let fields_w = (rect.w - if kit_w > 0.0 { kit_w + 6.0 } else { 0.0 }).max(100.0);
    let count = SampleToolbarField::ALL.len();
    let gap = 5.0;
    let card_w = ((fields_w - gap * (count.saturating_sub(1)) as f32) / count as f32).max(40.0);

    let mut x = rect.x;
    for field in SampleToolbarField::ALL {
        let selected = focused && view.toolbar_field == field;
        let card = Rect::new(x, rect.y + 2.0, card_w, (rect.h - 2.0).max(20.0));
        let (bg, border, glow) = if selected {
            (
                theme.accent.focus.with_alpha(0.20),
                theme.accent.focus,
                theme.glow.focus_border * 0.45,
            )
        } else {
            (theme.surface.elevated, theme.border.subtle, 0.0)
        };
        dl.panel(
            card,
            bg,
            border,
            theme.radii.pill,
            if selected { 1.5 } else { 1.0 },
            glow,
        );

        let value_str = match field {
            SampleToolbarField::Cursor => fmt_time_precise(view.cursor_frame, view.sample_rate),
            SampleToolbarField::Sensitivity => format!("{:.0}%", view.sensitivity * 100.0),
            SampleToolbarField::MinGap => format!("{:.0}ms", view.min_gap_ms),
            SampleToolbarField::GridDivisions => format!("{} div", view.grid_divisions),
            SampleToolbarField::LoopXfade => format!("{:.1}ms", view.loop_xfade_ms),
            SampleToolbarField::SnapZero => {
                if view.snap_zero {
                    "ZC ON".into()
                } else {
                    "FREE".into()
                }
            }
        };

        let inner = card.inset(6.0, 2.0);
        let field_lbl = if inner.w < 160.0 {
            match field {
                SampleToolbarField::Sensitivity => "SENS",
                SampleToolbarField::LoopXfade => "XFADE",
                SampleToolbarField::MinGap => "GAP",
                other => other.label(),
            }
        } else {
            field.label()
        };
        if inner.w >= 96.0 {
            let (l_rect, r_rect) = inner.split_left(inner.w * 0.54);
            dl.text(
                l_rect,
                field_lbl,
                theme.font.small - 1.0,
                selected,
                if selected {
                    theme.accent.focus
                } else {
                    theme.text.muted
                },
                TextAlign::Left,
            );
            dl.text(
                r_rect,
                value_str,
                theme.font.small - 0.5,
                true,
                if selected {
                    theme.text.primary
                } else {
                    theme.text.secondary
                },
                TextAlign::Right,
            );
        } else {
            dl.text(
                inner,
                format!("{} {}", trim_label(field_lbl, 4), value_str),
                theme.font.small - 1.5,
                selected,
                if selected {
                    theme.text.primary
                } else {
                    theme.text.secondary
                },
                TextAlign::Center,
            );
        }
        x += card_w + gap;
    }

    if kit_w > 0.0 {
        let kit_rect = Rect::new(
            rect.right() - kit_w,
            rect.y + 2.0,
            kit_w,
            (rect.h - 2.0).max(20.0),
        );
        let ready = !view.slices.is_empty();
        let accent = if ready {
            theme.accent.edit
        } else {
            theme.text.muted
        };
        dl.panel(
            kit_rect,
            if ready {
                accent.with_alpha(0.18)
            } else {
                theme.surface.recessed
            },
            if ready {
                accent.with_alpha(0.85)
            } else {
                theme.border.hairline
            },
            theme.radii.pill,
            1.0,
            if ready { 0.14 } else { 0.0 },
        );
        dl.text(
            kit_rect.inset(6.0, 0.0),
            if ready {
                format!("[K] KIT ({})", view.slices.len().min(32))
            } else {
                "[d/D] SLICE".into()
            },
            theme.font.small - 0.5,
            true,
            accent,
            TextAlign::Center,
        );
    }
}
