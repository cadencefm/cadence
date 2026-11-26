//! Modal overlays and floating performance HUDs: Sample Browser (with GPU waveform preview),
//! Authoring/Workflow draft forms, ValueInput, Picker, Event Provenance, Confirm, Help,
//! Settings, File Hub, Inspect/Assets, Tutorial, WhichKey Prefix Menus, Jam HUD, and Lane Layer HUD.
mod authoring;
mod browser;
mod file_hub;
mod groove_browser;
mod help;
mod hud;
mod mini_editor;
mod palette;
mod preset_browser;
mod settings;
mod song_props;

pub use hud::{draw_jam_hud, draw_lane_layer_hud};

use crate::display::{DisplayList, Rect, TextAlign};
use crate::theme::{Color, Theme};
use cadence_app::view::OverlayView;

pub fn draw_overlay(dl: &mut DisplayList, viewport: Rect, overlay: &OverlayView, theme: &Theme) {
    match overlay {
        OverlayView::PrefixMenu { title, items } => {
            hud::draw_prefix_menu(dl, viewport, title, items, theme);
        }
        OverlayView::Browser(browser) => {
            draw_scrim(dl, viewport, theme);
            browser::draw_browser(dl, centered(viewport, 920.0, 560.0), browser, theme);
        }
        OverlayView::PresetBrowser(pb) => {
            draw_scrim(dl, viewport, theme);
            preset_browser::draw_preset_browser(dl, centered(viewport, 920.0, 560.0), pb, theme);
        }
        OverlayView::GrooveBrowser(gb) => {
            draw_scrim(dl, viewport, theme);
            groove_browser::draw_groove_browser(dl, centered(viewport, 920.0, 560.0), gb, theme);
        }
        OverlayView::SongProperties(sp) => {
            draw_scrim(dl, viewport, theme);
            song_props::draw_song_properties(dl, centered(viewport, 840.0, 624.0), sp, theme);
        }
        OverlayView::MiniEditor(me) => {
            draw_scrim(dl, viewport, theme);
            mini_editor::draw_mini_editor(dl, centered(viewport, 780.0, 310.0), me, theme);
        }
        OverlayView::Palette(pal) => {
            draw_scrim(dl, viewport, theme);
            palette::draw_palette(dl, centered(viewport, 780.0, 510.0), pal, theme);
        }
        OverlayView::Authoring(draft) => {
            draw_scrim(dl, viewport, theme);
            authoring::draw_authoring(dl, centered(viewport, 780.0, 440.0), draft, theme);
        }
        OverlayView::ValueInput(input) => {
            draw_scrim(dl, viewport, theme);
            authoring::draw_value_input(dl, centered(viewport, 540.0, 248.0), input, theme);
        }
        OverlayView::Picker(picker) => {
            draw_scrim(dl, viewport, theme);
            browser::draw_picker(dl, centered(viewport, 640.0, 420.0), picker, theme);
        }
        OverlayView::EventInspection(insp) => {
            draw_scrim(dl, viewport, theme);
            hud::draw_event_inspection(dl, centered(viewport, 740.0, 420.0), insp, theme);
        }
        OverlayView::Confirm { title, message } => {
            draw_scrim(dl, viewport, theme);
            authoring::draw_confirm(dl, centered(viewport, 520.0, 150.0), title, message, theme);
        }
        OverlayView::Help(help) => {
            draw_scrim(dl, viewport, theme);
            help::draw_help(dl, centered(viewport, 1120.0, 590.0), help, theme);
        }
        OverlayView::Settings(settings) => {
            draw_scrim(dl, viewport, theme);
            settings::draw_settings(dl, centered(viewport, 760.0, 540.0), settings, theme);
        }
        OverlayView::FileHub(hub) => {
            draw_scrim(dl, viewport, theme);
            file_hub::draw_file_hub(dl, centered(viewport, 1020.0, 610.0), hub, theme);
        }
        OverlayView::Inspect {
            title,
            lines,
            scroll,
        } => {
            draw_scrim(dl, viewport, theme);
            hud::draw_inspect(
                dl,
                centered(viewport, 820.0, 540.0),
                title,
                lines,
                *scroll,
                theme,
            );
        }
        OverlayView::Tutorial {
            step,
            total,
            title,
            body,
            command,
        } => {
            draw_scrim(dl, viewport, theme);
            hud::draw_tutorial(
                dl,
                centered(viewport, 680.0, 280.0),
                &hud::TutorialView {
                    step: *step,
                    total: *total,
                    title,
                    body,
                    command,
                },
                theme,
            );
        }
    }
}

pub(super) fn draw_scrim(dl: &mut DisplayList, viewport: Rect, theme: &Theme) {
    dl.fill(viewport, theme.surface.overlay_scrim);
}

