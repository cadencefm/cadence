//! Project document lifecycle, sample reloading, export, and structured JSON request responses.
use crate::*;

impl App {
    pub(super) fn apply_inspect_assets(&self) -> Outcome {
        Outcome {
            inspection: Some(self.inspect_assets()),
            ..Outcome::default()
        }
    }

    pub(super) fn apply_reload_samples(&mut self) -> Outcome {
        self.assets.library.invalidate_decoded_cache();
        self.sync_loaded_samples();
        self.status = format!(
            "Reloaded samples · {} unavailable",
            self.assets.issues.len()
        );
        Outcome {
            samples_changed: true,
            ..Outcome::default()
        }
    }

    pub(super) fn apply_save(&mut self, path: PathBuf) -> Result<Outcome> {
        self.save_project(&path)?;
        self.doc.mark_saved();
        self.doc.set_save_path(path.clone());
        self.record_recent_project(&path);
        self.maybe_persist_config();
        self.status = format!("Saved {}", path.display());
        Ok(Outcome::default())
    }

    pub(super) fn apply_load(&mut self, path: PathBuf) -> Result<Outcome> {
        if cadence_formats::find_import_format_for_path(&path).is_some() {
            return self.apply_import_rbs(path);
        }
        let (project, flags) = load_document_with_io(self.io.as_ref(), &path)?;
        let base = absolute_parent_with_io(self.io.as_ref(), &path)?;
        self.record_recent_project(&path);
        self.maybe_persist_config();
        self.replace_document(project, flags, base, path, false)
    }

    pub(super) fn apply_import_rbs(&mut self, path: PathBuf) -> Result<Outcome> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("rbs");
        let Some(spec) = cadence_formats::find_import_format_by_ext(ext) else {
            anyhow::bail!(
                "ReBirth .rbs support is disabled in this build (compile with --features rbs)"
            );
        };
        let bytes = self
            .io
            .read_bounded(&path, spec.max_bytes)
            .with_context(|| format!("Failed to read external song {}", path.display()))?;
        let fallback = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("ReBirth Song");
        let imported = cadence_formats::import_bytes_with_presets(
            ext,
            &bytes,
            fallback,
            &self.assets.presets,
        )?;
        let base = absolute_parent_with_io(self.io.as_ref(), &path)?;
        let json_path = path.with_extension("cadence.json");
        self.record_recent_project(&path);
        self.maybe_persist_config();
        let out = self.replace_document(
            imported.project,
            LiveFlags::default(),
            base,
            json_path,
            true,
        )?;
        self.status = imported.status_line;
        Ok(out)
    }

    pub(super) fn apply_new(&mut self, title: Option<String>) -> Result<Outcome> {
        let mut project = Project::empty_with_config(&self.config);
        if let Some(title) = title {
            project.title = title;
        }
        let untitled_path = self.doc.asset_base().join("untitled.json");
        self.replace_document(
            project,
            LiveFlags::default(),
            self.doc.asset_base().clone(),
            untitled_path,
            true,
        )
    }

    pub(super) fn apply_export(&mut self, path: PathBuf, bars: Option<u32>) -> Result<Outcome> {
        self.ensure_samples_ready()?;
        let bars = bars.unwrap_or(self.doc.project().bars as u32);
        self.status = format!("Rendered {}", path.display());
        Ok(Outcome {
            export: Some(ExportRequest {
                path,
                bars,
                start_step: 0,
                steps: u64::from(bars) * 16,
                sample_rate: 48_000,
                mode: ExportMode::MasterWav,
                use_take: false,
            }),
            ..Outcome::default()
        })
    }

    /// Full structured state JSON for standard `--script`, `--stdio`, and `--ctl` responses.
    pub fn full_state_response_json(
        &self,
        id: Option<serde_json::Value>,
        inspection: Option<serde_json::Value>,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "ok": true,
            "revision": self.doc.revision(),
            "message": self.status,
            "status": self.status,
            "dirty": self.doc.dirty(),
            "bpm": self.doc.project().bpm,
            "project": self.doc.project(),
            "flags": self.doc.flags(),
            "selection": {
                "workspace": self.workspace(),
                "arrange_view": self.arrange_ui.view,
                "arrange_step": self.arrange_ui.step,
                "event": self.selected_arrangement_event().map(|n| n.key),
                "pattern": self.selected_pattern,
                "lane": self.selected_lane,
                "placement": self.selected_placement,
                "part": self.selected_part,
                "instrument": self.selected_instrument,
                "channel": self.selected_channel,
                "scope": self.scope,
                "deck": {
                    "target": self.detail,
                    "visible": self.view_state.deck_visible,
                    "pinned": self.view_state.deck_pinned,
                    "zoomed": self.view_state.deck_zoomed,
                }
            },
            "inspection": inspection,
            "ready": self.ready(),
            "sample_issues": self.assets.issues,
            "required_sample_issues": self.required_sample_issues(),
            "audition": self.audition_pattern(),
            "follow": self.follow_mode(),
            "live": self.live_state_json(),
        })
    }

    /// Lightweight response JSON omitting the full `Project` / `LiveFlags` dump for fast polling.
    pub fn compact_state_response_json(
        &self,
        id: Option<serde_json::Value>,
        inspection: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let mut reply = serde_json::json!({
            "id": id,
            "ok": true,
            "revision": self.doc.revision(),
            "playing": self.transport.playing,
            "step": self.transport.step,
            "bpm": self.doc.project().bpm,
            "status": self.status,
            "message": self.status,
            "dirty": self.doc.dirty(),
            "ready": self.ready(),
            "channel_meters": serde_json::json!([]),
            "effect_meters": serde_json::json!([]),
        });
        if let Some(insp) = inspection {
            reply["inspection"] = insp;
        }
        reply
    }

    pub fn execute_request(&mut self, request: Request) -> (serde_json::Value, Outcome) {
        let result = (|| {
            if let Some(revision) = request.expected_revision {
                ensure!(
                    revision == self.doc.revision(),
                    "Revision conflict: expected {revision}, current {}",
                    self.doc.revision()
                );
            }
            self.apply(request.action)
        })();
        match result {
            Ok(out) => {
                let reply = if request.compact {
                    self.compact_state_response_json(request.id, out.inspection.clone())
                } else {
                    self.full_state_response_json(request.id, out.inspection.clone())
                };
                (reply, out)
            }
            Err(error) => (
                serde_json::json!({"id":request.id,"ok":false,"revision":self.doc.revision(),"error":format!("{error:#}")}),
                Outcome::default(),
            ),
        }
    }
}
