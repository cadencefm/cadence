//! Consolidated headless Metal/Vulkan GPU integration test harness for `cadence-gui`.
//!
//! All GPU visual regression suites share a single compiled test binary and a lock-free-on-render
//! checkout pool of `HeadlessGpu` instances (resized in place), verifying balanced `DisplayList`
//! clip stacks, zero missing embedded `JetBrains Mono` glyphs, and non-blank rasterized pixels on
//! every frame.
//!
//! Set `CADENCE_DUMP_GPU_DIR=/path/to/dir` to export PNG screenshots for visual inspection.

#[path = "gpu/arrange_and_graph.rs"]
mod arrange_and_graph;
#[path = "gpu/common.rs"]
mod common;
#[path = "gpu/fx.rs"]
mod fx;
#[path = "gpu/mixer_and_metering.rs"]
mod mixer_and_metering;
#[path = "gpu/patterns_and_overlays.rs"]
mod patterns_and_overlays;
#[path = "gpu/rack_and_zones.rs"]
mod rack_and_zones;
#[path = "gpu/sample_deck.rs"]
mod sample_deck;
#[path = "gpu/shaders.rs"]
mod shaders;
#[path = "gpu/staging_macros_devices.rs"]
mod staging_macros_devices;