pub(super) fn centered(viewport: Rect, max_w: f32, max_h: f32) -> Rect {
    let w = max_w.min((viewport.w - 48.0).max(260.0));
    let h = max_h.min((viewport.h - 64.0).max(120.0));
    Rect::new(
        viewport.x + (viewport.w - w) * 0.5,
        viewport.y + (viewport.h - h) * 0.5,
        w,
        h,
    )
}

pub(super) fn overlay_frame(
    dl: &mut DisplayList,
    rect: Rect,
    title: &str,
    accent: Color,
    theme: &Theme,
) -> Rect {
    dl.panel(
        rect,
        theme.surface.overlay,
        accent,
        theme.radii.panel,
        1.5,
        theme.glow.focus_border,
    );
    let (hdr, body) = rect.inset(1.5, 1.5).split_top(28.0);
    dl.fill(hdr, accent.with_alpha(0.14));
    dl.fill(
        Rect::new(hdr.x, hdr.bottom() - 1.0, hdr.w, 1.0),
        theme.border.hairline,
    );
    dl.text(
        hdr.inset(12.0, 0.0),
        title,
        theme.font.body,
        true,
        accent,
        TextAlign::Left,
    );
    body.inset(12.0, 10.0)
}

/// Compute a clamped scroll start index that centers `selected` within `visible` rows.
pub(super) fn scroll_window_centered(selected: usize, total: usize, visible: usize) -> usize {
    selected
        .saturating_sub(visible / 2)
        .min(total.saturating_sub(visible))
}

/// Clamp an explicit scroll offset so the last page stays pinned cleanly to the bottom.
pub(super) fn scroll_window_clamped(scroll: usize, total: usize, visible: usize) -> usize {
    scroll.min(total.saturating_sub(visible))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cadence_app::authoring::{DraftField, DraftFieldView, DraftInput, DraftView};
    use cadence_app::PickerView;

    #[test]
    fn draw_overlay_authoring_left_pane_scrolls_and_picker_centers_cursor() {
        let theme = Theme::cadence_dark();
        let viewport = Rect::new(0.0, 0.0, 1000.0, 360.0);

        let fields: Vec<DraftFieldView> = (0..14)
            .map(|i| DraftFieldView {
                label: format!("Field{i:02}"),
                value: format!("Val{i:02}"),
                active: i == 11,
            })
            .collect();
        let draft = DraftView {
            title: "PLACED PART".into(),
            field: DraftField::Start,
            field_label: "Start bar".into(),
            input: DraftInput::Number,
            text: "1".into(),
            replace_on_type: true,
            query: String::new(),
            searching: false,
            candidates: Vec::new(),
            selected: None,
            fields,
            preview: vec!["Preview".into()],
            summary: "Summary".into(),
            error: None,
            hints: vec![("Enter", "accept")],
        };
        let mut dl = DisplayList::new(viewport.w, viewport.h, 1.0);
        dl.begin_overlay();
        draw_overlay(&mut dl, viewport, &OverlayView::Authoring(draft), &theme);
        let auth_texts: Vec<&str> = dl.overlay.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(
            auth_texts.iter().any(|t| t.contains("Field11")),
            "Active field #11 must be scrolled into view: {auth_texts:?}"
        );
        assert!(
            auth_texts
                .iter()
                .any(|t| t.contains("▲") && t.contains("12/14")),
            "Expected vertical scroll indicator (`▲ ... 12/14`) on left field list, got: {auth_texts:?}"
        );

        let picker = PickerView {
            title: "ROUTE CHANNEL".into(),
            filter: None,
            searching: false,
            cursor: 15,
            audition: false,
            items: (0..24)
                .map(|i| (format!("Channel {i:02}"), "Group".into(), i == 15))
                .collect(),
        };
        let mut dl_p = DisplayList::new(1000.0, 420.0, 1.0);
        dl_p.begin_overlay();
        draw_overlay(
            &mut dl_p,
            Rect::new(0.0, 0.0, 1000.0, 420.0),
            &OverlayView::Picker(picker),
            &theme,
        );
        let pick_texts: Vec<&str> = dl_p.overlay.texts.iter().map(|t| t.text.as_str()).collect();
        assert!(
            pick_texts.contains(&"Channel 15")
                && pick_texts.contains(&"Channel 14")
                && pick_texts.contains(&"Channel 16"),
            "Picker scroll must center around cursor 15 (showing neighbors 14 & 16), got: {pick_texts:?}"
        );
        assert!(
            !pick_texts.contains(&"Channel 00"),
            "Picker must have scrolled past item 00"
        );
    }
}
