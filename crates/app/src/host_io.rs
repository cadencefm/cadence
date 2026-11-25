//! Narrow host I/O abstraction owned by `cadence-app`.
//!
//! All filesystem, environment, config, preset, bundle, and sample-file I/O
//! inside `cadence-app` routes through [`HostIo`], keeping the controller free
//! of direct OS filesystem calls and 100% testable in memory via
//! [`MemoryHostIo`].
use anyhow::{bail, ensure, Context, Result};
use cadence_core::bundle::{self, BundleReport};
use cadence_core::config::UserConfig;
use cadence_core::filter_patterns::FilterGrooveCatalog;
use cadence_core::material::Project;
use cadence_core::mixer::LiveFlags;
use cadence_core::presets::PresetCatalog;
use cadence_core::sampler::{SampleData, SampleLibrary};
use cadence_core::store;
use cadence_core::take::{self, MemoryJournalSink, TakeJournalSink};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

pub use cadence_core::store::DirEntryInfo;

/// Narrow host I/O trait injected into [`crate::App`].
pub trait HostIo: Send + Sync {
    fn current_dir(&self) -> Result<PathBuf>;
    fn home_dir(&self) -> Option<PathBuf>;
    fn exists(&self, path: &Path) -> bool;
    fn is_dir(&self, path: &Path) -> bool;
    fn is_file(&self, path: &Path) -> bool;
    fn file_len(&self, path: &Path) -> Result<u64>;
    fn canonicalize(&self, path: &Path) -> Result<PathBuf>;
    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>>;
    fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>>;
    fn write_atomic(&self, path: &Path, max_bytes: u64, payload: &[u8]) -> Result<()>;
    fn create_dir_all(&self, path: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;

    fn config_path(&self) -> PathBuf;
    fn save_user_config(&self, config: &UserConfig) -> Result<PathBuf>;
    fn discover_presets(
        &self,
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> PresetCatalog;
    fn discover_samples(
        &self,
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> SampleLibrary;
    fn load_filter_grooves(&self, roots: &[PathBuf]) -> FilterGrooveCatalog;
    fn save_portable_bundle(
        &self,
        project: &Project,
        flags: &LiveFlags,
        asset_base: &Path,
        library: &SampleLibrary,
        resolved: &HashMap<u64, PathBuf>,
        target_dir: &Path,
    ) -> Result<BundleReport>;
    fn next_session_take_path(&self, takes_dir: &Path) -> Result<PathBuf>;
    fn open_take_sink(&self, path: &Path) -> Result<Box<dyn TakeJournalSink>>;
    fn prune_takes_dir(&self, takes_dir: &Path, max_keep: usize) -> Result<usize>;
    fn load_sample_file(&self, path: &Path, label: Option<String>) -> Result<Arc<SampleData>>;
}

/// Default host I/O adapter delegating to `cadence-core`'s bounded OS persistence helpers.
#[derive(Clone, Copy, Debug, Default)]
pub struct CoreHostIo;

impl HostIo for CoreHostIo {
    fn current_dir(&self) -> Result<PathBuf> {
        store::current_dir()
    }

    fn home_dir(&self) -> Option<PathBuf> {
        store::home_dir()
    }

    fn exists(&self, path: &Path) -> bool {
        store::path_exists(path)
    }

    fn is_dir(&self, path: &Path) -> bool {
        store::is_dir(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        store::is_file(path)
    }

    fn file_len(&self, path: &Path) -> Result<u64> {
        store::file_len(path)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        store::canonicalize(path)
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>> {
        store::read_dir_entries(path)
    }

    fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
        store::read_bounded(path, max_bytes)
    }

    fn write_atomic(&self, path: &Path, max_bytes: u64, payload: &[u8]) -> Result<()> {
        store::write_atomic_bytes(path, max_bytes, payload)
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        store::create_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        store::remove_file(path)
    }

    fn config_path(&self) -> PathBuf {
        UserConfig::default_path()
    }

    fn save_user_config(&self, config: &UserConfig) -> Result<PathBuf> {
        config.save_default()
    }

    fn discover_presets(
        &self,
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> PresetCatalog {
        PresetCatalog::discover(extra_dirs, project_path, config)
    }

    fn discover_samples(
        &self,
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> SampleLibrary {
        SampleLibrary::discover_with_config(extra_dirs, project_path, config)
    }

    fn load_filter_grooves(&self, roots: &[PathBuf]) -> FilterGrooveCatalog {
        FilterGrooveCatalog::load_from_roots(roots)
    }

    fn save_portable_bundle(
        &self,
        project: &Project,
        flags: &LiveFlags,
        asset_base: &Path,
        library: &SampleLibrary,
        resolved: &HashMap<u64, PathBuf>,
        target_dir: &Path,
    ) -> Result<BundleReport> {
        bundle::save_portable_bundle(project, flags, asset_base, library, resolved, target_dir)
    }

    fn next_session_take_path(&self, takes_dir: &Path) -> Result<PathBuf> {
        store::next_session_take_path(takes_dir)
    }

    fn open_take_sink(&self, path: &Path) -> Result<Box<dyn TakeJournalSink>> {
        take::open_disk_journal_sink(path.to_path_buf())
    }

    fn prune_takes_dir(&self, takes_dir: &Path, max_keep: usize) -> Result<usize> {
        store::prune_takes_dir(takes_dir, max_keep)
    }

    fn load_sample_file(&self, path: &Path, label: Option<String>) -> Result<Arc<SampleData>> {
        cadence_core::sampler::load_sample_file(path, label)
    }
}

#[derive(Default)]
struct MemoryFs {
    cwd: PathBuf,
    home_dir: Option<PathBuf>,
    dirs: BTreeSet<PathBuf>,
    files: BTreeMap<PathBuf, Vec<u8>>,
    samples: HashMap<PathBuf, Arc<SampleData>>,
    saved_config: Option<UserConfig>,
    presets: PresetCatalog,
    filter_grooves: FilterGrooveCatalog,
    take_seq: u64,
}

/// 100% pure in-memory [`HostIo`] implementation for deterministic, disk-free tests.
pub struct MemoryHostIo {
    state: Arc<RwLock<MemoryFs>>,
}

impl Default for MemoryHostIo {
    fn default() -> Self {
        Self::new("/virtual")
    }
}

impl MemoryHostIo {
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        let cwd = normalize_path(Path::new("/"), &cwd.into());
        let mut dirs = BTreeSet::new();
        insert_ancestors(&mut dirs, &cwd);
        Self {
            state: Arc::new(RwLock::new(MemoryFs {
                cwd,
                home_dir: None,
                dirs,
                files: BTreeMap::new(),
                samples: HashMap::new(),
                saved_config: None,
                presets: PresetCatalog::default(),
                filter_grooves: FilterGrooveCatalog::default(),
                take_seq: 0,
            })),
        }
    }

    pub fn insert_file(&self, path: impl AsRef<Path>, bytes: impl Into<Vec<u8>>) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path.as_ref());
        if let Some(parent) = norm.parent() {
            insert_ancestors(&mut fs.dirs, parent);
        }
        fs.samples.remove(&norm);
        fs.files.insert(norm, bytes.into());
    }

    pub fn create_dir(&self, path: impl AsRef<Path>) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path.as_ref());
        insert_ancestors(&mut fs.dirs, &norm);
    }

    pub fn set_home_dir(&self, path: impl AsRef<Path>) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path.as_ref());
        insert_ancestors(&mut fs.dirs, &norm);
        fs.home_dir = Some(norm);
    }

    pub fn read_file(&self, path: impl AsRef<Path>) -> Option<Vec<u8>> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path.as_ref());
        fs.files.get(&norm).cloned()
    }

    pub fn file_paths(&self) -> Vec<PathBuf> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        fs.files.keys().cloned().collect()
    }

    pub fn saved_config(&self) -> Option<UserConfig> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        fs.saved_config.clone()
    }

    pub fn set_presets(&self, presets: PresetCatalog) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        fs.presets = presets;
    }

    pub fn set_filter_grooves(&self, filter_grooves: FilterGrooveCatalog) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        fs.filter_grooves = filter_grooves;
    }

    pub fn insert_sample(&self, path: impl AsRef<Path>, sample: Arc<SampleData>) {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path.as_ref());
        if let Some(parent) = norm.parent() {
            insert_ancestors(&mut fs.dirs, parent);
        }
        fs.files.entry(norm.clone()).or_default();
        fs.samples.insert(norm, sample);
    }
}

