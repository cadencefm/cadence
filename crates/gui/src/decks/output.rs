//! GPU Output deck: phosphor oscilloscope trace + 32-bit float `-60..+6 dBFS`
//! stereo VU meters with `0 dB` hairline, pre-fader ticks, peak-hold ticks, `HOT`/`!CLIP`
//! status badges, and live Master dynamics gain reduction telemetry.
use crate::chrome::{draw_deck_frame, DeckFrameSpec};
use crate::display::{trim_label, DisplayList, Rect, TextAlign};
use crate::theme::Theme;
use cadence_app::view::{OutputDeckView, UiModeKind};
use cadence_core::dsp::math::{lin_to_db, LIN_FLOOR_F32};

pub const METER_MIN_DB: f32 = -60.0;
pub const METER_MAX_DB: f32 = 6.0;
pub const METER_ZERO_DB_NORM: f32 = (0.0 - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB);
pub const METER_GRATICULE_DB: [f32; 4] = [-48.0, -24.0, -12.0, -6.0];

pub fn draw_output(dl: &mut DisplayList, area: Rect, view: &OutputDeckView, theme: &Theme) {
    let frame_accent = if view.clipped {
        theme.meter.clip
    } else if view.hot {
        theme.meter.hot
    } else {
        theme.accent.routing
    };
    let inner = draw_deck_frame(
        dl,
        area,
        &DeckFrameSpec {
            title: view.title,
            right_badge: Some("go hide · C / tC clear clips"),
            accent: frame_accent,
            focused: false,
            mode: UiModeKind::Normal,
        },
        theme,
    );
    if inner.w < 120.0 || inner.h < 28.0 {
        return;
    }
    dl.push_clip(inner);
    let meter_w = (inner.w * 0.42).clamp(260.0, 440.0);
    let (scope_area, meter_area) = inner.split_right(meter_w);
    let scope_box = Rect::new(
        scope_area.x,
        scope_area.y,
        (scope_area.w - 10.0).max(0.0),
        scope_area.h,
    );

    // Recessed oscilloscope glass with center zero hairline & ±6dB graticules.
    dl.panel(
        scope_box,
        theme.surface.recessed,
        theme.border.subtle,
        theme.radii.pill,
        1.0,
        0.0,
    );
    let mid_y = scope_box.y + scope_box.h * 0.5;
    dl.fill(
        Rect::new(scope_box.x + 2.0, mid_y, (scope_box.w - 4.0).max(0.0), 1.0),
        theme.border.grid_beat,
    );
    for frac in [0.25_f32, 0.75] {
        dl.fill(
            Rect::new(
                scope_box.x + 2.0,
                scope_box.y + scope_box.h * frac,
                (scope_box.w - 4.0).max(0.0),
                1.0,
            ),
            theme.border.grid_step,
        );
    }

    if view.waveform.len() >= 2 {
        let pad = 4.0;
        let w = (scope_box.w - pad * 2.0).max(1.0);
        let half_h = (scope_box.h * 0.44).max(1.0);
        let n = view.waveform.len();
        let points: Vec<(f32, f32)> = view
            .waveform
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let x = scope_box.x + pad + w * (i as f32 / (n - 1) as f32);
                let y = mid_y - s.clamp(-1.0, 1.0) * half_h;
                (x, y)
            })
            .collect();
        dl.polyline(
            &points,
            1.75,
            theme.accent.routing,
            theme.glow.scope,
            Some((mid_y, 0.16)),
        );
    } else {
        dl.text(
            scope_box,
            "Awaiting engine waveform",
            theme.font.small,
            false,
            theme.text.muted,
            TextAlign::Center,
        );
    }

    // Right stereo VU meters on -60..+6 dBFS scale with 0 dB hairline & pre-fader input ticks.
    let bar_h = 16.0;
    let l_track = draw_meter_bar(
        dl,
        Rect::new(meter_area.x, meter_area.y + 1.0, meter_area.w, bar_h),
        &MeterChannelData {
            label: "L",
            input_peak: view.input_left,
            input_hold: view.input_hold_left,
            peak: view.peak_left,
            hold: view.hold_left,
            observed: view.observed,
        },
        theme,
    );
    let _ = draw_meter_bar(
        dl,
        Rect::new(meter_area.x, meter_area.y + 19.0, meter_area.w, bar_h),
        &MeterChannelData {
            label: "R",
            input_peak: view.input_right,
            input_hold: view.input_hold_right,
            peak: view.peak_right,
            hold: view.hold_right,
            observed: view.observed,
        },
        theme,
    );

    let show_ruler = meter_area.h >= 62.0 && l_track.w >= 110.0;
    if show_ruler {
        let ruler_y = meter_area.y + 36.0;
        let font_sz = theme.font.small - 2.5;
        for (db, label) in [
            (-48.0_f32, "-48"),
            (-24.0, "-24"),
            (-12.0, "-12"),
            (-6.0, "-6"),
            (0.0, "0"),
            (6.0, "+6"),
        ] {
            let x = l_track.x + l_track.w * db_to_meter_norm(db);
            let is_zero = db == 0.0;
            let (rx, align) = if db <= -48.0 {
                (Rect::new(x - 10.0, ruler_y, 24.0, 11.0), TextAlign::Center)
            } else if db >= 6.0 {
                (
                    Rect::new(l_track.right() - 22.0, ruler_y, 22.0, 11.0),
                    TextAlign::Right,
                )
            } else {
                (Rect::new(x - 13.0, ruler_y, 26.0, 11.0), TextAlign::Center)
            };
            dl.text(
                rx,
                label,
                font_sz,
                is_zero,
                if is_zero {
                    theme.text.secondary
                } else {
                    theme.text.muted
                },
                align,
            );
        }
    }

    let status_y = if show_ruler {
        meter_area.y + 49.0
    } else {
        meter_area.y + 39.0
    };
    if meter_area.bottom() >= status_y + 15.0 {
        let row = Rect::new(meter_area.x, status_y, meter_area.w, 16.0);
        let mut right_edge = row.right();

        // Far-right headroom status badge: !CLIP (red) or HOT (amber).
        if view.clipped || view.hot {
            let (label, accent, glow, w) = if view.clipped {
                ("!CLIP", theme.meter.clip, theme.glow.clip * 0.45, 48.0)
            } else {
                ("HOT", theme.meter.hot, 0.18, 38.0)
            };
            let prect = Rect::new(right_edge - w, row.y + 0.5, w, row.h - 1.0);
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
                theme.font.small - 1.5,
                true,
                accent,
                TextAlign::Center,
            );
            right_edge = prect.x - 6.0;
        }

        // Live Master dynamics GR readout pill (Compressor / Limiter).
        if let Some(tag) = view.master_gr_tag {
            let active_gr = view.master_gr_db > 0.05 || view.master_gr_hold_db > 0.05;
            let gr_text = if view.master_gr_hold_db > view.master_gr_db + 0.2 {
                format!(
                    "{tag} -{:.1} (pk -{:.1})",
                    view.master_gr_db, view.master_gr_hold_db
                )
            } else if active_gr {
                format!("{tag} -{:.1} dB", view.master_gr_db)
            } else {
                format!("{tag} 0.0 dB")
            };
            let font_sz = theme.font.small - 1.5;
            let gr_w = (gr_text.chars().count() as f32 * theme.char_width(font_sz) + 12.0)
                .clamp(58.0, 140.0);
            if right_edge - gr_w >= row.x + 80.0 {
                let gr_col = if view.master_gr_db >= 6.0 || view.master_gr_hold_db >= 9.0 {
                    theme.meter.clip
                } else if active_gr {
                    theme.meter.gr
                } else {
                    theme.text.muted
                };
                let grect = Rect::new(right_edge - gr_w, row.y + 0.5, gr_w, row.h - 1.0);
                dl.panel(
                    grect,
                    if active_gr {
                        gr_col.with_alpha(0.16)
                    } else {
                        theme.surface.recessed
                    },
                    gr_col.with_alpha(if active_gr { 0.70 } else { 0.35 }),
                    theme.radii.badge,
                    1.0,
                    if active_gr { 0.14 } else { 0.0 },
                );
                dl.text(
                    grect,
                    gr_text,
                    font_sz,
                    active_gr,
                    gr_col,
                    TextAlign::Center,
                );
                right_edge = grect.x - 6.0;
            }
        }

        let left_w = (right_edge - row.x).max(0.0);
        let hold_str = match (view.hold_left, view.hold_right) {
            (Some(l), Some(r)) => {
                let pre = match (view.input_hold_left, view.input_hold_right) {
                    (Some(il), Some(ir)) => format!(" · Pre {}", fmt_db(il.max(ir))),
                    _ => String::new(),
                };
                format!("Hold L {} R {}{pre}", fmt_db(l), fmt_db(r))
            }
            _ => "Unity 32-bit float · 0 dBFS DAC/WAV".into(),
        };
        let max_chars = (left_w / theme.char_width(theme.font.small - 0.5)).max(8.0) as usize;
        dl.text(
            Rect::new(row.x, row.y, left_w, row.h),
            trim_label(&hold_str, max_chars),
            theme.font.small - 0.5,
            view.clipped,
            if view.clipped {
                theme.meter.clip
            } else if view.hot {
                theme.meter.hot
            } else {
                theme.text.muted
            },
            TextAlign::Left,
        );
    }

    dl.pop_clip();
}

