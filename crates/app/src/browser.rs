//! Bounded asynchronous sample browsing. Selection/decoding is session state;
//! only explicit acceptance writes an instrument patch or a pending workflow.
use super::*;
use crate::state::SuspendedWorkflow;
use std::sync::atomic::Ordering;
use std::sync::{
    atomic::AtomicU64,
    mpsc::{self, Receiver, SyncSender},
};
use std::thread;

#[derive(Clone, Copy, Debug)]
pub(super) enum BrowserTarget {
    Workflow,
    AssignModule {
        instrument: Id,
        zone_id: u16,
        module_id: Option<u16>,
    },
    AddZone {
        instrument: Id,
    },
    NewSampler,
}
#[derive(Clone, Debug)]
pub struct BrowserEntryView {
    pub index: usize,
    pub label: String,
    pub category: String,
    pub path: String,
    pub selected: bool,
}
#[derive(Clone, Debug)]
pub struct BrowserView {
    pub title: String,
    pub destination: String,
    pub query: String,
    pub searching: bool,
    pub total: usize,
    pub filtered: usize,
    pub first: usize,
    pub selected: usize,
    pub entries: Vec<BrowserEntryView>,
    pub loading: bool,
    pub error: Option<String>,
    pub sample: Option<Arc<SampleData>>,
    pub status: String,
}
#[derive(Clone, Debug)]
struct BrowserJob {
    token: u64,
    path: PathBuf,
    label: String,
}
struct BrowserResult {
    job: BrowserJob,
    result: Result<Arc<SampleData>, String>,
}
pub(super) struct BrowserLoader {
    tx: SyncSender<BrowserJob>,
    rx: Receiver<BrowserResult>,
    active: Arc<AtomicU64>,
}
impl BrowserLoader {
    fn new(io: Arc<dyn HostIo>) -> Result<Self> {
        Self::with_decoder(move |job| {
            io.load_sample_file(&job.path, Some(job.label.clone()))
                .map_err(|error| format!("{error:#}"))
        })
    }
    fn with_decoder(
        decode: impl Fn(&BrowserJob) -> Result<Arc<SampleData>, String> + Send + 'static,
    ) -> Result<Self> {
        let (tx, jobs) = mpsc::sync_channel::<BrowserJob>(1);
        let (results, rx) = mpsc::sync_channel(1);
        let active = Arc::new(AtomicU64::new(0));
        let worker_active = Arc::clone(&active);
        thread::Builder::new()
            .name("cadence-sample-browser".into())
            .spawn(move || {
                while let Ok(job) = jobs.recv() {
                    if worker_active.load(Ordering::Acquire) != job.token {
                        continue;
                    }
                    // A codec panic must not leave a pending acceptance waiting
                    // forever or prevent later files from being browsed.
                    let result =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&job)))
                            .unwrap_or_else(|_| {
                                Err(
                            "Sample decoder failed unexpectedly; choose another file or reload"
                                .into(),
                        )
                            });
                    if worker_active.load(Ordering::Acquire) != job.token {
                        continue;
                    }
                    if results.send(BrowserResult { job, result }).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { tx, rx, active })
    }
}
pub(super) struct BrowserState {
    target: BrowserTarget,
    origin: Navigation,
    pub(super) searching: bool,
    selected_path: Option<PathBuf>,
    sample: Option<Arc<SampleData>>,
    error: Option<String>,
    loading: bool,
    job: Option<BrowserJob>,
    submitted: bool,
    audition_when_ready: bool,
    accept_when_ready: bool,
}
impl App {
    pub fn is_sample_browser(&self) -> bool {
        self.input.mode == Mode::Browser && self.modal.browser().is_some()
    }
    pub fn browser_busy(&self) -> bool {
        self.is_sample_browser() && self.modal.browser().is_some_and(|b| b.loading)
    }
    pub fn browser_accept_pending(&self) -> bool {
        self.is_sample_browser() && self.modal.browser().is_some_and(|b| b.accept_when_ready)
    }
    pub fn browser_view(&self) -> Option<BrowserView> {
        let state = self
            .modal
            .browser()
            .filter(|_| self.input.mode == Mode::Browser)?;
        let selected = self.assets.library.cursor;
        let first = selected.saturating_sub(30);
        let entries = self
            .assets
            .library
            .visible_entries()
            .skip(first)
            .take(61)
            .map(|(index, e)| BrowserEntryView {
                index,
                label: e.label.clone(),
                category: e.category.clone(),
                path: e.path.to_string_lossy().into_owned(),
                selected: index == selected,
            })
            .collect();
        let destination = match state.target {
            BrowserTarget::AssignModule {
                instrument,
                zone_id,
                module_id,
            } => {
                let (inst_name, zone_name, mod_label) = self
                    .instrument(instrument)
                    .map(|i| {
                        let z = i.rack.zone(zone_id);
                        let zn = z.map_or("zone", |z| z.name.as_str());
                        let ml = module_id
                            .and_then(|mid| z.and_then(|z| z.module(mid)).map(|m| m.name.as_str()))
                            .unwrap_or("Sampler");
                        (i.name.as_str(), zn, ml)
                    })
                    .unwrap_or(("missing", "zone", "Sampler"));
                format!("Assign to {inst_name} · {zone_name} · {mod_label}")
            }
            BrowserTarget::AddZone { instrument } => {
                let inst_name = self
                    .instrument(instrument)
                    .map_or("missing", |i| i.name.as_str());
                format!("Add new zone to {inst_name}")
            }
            BrowserTarget::Workflow => format!(
                "Choose for {}",
                self.modal
                    .browser_return_workflow()
                    .map_or("instrument draft".into(), |d| d
                        .sample_target_label(self.doc.project()))
            ),
            BrowserTarget::NewSampler => "Choose to create a Sampler Rack".into(),
        };
        Some(BrowserView {
            title: "SAMPLE BROWSER".into(),
            destination,
            query: self.assets.library.query.clone(),
            searching: state.searching,
            total: self.assets.library.total_count(),
            filtered: self.assets.library.filtered_count(),
            first,
            selected,
            entries,
            loading: state.loading,
            error: state.error.clone(),
            sample: state.sample.clone(),
            status: if state.accept_when_ready {
                "Preparing the chosen sample before acceptance…".into()
            } else if state.audition_when_ready {
                "Preparing audition…".into()
            } else if state.loading {
                "Loading waveform and metadata…".into()
            } else if state.searching {
                "Type to filter · Enter apply filter · Esc clear search".into()
            } else if !self.assets.library.query.is_empty() {
                "A audition · Enter choose · Esc clear filter".into()
            } else {
                "A audition · Enter choose · Esc back".into()
            },
        })
    }
    pub(super) fn browse_samples(&mut self) -> Result<Outcome> {
        let target = if let Some((instrument, zone_id, module_id)) = self.focused_sample_target() {
            BrowserTarget::AssignModule {
                instrument,
                zone_id,
                module_id: Some(module_id),
            }
        } else if let Some(inst_id) = self.active_instrument() {
            if let Some(inst) = self.instrument(inst_id) {
                let z_idx = self.active_zone_index(&inst.rack);
                if let Some(zone) = inst.rack.zones.get(z_idx) {
                    // Prefer the currently focused Sampler module (Panel pill or Patchbay focus),
                    // else the first Sampler module in the active zone, else None (auto-adds Sampler).
                    let focused_mod = match self.rack_ui.tab {
                        RackTab::Panel => self.active_panel_pill(inst_id).and_then(|(_, m)| m),
                        RackTab::Patchbay => self.focused_patchbay_module_in(zone),
                        RackTab::Zones => None,
                    };
                    let module_id = focused_mod
                        .filter(|&mid| {
                            zone.module(mid)
                                .is_some_and(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                        })
                        .or_else(|| {
                            zone.modules
                                .iter()
                                .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                                .map(|m| m.id)
                        });
                    BrowserTarget::AssignModule {
                        instrument: inst_id,
                        zone_id: zone.id,
                        module_id,
                    }
                } else {
                    BrowserTarget::NewSampler
                }
            } else {
                BrowserTarget::NewSampler
            }
        } else {
            BrowserTarget::NewSampler
        };
        self.begin_sample_browser(target)
    }
    pub(super) fn begin_sample_browser(&mut self, target: BrowserTarget) -> Result<Outcome> {
        if self.assets.browser_loader.is_none() {
            self.assets.browser_loader = Some(BrowserLoader::new(Arc::clone(&self.io))?);
        }
        let preferred = match target {
            BrowserTarget::AssignModule {
                instrument,
                zone_id,
                module_id,
            } => self
                .instrument(instrument)
                .and_then(|i| i.rack.zone(zone_id))
                .and_then(|z| {
                    let m = module_id.and_then(|mid| z.module(mid)).or_else(|| {
                        z.modules
                            .iter()
                            .find(|m| matches!(m.kind, ModuleKind::Sampler(_)))
                    })?;
                    match &m.kind {
                        ModuleKind::Sampler(smp) => smp.path.as_deref(),
                        _ => None,
                    }
                })
                .and_then(|p| self.assets.library.resolve_path(p)),
            BrowserTarget::Workflow => self
                .modal
                .workflow()
                .and_then(|d| d.selected_sample())
                .and_then(|(p, _)| self.assets.library.resolve_path(p)),
            BrowserTarget::AddZone { .. } | BrowserTarget::NewSampler => None,
        };
        if let Some(path) = preferred {
            let canonical_pref = self.io.canonicalize(&path).unwrap_or(path);
            let position = self
                .assets
                .library
                .visible_entries()
                .find(|(_, e)| {
                    e.path == canonical_pref
                        || self
                            .io
                            .canonicalize(&e.path)
                            .is_ok_and(|c| c == canonical_pref)
                })
                .map(|(i, _)| i);
            if let Some(position) = position {
                self.assets.library.cursor = position;
            }
        }
        let return_workflow = if matches!(target, BrowserTarget::Workflow) {
            self.modal.take_workflow().map(|(draft, origin)| {
                Box::new(SuspendedWorkflow {
                    draft: *draft,
                    origin,
                })
            })
        } else {
            None
        };
        let origin = self.navigation();
        self.modal = ActiveModal::Browser {
            state: Box::new(BrowserState {
                target,
                origin,
                searching: false,
                selected_path: None,
                sample: None,
                error: None,
                loading: false,
                job: None,
                submitted: false,
                audition_when_ready: false,
                accept_when_ready: false,
            }),
            return_workflow,
        };
        self.input.mode = Mode::Browser;
        self.input.count = None;
        self.refresh_browser_selection(false)?;
        let mut out = self.release_held();
        out.stop_audition = true;
        Ok(out)
    }
    fn refresh_browser_selection(&mut self, force: bool) -> Result<()> {
        if force {
            self.assets.library.invalidate_decoded_cache();
        }
        let selected = self.assets.library.selected_entry().map(|e| {
            (
                self.io
                    .canonicalize(&e.path)
                    .unwrap_or_else(|_| e.path.clone()),
                e.label.clone(),
            )
        });
        let state = self.modal.browser_mut().context("Browser missing")?;
        if !force && selected.as_ref().map(|(p, _)| p) == state.selected_path.as_ref() {
            return Ok(());
        }
        self.assets.browser_generation = self
            .assets
            .browser_generation
            .checked_add(1)
            .context("Browser request identity exhausted")?;
        if let Some(loader) = &self.assets.browser_loader {
            loader
                .active
                .store(self.assets.browser_generation, Ordering::Release);
        }
        state.sample = None;
        state.error = None;
        state.loading = false;
        state.job = None;
        state.submitted = false;
        state.audition_when_ready = false;
        state.accept_when_ready = false;
        state.selected_path = selected.as_ref().map(|(p, _)| p.clone());
        if let Some((path, label)) = selected {
            if !force {
                if let Some(sample) = self.assets.library.cached_path(&path) {
                    state.sample = Some(sample);
                    return Ok(());
                }
            }
            state.loading = true;
            state.job = Some(BrowserJob {
                token: self.assets.browser_generation,
                path,
                label,
            });
            self.submit_browser_job();
        }
        Ok(())
    }
    fn submit_browser_job(&mut self) {
        let Some(state) = self.modal.browser() else {
            return;
        };
        if !state.loading || state.submitted {
            return;
        }
        let Some(job) = state.job.clone() else {
            return;
        };
        let loader = match self.assets.browser_loader.as_ref() {
            Some(loader) => loader,
            None => match BrowserLoader::new(Arc::clone(&self.io)) {
                Ok(loader) => self.assets.browser_loader.insert(loader),
                Err(error) => {
                    self.fail_browser_loader(format!("Could not start sample decoder: {error:#}"));
                    return;
                }
            },
        };
        loader.active.store(job.token, Ordering::Release);
        match loader.tx.try_send(job) {
            Ok(()) => {
                if let Some(state) = self.modal.browser_mut() {
                    state.submitted = true;
                }
            }
            Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.fail_browser_loader(
                    "Sample decoder disconnected; reload or choose another file to retry".into(),
                );
            }
        }
    }
    fn fail_browser_loader(&mut self, error: String) {
        if let Some(loader) = self.assets.browser_loader.take() {
            loader.active.store(0, Ordering::Release);
        }
        if let Some(state) = self.modal.browser_mut() {
            if state.loading {
                state.loading = false;
                state.job = None;
                state.submitted = false;
                state.accept_when_ready = false;
                state.audition_when_ready = false;
                state.error = Some(error);
            }
        }
    }
    /// Called by live/headless hosts between input and rendering. Never waits on IO.
    pub fn poll_browser_updates(&mut self) -> Result<Outcome> {
        let mut ready = None;
        let mut disconnected = false;
        if let Some(loader) = &self.assets.browser_loader {
            loop {
                let result = match loader.rx.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                };
                if self.is_sample_browser()
                    && self
                        .modal
                        .browser()
                        .and_then(|b| b.job.as_ref())
                        .is_some_and(|j| j.token == result.job.token && j.path == result.job.path)
                {
                    ready = Some(result);
                }
            }
        }
        if disconnected {
            if ready.is_some() {
                // A completed result remains valid even if its worker ended.
                // Future requests will create a fresh worker lazily.
                self.assets.browser_loader = None;
            } else {
                self.fail_browser_loader(
                    "Sample decoder disconnected; reload or choose another file to retry".into(),
                );
            }
        }
        let mut out = Outcome::default();
        if let (Some(result), Some(state)) = (ready, self.modal.browser_mut()) {
            state.loading = false;
            state.job = None;
            state.submitted = false;
            let accept = state.accept_when_ready;
            let audition = state.audition_when_ready;
            state.accept_when_ready = false;
            state.audition_when_ready = false;
            match result.result {
                Ok(sample) => {
                    self.assets
                        .library
                        .cache_prepared(&result.job.path, Arc::clone(&sample));
                    state.sample = Some(Arc::clone(&sample));
                    state.error = None;
                    if accept {
                        return self.accept_browser_selection();
                    }
                    if audition {
                        out.stop_audition = true;
                        out.preview_sample =
                            Some(cadence_core::sampler::SamplePreview::full(sample));
                    }
                }
                Err(error) => state.error = Some(error),
            }
        }
        self.submit_browser_job();
        Ok(out)
    }
    fn browser_move(&mut self, delta: isize) -> Result<Outcome> {
        self.assets.library.move_cursor(delta);
        self.refresh_browser_selection(false)?;
        Ok(Outcome {
            stop_audition: true,
            ..Outcome::default()
        })
    }
    fn browser_query(&mut self, query: String) -> Result<Outcome> {
        let selected = self.assets.library.selected_entry().map(|e| e.path.clone());
        self.assets.library.set_query(query);
        let matching = selected.and_then(|path| {
            self.assets
                .library
                .visible_entries()
                .find(|(_, e)| e.path == path)
                .map(|(i, _)| i)
        });
        self.assets.library.cursor = matching.unwrap_or(0);
        self.refresh_browser_selection(false)?;
        Ok(Outcome {
            stop_audition: true,
            ..Outcome::default()
        })
    }
    fn close_browser(&mut self) -> Outcome {
        if let Some(loader) = &self.assets.browser_loader {
            loader.active.store(0, Ordering::Release);
        }
        if let Some(taken) = self.modal.take_browser() {
            if matches!(taken.state.target, BrowserTarget::Workflow) {
                if let Some(wf) = taken.return_workflow {
                    let SuspendedWorkflow { draft, origin } = *wf;
                    self.modal = ActiveModal::Workflow {
                        draft: Box::new(draft),
                        origin,
                    };
                }
                self.input.mode = Mode::Workflow;
            } else {
                self.input.mode = Mode::Normal;
                self.restore_navigation(taken.state.origin);
            }
        }
        self.status = "Sample browsing cancelled · instrument unchanged".into();
        Outcome {
            stop_audition: true,
            ..Outcome::default()
        }
    }
    fn accept_browser_selection(&mut self) -> Result<Outcome> {
        let Some(state) = self.modal.browser_mut() else {
            return Ok(Outcome::default());
        };
        if state.loading {
            state.accept_when_ready = true;
            return Ok(Outcome::default());
        }
        let Some(sample) = state.sample.clone() else {
            state.error = Some("Choose a decodable sample first".into());
            return Ok(Outcome::default());
        };
        let Some(entry) = self.assets.library.selected_entry().cloned() else {
            return Ok(Outcome::default());
        };
        let exact = match self.io.canonicalize(&entry.path) {
            Ok(path) if self.io.is_file(&path) => path,
            _ => {
                state.error = Some(
                    "The chosen sample is no longer available; reload or choose another file"
                        .into(),
                );
                return Ok(Outcome::default());
            }
        };
        if state.selected_path.as_ref() != Some(&exact) {
            state.error = Some("Sample selection changed; reload or choose again".into());
            return Ok(Outcome::default());
        }
        let canon_base = self
            .io
            .canonicalize(self.doc.asset_base())
            .unwrap_or_else(|_| self.doc.asset_base().clone());
        let path = if let Ok(rel) = exact
            .strip_prefix(self.doc.asset_base())
            .or_else(|_| exact.strip_prefix(&canon_base))
        {
            format!("./{}", rel.to_string_lossy().replace('\\', "/"))
        } else {
            entry.label.clone()
        };
        let target = state.target;
        if let BrowserTarget::AssignModule {
            instrument: id,
            zone_id,
            module_id,
        } = target
        {
            let mut assigned_label = String::new();
            let result = (|| {
                let inst = self
                    .instrument(id)
                    .context("The destination instrument no longer exists")?;
                let inst_name = inst.name.clone();
                let mut rack = inst.rack.clone();
                let zone = rack
                    .zone_mut(zone_id)
                    .context("The destination zone no longer exists")?;
                let sampler_id = Self::ensure_zone_sampler_module(zone, module_id)?;
                zone.apply_sample_defaults(Some(sampler_id), path.clone(), &sample);
                zone.validate()?;
                let mod_name = zone
                    .module(sampler_id)
                    .map_or("Sampler", |m| m.name.as_str());
                assigned_label = format!("{inst_name} · {} · {mod_name}", zone.name);
                self.apply(Action::Edit {
                    edit: Edit::UpdateRackPatch { id, rack },
                })
            })();
            match result {
                Ok(mut out) => {
                    // Accepting reloaded bytes at the same path/metadata is a
                    // resource publication, even when the authored patch is equal.
                    if !out.changed {
                        self.sync_loaded_samples();
                        out.samples_changed = true;
                    }
                    let taken = self.modal.take_browser().context("Browser state missing")?;
                    self.input.mode = Mode::Normal;
                    self.restore_navigation(taken.state.origin);
                    if let Some(loader) = &self.assets.browser_loader {
                        loader.active.store(0, Ordering::Release);
                    }
                    out.stop_audition = true;
                    self.status = format!("Loaded {} into {assigned_label}", entry.name);
                    return Ok(out);
                }
                Err(error) => {
                    if let Some(b) = self.modal.browser_mut() {
                        b.error = Some(format!("{error:#}"));
                    }
                    return Ok(Outcome::default());
                }
            }
        }
        if let BrowserTarget::AddZone { instrument: id } = target {
            let mut created_idx = 0usize;
            let mut created_label = String::new();
            let result = (|| {
                let inst = self
                    .instrument(id)
                    .context("The destination instrument no longer exists")?;
                ensure!(
                    inst.rack.zones.len() < cadence_core::rack::MAX_ZONES,
                    "Rack already has the maximum {} zones",
                    cadence_core::rack::MAX_ZONES
                );
                let inst_name = inst.name.clone();
                let next_id = inst.rack.next_zone_id();
                let free_midi = crate::zones::next_free_midi(&inst.rack);
                let zname = crate::unique_name(
                    &entry.name,
                    inst.rack.zones.iter().map(|z| z.name.as_str()),
                    Some(32),
                );
                let is_pad_kit = inst.rack.zones.iter().any(|z| z.low_midi == z.high_midi);
                let mut zone = if is_pad_kit {
                    RackZone::sample_pad(next_id, zname.clone(), free_midi, &path)
                } else {
                    RackZone::blank_sample(next_id, zname.clone())
                };
                let fallback_midi = if is_pad_kit { free_midi } else { 60 };
                let resolved_fallback = cadence_core::sampler::parse_filename_midi(&sample.name)
                    .or_else(|| cadence_core::sampler::parse_filename_midi(&path))
                    .unwrap_or(fallback_midi);
                zone.apply_sample_metadata(
                    Some(1),
                    path.clone(),
                    &sample.meta,
                    sample.frames,
                    resolved_fallback,
                );
                zone.validate()?;
                created_idx = inst.rack.zones.len();
                created_label = format!("{inst_name} · {zname}");
                self.commit(
                    vec![Edit::AddRackZone {
                        instrument: id,
                        zone,
                        index: None,
                    }],
                    format!("Added sample zone '{zname}' → {inst_name}"),
                )
            })();
            match result {
                Ok(mut out) => {
                    self.sync_loaded_samples();
                    out.samples_changed = true;
                    let taken = self.modal.take_browser().context("Browser state missing")?;
                    self.input.mode = Mode::Normal;
                    self.restore_navigation(taken.state.origin);
                    self.rack_ui.zone_cursor = created_idx;
                    self.rack_ui.panel_slot = 0;
                    self.rack_ui.patchbay_cable_id = None;
                    if let Some(r) = self.instrument(id).map(|i| i.rack.clone()) {
                        self.clamp_rack_state(&r);
                    }
                    let count = self.control_descriptors().len();
                    self.control_cursor = self.control_cursor.min(count.saturating_sub(1));
                    if let Some(loader) = &self.assets.browser_loader {
                        loader.active.store(0, Ordering::Release);
                    }
                    out.stop_audition = true;
                    self.status = format!("Added zone {} ({created_label})", entry.name);
                    return Ok(out);
                }
                Err(error) => {
                    if let Some(b) = self.modal.browser_mut() {
                        b.error = Some(format!("{error:#}"));
                    }
                    return Ok(Outcome::default());
                }
            }
        }
        let taken = self.modal.take_browser().context("Browser state missing")?;
        if let Some(loader) = &self.assets.browser_loader {
            loader.active.store(0, Ordering::Release);
        }
        self.input.mode = Mode::Workflow;
        let mut out = if matches!(target, BrowserTarget::Workflow) {
            let SuspendedWorkflow { mut draft, origin } =
                *taken.return_workflow.context("Instrument draft missing")?;
            let next = draft.set_sample(self.doc.project(), path, entry.name);
            self.modal = ActiveModal::Workflow {
                draft: Box::new(draft),
                origin,
            };
            self.apply_workflow_outcome(next)?
        } else {
            let draft = WorkflowDraft::instrument_with_sample(
                self.doc.project(),
                self.workflow_context(),
                path,
                entry.name,
                &self.assets.presets,
            );
            self.modal = ActiveModal::Workflow {
                draft: Box::new(draft),
                origin: Some(taken.state.origin),
            };
            Outcome::default()
        };
        out.stop_audition = true;
        Ok(out)
    }
    pub(super) fn handle_browser(&mut self, key: KeyEvent) -> Result<Outcome> {
        if key.code == KeyCode::Esc {
            if self.modal.browser().is_some_and(|b| b.searching) {
                if let Some(b) = self.modal.browser_mut() {
                    b.searching = false;
                }
                return self.browser_query(String::new());
            }
            if !self.assets.library.query.is_empty() {
                return self.browser_query(String::new());
            }
            return Ok(self.close_browser());
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(self.close_browser());
        }
        if key.kind == KeyEventKind::Repeat
            && matches!(
                key.code,
                KeyCode::Enter | KeyCode::Char('A' | 'r' | 'R' | ' ')
            )
        {
            return Ok(Outcome::default());
        }
        let searching = self.modal.browser().is_some_and(|b| b.searching);
        if searching {
            match key.code {
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some(b) = self.modal.browser_mut() {
                        b.searching = false;
                    }
                }
                KeyCode::Up => return self.browser_move(-1),
                KeyCode::Down => return self.browser_move(1),
                KeyCode::Backspace => {
                    let mut q = self.assets.library.query.clone();
                    q.pop();
                    return self.browser_query(q);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return self.browser_query(String::new())
                }
                KeyCode::Char(c)
                    if !c.is_control()
                        && !key
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                        && self.assets.library.query.len() < 256 =>
                {
                    let mut q = self.assets.library.query.clone();
                    q.push(c);
                    return self.browser_query(q);
                }
                _ => {}
            }
            return Ok(Outcome::default());
        }
        match key.code {
            KeyCode::Char('/') => {
                if let Some(b) = self.modal.browser_mut() {
                    b.searching = true;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => return self.browser_move(1),
            KeyCode::Char('k') | KeyCode::Up => return self.browser_move(-1),
            KeyCode::PageDown => return self.browser_move(16),
            KeyCode::PageUp => return self.browser_move(-16),
            KeyCode::Home => {} // handled globally before browser dispatch
            KeyCode::Char('G') | KeyCode::End => {
                self.assets.library.cursor = self.assets.library.filtered_count().saturating_sub(1);
                self.refresh_browser_selection(false)?;
                return Ok(Outcome {
                    stop_audition: true,
                    ..Outcome::default()
                });
            }
            KeyCode::Char('0') => {
                self.assets.library.cursor = 0;
                self.refresh_browser_selection(false)?;
                return Ok(Outcome {
                    stop_audition: true,
                    ..Outcome::default()
                });
            }
            KeyCode::Char(' ') => {
                return self.apply(Action::Play {
                    playing: !self.transport.playing,
                });
            }
            KeyCode::Enter => return self.accept_browser_selection(),
            KeyCode::Char('A') => {
                let Some(state) = self.modal.browser_mut() else {
                    return Ok(Outcome::default());
                };
                if let Some(sample) = &state.sample {
                    return Ok(Outcome {
                        stop_audition: true,
                        preview_sample: Some(cadence_core::sampler::SamplePreview::full(
                            Arc::clone(sample),
                        )),
                        ..Outcome::default()
                    });
                }
                if state.loading {
                    state.audition_when_ready = true;
                } else {
                    state.error = Some("Choose a decodable sample to audition".into());
                }
            }
            KeyCode::Char('r') => self.refresh_browser_selection(true)?,
            KeyCode::Char('R') => {
                let selected = self
                    .assets
                    .library
                    .selected_entry()
                    .map(|entry| entry.path.clone());
                self.assets.library.rescan();
                let position = selected.and_then(|path| {
                    self.assets
                        .library
                        .visible_entries()
                        .find(|(_, entry)| entry.path == path)
                        .map(|(index, _)| index)
                });
                self.assets.library.cursor = position.unwrap_or(0);
                self.refresh_browser_selection(true)?;
            }
            _ => {}
        }
        Ok(Outcome::default())
    }
    pub(super) fn clear_browser(&mut self) {
        if let Some(loader) = &self.assets.browser_loader {
            loader.active.store(0, Ordering::Release);
        }
        if matches!(self.modal, ActiveModal::Browser { .. }) {
            self.modal = ActiveModal::None;
        }
    }
    pub(super) fn stop_pending_browser_audition(&mut self) {
        if let Some(state) = self.modal.browser_mut() {
            state.audition_when_ready = false;
        }
    }
}

#[cfg(test)]
mod tests;
