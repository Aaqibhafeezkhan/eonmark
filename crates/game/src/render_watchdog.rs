//! Render pipeline watchdog: logs an `error!` for every render or compute
//! pipeline that is still not ready [`PIPELINE_DEADLINE`] after it was first
//! seen waiting.
//!
//! Bevy logs a shader that fails to compile, but a shader whose `#import`
//! names a module that does not exist is retried silently every frame
//! (`PipelineCache::process_pipeline` resets `ShaderImportNotYetAvailable`
//! to `Queued`, `bevy_render` 0.19.1 `pipeline_cache.rs`), so whatever uses
//! it is never drawn and nothing says so. This turns that, and any other
//! pipeline that never becomes ready, into an ERROR line, which
//! `scripts/render_smoke.sh` fails on. It runs in every windowed build;
//! headless runs have no render app.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::render::render_resource::{CachedPipelineState, PipelineCache, PipelineDescriptor};
use bevy::render::{Render, RenderApp};

/// How long a pipeline may wait before it is reported. Generous: lavapipe
/// on a CI runner compiles a PBR pipeline in a few seconds.
pub const PIPELINE_DEADLINE: Duration = Duration::from_secs(20);

/// Adds the watchdog to the render app.
pub struct RenderWatchdogPlugin;

impl Plugin for RenderWatchdogPlugin {
    fn build(&self, app: &mut App) {
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, watch_pipelines);
        }
    }
}

/// When each waiting pipeline (by index in the cache) was first seen, and
/// which were reported.
#[derive(Debug, Default)]
pub struct Watchdog {
    since: BTreeMap<usize, Instant>,
    reported: BTreeSet<usize>,
}

impl Watchdog {
    /// Record pipeline `index` as ready or waiting at `now`; `true` exactly
    /// once, when it has waited [`PIPELINE_DEADLINE`]. A pipeline that
    /// becomes ready starts afresh if it ever waits again.
    pub fn observe(&mut self, index: usize, ready: bool, now: Instant) -> bool {
        if ready {
            self.since.remove(&index);
            return false;
        }
        let since = *self.since.entry(index).or_insert(now);
        now.duration_since(since) >= PIPELINE_DEADLINE && self.reported.insert(index)
    }
}

fn watch_pipelines(cache: Res<PipelineCache>, mut watchdog: Local<Watchdog>) {
    let now = Instant::now();
    for (index, pipeline) in cache.pipelines().enumerate() {
        let ready = matches!(pipeline.state, CachedPipelineState::Ok(_));
        if !watchdog.observe(index, ready, now) {
            continue;
        }
        let state = match &pipeline.state {
            CachedPipelineState::Queued => "queued".to_string(),
            CachedPipelineState::Creating(_) => "creating".to_string(),
            CachedPipelineState::Err(e) => format!("error: {e}"),
            CachedPipelineState::Ok(_) => "ready".to_string(),
        };
        let (label, shaders) = match &pipeline.descriptor {
            PipelineDescriptor::RenderPipelineDescriptor(d) => (
                d.label.clone(),
                [
                    Some(&d.vertex.shader),
                    d.fragment.as_ref().map(|f| &f.shader),
                ]
                .into_iter()
                .flatten()
                .map(|s| {
                    s.path()
                        .map_or_else(|| format!("{s:?}"), ToString::to_string)
                })
                .collect::<Vec<_>>(),
            ),
            PipelineDescriptor::ComputePipelineDescriptor(d) => (
                d.label.clone(),
                vec![
                    d.shader
                        .path()
                        .map_or_else(|| format!("{:?}", d.shader), ToString::to_string),
                ],
            ),
        };
        error!(
            "render pipeline {} ({}) not ready after {} s, {state}; shaders: {}",
            index,
            label.as_deref().unwrap_or("unlabelled"),
            PIPELINE_DEADLINE.as_secs(),
            shaders.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pipeline_is_reported_once_after_the_deadline_and_ready_ones_never() {
        let t0 = Instant::now();
        let mut w = Watchdog::default();
        assert!(!w.observe(0, false, t0));
        assert!(!w.observe(1, true, t0));
        let late = t0 + PIPELINE_DEADLINE;
        assert!(!w.observe(0, false, late - Duration::from_millis(1)));
        assert!(w.observe(0, false, late), "reported at the deadline");
        assert!(!w.observe(0, false, late + Duration::from_secs(5)), "once");
        assert!(!w.observe(1, true, late + Duration::from_secs(5)));
        // Waiting again after being ready restarts the clock.
        assert!(!w.observe(2, false, t0));
        assert!(!w.observe(2, true, t0 + Duration::from_secs(1)));
        assert!(!w.observe(2, false, late));
    }
}