struct MeterChannelData<'a> {
    label: &'a str,
    input_peak: Option<f32>,
    input_hold: Option<f32>,
    peak: f32,
    hold: Option<f32>,
    observed: bool,
}

fn draw_meter_bar(
    dl: &mut DisplayList,
    rect: Rect,
    ch: &MeterChannelData<'_>,
    theme: &Theme,
) -> Rect {
    let MeterChannelData {
        label,
        input_peak,
        input_hold,
        peak,
        hold,
        observed,
    } = *ch;
    dl.text(
        Rect::new(rect.x, rect.y, 14.0, rect.h),
        label,
        theme.font.small,
        true,
        theme.accent.routing,
        TextAlign::Left,
    );
    let readout_w = 68.0;
    let track = Rect::new(
        rect.x + 16.0,
        rect.y + 3.0,
        (rect.w - 16.0 - readout_w - 6.0).max(0.0),
        (rect.h - 6.0).max(4.0),
    );
    dl.rounded(track, theme.meter.track_bg, 2.0);

    // Subtle DAC/WAV ceiling headroom zone above 0 dBFS.
    let zero_x = track.x + track.w * METER_ZERO_DB_NORM;
    if track.right() > zero_x + 1.0 {
        dl.fill(
            Rect::new(zero_x, track.y, (track.right() - zero_x).max(0.0), track.h),
            theme.meter.clip.with_alpha(0.11),
        );
    }

    // Subtle dB graticule ticks (-48, -24, -12, -6 dB).
    for db in METER_GRATICULE_DB {
        let gx = track.x + track.w * db_to_meter_norm(db);
        dl.fill(
            Rect::new(gx, track.y + 1.0, 1.0, (track.h - 2.0).max(1.0)),
            theme.border.grid_step,
        );
    }

    if observed {
        // Slim pre-insert/pre-fader input indicator along top edge of meter track.
        if let Some(inp) = input_peak {
            let in_norm = db_norm(inp);
            let in_w = (track.w * in_norm).clamp(0.0, track.w);
            if in_w > 0.5 {
                let in_col = if inp > 1.0 {
                    theme.meter.hot
                } else {
                    theme.meter.pre_fader
                };
                dl.fill(
                    Rect::new(track.x, track.y, in_w, 2.0),
                    in_col.with_alpha(0.75),
                );
            }
        }
        if let Some(ih) = input_hold {
            let ihn = db_norm(ih);
            if ihn > 0.02 {
                let ihx = track.x + (track.w * ihn).min(track.w - 2.0);
                dl.fill(
                    Rect::new(ihx, track.y - 1.5, 2.0, 4.0),
                    if ih > 1.0 {
                        theme.meter.hot
                    } else {
                        theme.meter.pre_fader
                    },
                );
            }
        }

        // Main post-insert/post-fader Master bar.
        let norm = db_norm(peak);
        let fill_w = (track.w * norm).clamp(0.0, track.w);
        if fill_w > 0.5 {
            let color = if peak > 1.0 {
                theme.meter.clip
            } else if peak >= 0.5 {
                theme.meter.high
            } else {
                theme.meter.mid
            };
            let glow = if peak > 1.0 { theme.glow.clip } else { 0.22 };
            dl.rounded_glow(
                Rect::new(track.x, track.y + 2.0, fill_w, (track.h - 2.0).max(2.0)),
                color,
                1.5,
                glow,
            );
        }
        if let Some(h) = hold {
            let hn = db_norm(h);
            if hn > 0.02 {
                let hx = track.x + (track.w * hn).min(track.w - 2.0);
                dl.fill(
                    Rect::new(hx, track.y - 0.5, 2.0, track.h + 1.5),
                    if h > 1.0 {
                        theme.meter.clip
                    } else {
                        theme.meter.hold_tick
                    },
                );
            }
        }
    }

    // Crisp 0 dB unity hairline drawn on top so ceiling is always legible.
    dl.fill(
        Rect::new(zero_x, track.y - 1.0, 1.0, track.h + 2.0),
        theme.meter.zero_db.with_alpha(0.70),
    );

    let db_text = if observed {
        format!("{} dB", fmt_db(peak))
    } else {
        "-- dB".into()
    };
    dl.text(
        Rect::new(rect.right() - readout_w, rect.y, readout_w, rect.h),
        db_text,
        theme.font.small,
        observed && peak > 1.0,
        if observed && peak > 1.0 {
            theme.meter.clip
        } else {
            theme.text.secondary
        },
        TextAlign::Right,
    );
    track
}

pub fn db_to_meter_norm(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - METER_MIN_DB) / (METER_MAX_DB - METER_MIN_DB)).clamp(0.0, 1.0)
}

pub fn db_norm(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= LIN_FLOOR_F32 {
        return 0.0;
    }
    db_to_meter_norm(lin_to_db(peak))
}

pub fn fmt_db(peak: f32) -> String {
    cadence_core::dsp::format_db_value(peak)
}
