//! Key scripts: executable construction examples and headless replay harnesses.
use super::replay_view_keys;
use anyhow::{Context, Result};
use cadence_app::App;
use std::path::Path;

/// Maximum key script file size (4 MiB) to bound memory before parsing.
const MAX_KEY_SCRIPT_BYTES: u64 = 4 * 1024 * 1024;

/// Replay a JSONL key script through the pure `cadence-app` controller.
pub fn replay_key_script(app: &mut App, path: &Path) -> Result<serde_json::Value> {
    #[derive(serde::Deserialize)]
    struct KeyStep {
        #[serde(default)]
        label: String,
        keys: String,
        #[serde(default)]
        expect: std::collections::BTreeMap<String, serde_json::Value>,
        /// Headless live clock: advance the transport, e.g. "2 bars".
        #[serde(default)]
        advance: Option<String>,
    }
    let bytes = cadence_core::store::read_bounded(path, MAX_KEY_SCRIPT_BYTES)
        .with_context(|| format!("Reading key script {}", path.display()))?;
    let text = std::str::from_utf8(&bytes)
        .with_context(|| format!("Key script {} is not valid UTF-8", path.display()))?;
    let mut count = 0;
    for (line_no, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let step: KeyStep = serde_json::from_str(line)
            .with_context(|| format!("Key script line {}", line_no + 1))?;
        replay_view_keys(app, &step.keys)
            .with_context(|| format!("Key script line {} ({})", line_no + 1, step.label))?;
        if let Some(advance) = &step.advance {
            let steps = cadence_app::parse_duration(advance)
                .with_context(|| format!("Key script line {} ({})", line_no + 1, step.label))?;
            app.advance(steps);
        }
        let live = app.live_state_json();
        let state = serde_json::json!({
            "title":app.project().title,"patterns":app.project().patterns.len(),
            "instruments":app.project().instruments.len(),"parts":app.project().parts.len(),
            "lanes":app.project().lanes.len(),"placements":app.project().lanes.iter().map(|l|l.placements.len()).sum::<usize>(),
            "channels":app.project().mixer.channels.len(),"sections":app.project().sections.len(),
            "ducks":app.project().ducks.len(),"revision":app.revision(),"dirty":app.dirty(),"ready":app.ready(),
            "mode":app.mode_name(),"draft_open":app.is_authoring(),"value_entry_open":app.is_value_input(),
            "step":live["next_step"],"playing":live["playing"],
            "active":live["active"].as_array().map_or(0, Vec::len),
            "pending":live["pending"].as_array().map_or(0, Vec::len),
            "lane_parts":app.project().lanes.iter().map(|l|l.parts.len()).sum::<usize>(),
            "workspace":app.workspace(),
        });
        let project = serde_json::to_value(app.project())?;
        for (key, value) in step.expect {
            let actual = if key.starts_with('/') {
                project.pointer(&key)
            } else {
                state.get(&key)
            };
            anyhow::ensure!(
                actual == Some(&value),
                "Key script line {} ({}): expected {}={}, got {:?}",
                line_no + 1,
                step.label,
                key,
                value,
                actual
            );
        }
        count += 1;
    }
    Ok(
        serde_json::json!({"ok":true,"key_steps":count,"revision":app.revision(),"title":app.project().title}),
    )
}
