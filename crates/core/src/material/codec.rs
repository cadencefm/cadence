//! Flat JSON document serialization and deserialization for `Project` + `LiveFlags`.

use super::{LiveFlags, Project};
use anyhow::{Context, Result};
use serde::Serialize;

/// Serialize an authored `Project` and its live mute/solo `LiveFlags` into one
/// flat JSON document in struct field order, omitting `flags` when empty.
pub fn write_document_json(buf: &mut Vec<u8>, project: &Project, flags: &LiveFlags) -> Result<()> {
    let mut pruned = flags.clone();
    project.prune_flags(&mut pruned);
    #[derive(Serialize)]
    struct DocumentOut<'a> {
        #[serde(flatten)]
        project: &'a Project,
        #[serde(skip_serializing_if = "LiveFlags::is_empty")]
        flags: &'a LiveFlags,
    }
    serde_json::to_writer_pretty(
        &mut *buf,
        &DocumentOut {
            project,
            flags: &pruned,
        },
    )?;
    buf.push(b'\n');
    Ok(())
}

/// Deserialize a flat JSON document into `(Project, LiveFlags)`, rejecting any
/// unknown fields or invalid flag references.
pub fn parse_document_json(bytes: &[u8]) -> Result<(Project, LiveFlags)> {
    let mut map: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(bytes).context("Invalid retained-material project")?;
    let flags: LiveFlags = match map.remove("flags") {
        Some(raw) => serde_json::from_value(raw).context("Invalid live flags")?,
        None => LiveFlags::default(),
    };
    let project: Project = serde_json::from_value(serde_json::Value::Object(map))
        .context("Invalid retained-material project")?;
    project.validate()?;
    project.validate_flags(&flags)?;
    Ok((project, flags))
}