impl HostIo for MemoryHostIo {
    fn current_dir(&self) -> Result<PathBuf> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        Ok(fs.cwd.clone())
    }

    fn home_dir(&self) -> Option<PathBuf> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        fs.home_dir.clone()
    }

    fn exists(&self, path: &Path) -> bool {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        fs.files.contains_key(&norm) || fs.dirs.contains(&norm)
    }

    fn is_dir(&self, path: &Path) -> bool {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        fs.dirs.contains(&norm)
    }

    fn is_file(&self, path: &Path) -> bool {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        fs.files.contains_key(&norm)
    }

    fn file_len(&self, path: &Path) -> Result<u64> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        let bytes = fs
            .files
            .get(&norm)
            .with_context(|| format!("Missing virtual file {}", path.display()))?;
        Ok(bytes.len() as u64)
    }

    fn canonicalize(&self, path: &Path) -> Result<PathBuf> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        Ok(normalize_path(&fs.cwd, path))
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<DirEntryInfo>> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let dir = normalize_path(&fs.cwd, path);
        ensure!(
            fs.dirs.contains(&dir),
            "Virtual directory does not exist: {}",
            path.display()
        );
        let mut entries = Vec::new();
        for d in &fs.dirs {
            if d.parent() == Some(dir.as_path()) && d != &dir {
                if let Some(name) = d.file_name().and_then(|n| n.to_str()) {
                    entries.push(DirEntryInfo {
                        path: d.clone(),
                        file_name: name.to_string(),
                        is_dir: true,
                        is_file: false,
                    });
                }
            }
        }
        for f in fs.files.keys() {
            if f.parent() == Some(dir.as_path()) {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    entries.push(DirEntryInfo {
                        path: f.clone(),
                        file_name: name.to_string(),
                        is_dir: false,
                        is_file: true,
                    });
                }
            }
        }
        Ok(entries)
    }

    fn read_bounded(&self, path: &Path, max_bytes: u64) -> Result<Vec<u8>> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        let bytes = fs
            .files
            .get(&norm)
            .with_context(|| format!("Opening {}", path.display()))?;
        ensure!(
            bytes.len() as u64 <= max_bytes,
            "{} exceeds {max_bytes} byte limit",
            path.display()
        );
        Ok(bytes.clone())
    }

    fn write_atomic(&self, path: &Path, max_bytes: u64, payload: &[u8]) -> Result<()> {
        ensure!(
            payload.len() as u64 <= max_bytes,
            "Refusing to write {} ({} bytes exceeds {max_bytes} byte limit)",
            path.display(),
            payload.len()
        );
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        if let Some(parent) = norm.parent() {
            insert_ancestors(&mut fs.dirs, parent);
        }
        fs.samples.remove(&norm);
        fs.files.insert(norm, payload.to_vec());
        Ok(())
    }

    fn create_dir_all(&self, path: &Path) -> Result<()> {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        insert_ancestors(&mut fs.dirs, &norm);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        fs.files.remove(&norm);
        Ok(())
    }

    fn config_path(&self) -> PathBuf {
        let fs = self.state.read().expect("MemoryHostIo lock");
        fs.cwd.join("config.json")
    }

    fn save_user_config(&self, config: &UserConfig) -> Result<PathBuf> {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let path = fs.cwd.join("config.json");
        let mut bytes = serde_json::to_vec_pretty(config)?;
        bytes.push(b'\n');
        fs.saved_config = Some(config.clone());
        fs.files.insert(path.clone(), bytes);
        Ok(path)
    }

    fn discover_presets(
        &self,
        _extra_dirs: &[PathBuf],
        _project_path: Option<&Path>,
        _config: &UserConfig,
    ) -> PresetCatalog {
        let fs = self.state.read().expect("MemoryHostIo lock");
        fs.presets.clone()
    }

    fn discover_samples(
        &self,
        extra_dirs: &[PathBuf],
        project_path: Option<&Path>,
        config: &UserConfig,
    ) -> SampleLibrary {
        let mut roots: Vec<PathBuf> = extra_dirs.to_vec();
        for dir in &config.sample_dirs {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                roots.push(PathBuf::from(trimmed));
            }
        }
        SampleLibrary::from_roots(&roots, project_path)
    }

    fn load_filter_grooves(&self, roots: &[PathBuf]) -> FilterGrooveCatalog {
        let fs = self.state.read().expect("MemoryHostIo lock");
        for root in roots {
            let candidate = normalize_path(&fs.cwd, &root.join("filter_grooves.json"));
            if let Some(bytes) = fs.files.get(&candidate) {
                if let Ok(cat) = FilterGrooveCatalog::parse_bytes(bytes) {
                    return cat.with_source_path(candidate);
                }
            }
        }
        fs.filter_grooves.clone()
    }

    fn save_portable_bundle(
        &self,
        project: &Project,
        flags: &LiveFlags,
        _asset_base: &Path,
        _library: &SampleLibrary,
        _resolved: &HashMap<u64, PathBuf>,
        target_dir: &Path,
    ) -> Result<BundleReport> {
        project.validate()?;
        let mut buf = Vec::new();
        cadence_core::material::write_document_json(&mut buf, project, flags)?;
        let dir = self.canonicalize(target_dir)?;
        self.create_dir_all(&dir)?;
        let samples_dir = dir.join("samples");
        self.create_dir_all(&samples_dir)?;
        let project_path = dir.join("project.json");
        self.write_atomic(&project_path, crate::MAX_PROJECT_BYTES, &buf)?;
        Ok(BundleReport {
            project_path,
            samples_dir,
            samples_copied: 0,
            missing_samples: Vec::new(),
        })
    }

    fn next_session_take_path(&self, takes_dir: &Path) -> Result<PathBuf> {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let dir = normalize_path(&fs.cwd, takes_dir);
        insert_ancestors(&mut fs.dirs, &dir);
        let seq = fs.take_seq;
        fs.take_seq += 1;
        Ok(dir.join(format!("take-{seq:06}.take.jsonl")))
    }

    fn open_take_sink(&self, path: &Path) -> Result<Box<dyn TakeJournalSink>> {
        let state = Arc::clone(&self.state);
        let target = {
            let fs = self.state.read().expect("MemoryHostIo lock");
            normalize_path(&fs.cwd, path)
        };
        Ok(Box::new(MemoryJournalSink::with_on_seal(move |bytes| {
            let mut fs = state.write().expect("MemoryHostIo lock");
            if let Some(parent) = target.parent() {
                insert_ancestors(&mut fs.dirs, parent);
            }
            fs.files.insert(target, bytes);
            Ok(())
        })))
    }

    fn prune_takes_dir(&self, takes_dir: &Path, max_keep: usize) -> Result<usize> {
        let mut fs = self.state.write().expect("MemoryHostIo lock");
        let dir = normalize_path(&fs.cwd, takes_dir);
        let mut take_files: Vec<PathBuf> = fs
            .files
            .keys()
            .filter(|p| {
                p.parent() == Some(dir.as_path())
                    && p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(".take.jsonl"))
            })
            .cloned()
            .collect();
        if take_files.len() <= max_keep {
            return Ok(0);
        }
        take_files.sort();
        let remove_count = take_files.len() - max_keep;
        for old in take_files.into_iter().take(remove_count) {
            fs.files.remove(&old);
        }
        Ok(remove_count)
    }

    fn load_sample_file(&self, path: &Path, label: Option<String>) -> Result<Arc<SampleData>> {
        let fs = self.state.read().expect("MemoryHostIo lock");
        let norm = normalize_path(&fs.cwd, path);
        if let Some(sample) = fs.samples.get(&norm) {
            return Ok(Arc::clone(sample));
        }
        if let Some(bytes) = fs.files.get(&norm) {
            let name = norm
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Sample".into());
            let path_label = label.unwrap_or_else(|| norm.to_string_lossy().into_owned());
            return cadence_core::sampler::decode_sample_bytes(bytes, name, path_label, norm);
        }
        bail!("Virtual sample not found: {}", path.display())
    }
}

fn normalize_path(cwd: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let mut out = PathBuf::new();
    for comp in joined.components() {
        match comp {
            Component::Prefix(p) => out.push(p.as_os_str()),
            Component::RootDir => out.push(comp.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        out
    }
}

fn insert_ancestors(dirs: &mut BTreeSet<PathBuf>, dir: &Path) {
    for anc in dir.ancestors() {
        if !anc.as_os_str().is_empty() {
            dirs.insert(anc.to_path_buf());
        }
    }
}
