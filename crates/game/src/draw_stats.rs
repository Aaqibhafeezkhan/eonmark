//! The draw-call counter (art pass, `docs/design/ui.md`, "Draw calls"):
//! how many draw calls the render passes issue each frame, rebuilt from
//! Bevy 0.19.1's public render-phase data, since wgpu counts nothing and
//! the batch sets themselves are `pub(crate)`.
//!
//! [`count_draws`] runs in the render world every frame, in
//! `RenderSystems::PrepareBindGroups` after `bevy::ui_render::prepare_uinodes`
//! (the UI batch ranges exist by then; the indirect buffers were written in
//! `PrepareResourcesFlush` and are not cleared until the next
//! `PrepareViews`), and adds up:
//!
//! 1. **indirect**: one draw per allocated indirect-parameters entry, over
//!    every phase's [`IndirectParametersBuffers`] (indexed and non-indexed).
//!    That covers every mesh bin, unbatchable mesh and indirect sorted mesh
//!    of every view that draws indirectly. On Metal wgpu issues a
//!    multi-draw as one draw per entry, and `multi_draw_*_count` on Vulkan
//!    is bounded by the same count;
//! 2. **direct**: for every view with `NoIndirectDrawing` (all views in
//!    the `PreprocessingOnly` mode, the shadow cascades whenever the mode is
//!    not `Culling`), which allocate no indirect entries, the bins of its
//!    `Opaque3d`, `AlphaMask3d` and `Shadow` phases: one draw per
//!    multidrawable batch set and per batchable bin, one per unbatchable
//!    entity and per non-mesh item ([`binned_direct_draws`]);
//! 3. **sorted**: the `Transparent3d`, `Transmissive3d` and `TransparentUi`
//!    items of every view, walked exactly as `SortedRenderPhase::render_range`
//!    walks them: one draw per item with a non-empty batch range, which
//!    then skips the rest of its batch, except items whose extra index is
//!    an indirect-parameters index, which step 1 counted
//!    ([`sorted_draws`]). Gizmos (one item per retained gizmo entity) and
//!    UI batches are counted here.
//!
//! Not counted: the full-screen passes (tonemapping, upscaling, the MSAA
//! resolve; about 2 a frame, drawn on raw `wgpu` passes) and egui (dev
//! builds). The mode comes from `GpuPreprocessingSupport`; `None` (GL
//! only, never a target of ours) prints `mode=none`, which the gates treat
//! as a failure rather than trust a miscount.
//!
//! The render world stores each frame's numbers itself in [`DrawStats`]
//! (atomics shared with the main world through one `Arc`), so pipelined
//! rendering cannot split or merge frames; the main world only reads. The
//! first [`SKIP_FRAMES`] frames (pipeline compiles) are left out of the
//! run's statistics. Each counted frame is also logged at `debug` level
//! (`draw_stats: frame <n> total=<t> ...`, target `game::draw_stats`) for
//! `scripts/draw_trace_xcheck.sh`, which compares it with Bevy's own
//! `detailed_trace` draw lines.
//!
//! Read-out: the dev panel's `draw calls` row (`dev_tools.rs`) and, on
//! exit, `frame_stats: draws frames=<n> mean=<f> p95=<n> max=<n>
//! indirect_max=<n> direct_max=<n> sorted_max=<n> mode=<m>` (plus
//! `fight_max=<n>` when the fight window is on) and `frame_stats: triangles
//! main_mean=<n> main_max=<n> scene=<n>` (the triangles of the meshes the
//! RTS camera sees, and of every mesh entity, from a per-mesh cache).
//! Enabled by `--draw-stats` and in every `dev` build. The test-only knob
//! [`WGPU_DISABLE_ENV`] (`EONMARK_WGPU_DISABLE=indirect_first_instance`)
//! turns wgpu features off, which forces the `PreprocessingOnly` mode for
//! the cross-check.

use std::any::TypeId;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};

use bevy::app::AppExit;
use bevy::camera::visibility::VisibleEntities;
use bevy::core_pipeline::core_3d::{AlphaMask3d, Opaque3d, Transparent3d};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::{Shadow, Transmissive3d};
use bevy::prelude::*;
use bevy::render::batching::gpu_preprocessing::{
    GpuPreprocessingMode, GpuPreprocessingSupport, IndirectParametersBuffers,
    UntypedPhaseIndirectParametersBuffers,
};
use bevy::render::render_phase::{
    BinnedPhaseItem, BinnedRenderPhase, PhaseItemExtraIndex, SortedPhaseItem,
    ViewBinnedRenderPhases, ViewSortedRenderPhases,
};
use bevy::render::settings::WgpuFeatures;
use bevy::render::view::{ExtractedView, NoIndirectDrawing, RetainedViewEntity};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui_render::TransparentUi;

use crate::camera::RtsCamera;
use crate::frame_stats::{FightWindow, STDOUT_PREFIX};
use crate::sim_driver::SimHandle;

/// Frames left out of the run's statistics at start-up, while pipelines
/// compile and the first assets arrive.
pub const SKIP_FRAMES: u32 = 30;

/// The draw-call budget the dev panel compares against (ROADMAP M3b and M7:
/// under 500 draw calls).
pub const DRAW_BUDGET: u32 = 500;

/// Largest per-frame total the p95 histogram resolves; larger frames land
/// in its last bucket (the max is exact either way).
pub const HISTOGRAM_MAX: u32 = 4096;

/// Environment variable naming wgpu features to disable, comma-separated,
/// by their `wgpu::Features` names in any case
/// (`indirect_first_instance`): a test knob for the trace cross-check.
pub const WGPU_DISABLE_ENV: &str = "EONMARK_WGPU_DISABLE";

/// One frame's draws by source.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrawCounts {
    /// Indirect-parameters entries (step 1).
    pub indirect: u32,
    /// Direct draws of views without indirect drawing (step 2).
    pub direct: u32,
    /// Sorted-phase draws not already counted as indirect (step 3).
    pub sorted: u32,
}

impl DrawCounts {
    /// All three.
    pub fn total(&self) -> u32 {
        self.indirect
            .saturating_add(self.direct)
            .saturating_add(self.sorted)
    }
}

/// The GPU preprocessing mode the counter ran under, as printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountMode {
    /// No frame counted yet.
    Unknown,
    /// CPU preprocessing (GL only): not counted reliably.
    None,
    /// GPU preprocessing without culling: every view draws directly.
    PreprocessingOnly,
    /// GPU culling: views draw indirectly.
    Culling,
}

impl CountMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => CountMode::None,
            2 => CountMode::PreprocessingOnly,
            3 => CountMode::Culling,
            _ => CountMode::Unknown,
        }
    }

    fn to_u8(self) -> u8 {
        match self {
            CountMode::Unknown => 0,
            CountMode::None => 1,
            CountMode::PreprocessingOnly => 2,
            CountMode::Culling => 3,
        }
    }

    /// The name on the `draws` line.
    pub fn name(self) -> &'static str {
        match self {
            CountMode::Unknown => "unknown",
            CountMode::None => "none",
            CountMode::PreprocessingOnly => "preprocessing_only",
            CountMode::Culling => "culling",
        }
    }
}

impl From<GpuPreprocessingMode> for CountMode {
    fn from(mode: GpuPreprocessingMode) -> Self {
        match mode {
            GpuPreprocessingMode::None => CountMode::None,
            GpuPreprocessingMode::PreprocessingOnly => CountMode::PreprocessingOnly,
            GpuPreprocessingMode::Culling => CountMode::Culling,
        }
    }
}

/// The counts the render world writes and the main world reads.
#[derive(Debug)]
pub struct DrawStatsInner {
    /// Render frames seen, counted or not.
    seen: AtomicU32,
    /// Frames counted (after [`SKIP_FRAMES`]).
    frames: AtomicU32,
    last: [AtomicU32; 3],
    max: [AtomicU32; 3],
    max_total: AtomicU32,
    sum: AtomicU64,
    mode: AtomicU8,
    histogram: Box<[AtomicU32]>,
}

impl Default for DrawStatsInner {
    fn default() -> Self {
        Self {
            seen: AtomicU32::new(0),
            frames: AtomicU32::new(0),
            last: Default::default(),
            max: Default::default(),
            max_total: AtomicU32::new(0),
            sum: AtomicU64::new(0),
            mode: AtomicU8::new(0),
            histogram: (0..=HISTOGRAM_MAX).map(|_| AtomicU32::new(0)).collect(),
        }
    }
}

/// The run's draw statistics, shared by the main and the render world.
#[derive(Resource, Debug, Clone, Default)]
pub struct DrawStats(pub Arc<DrawStatsInner>);

/// What the read-outs show: the last counted frame and the run so far.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DrawSummary {
    /// Frames counted.
    pub frames: u32,
    /// The last counted frame.
    pub last: DrawCounts,
    /// The largest total of any counted frame.
    pub max: u32,
    /// The largest of each source in any counted frame.
    pub max_by_source: DrawCounts,
    /// Mean total per counted frame.
    pub mean: f64,
    /// 95th percentile of the totals (capped at [`HISTOGRAM_MAX`]).
    pub p95: u32,
    /// The mode of the last counted frame.
    pub mode: CountMode,
}

impl DrawStats {
    /// Record one render frame: the first [`SKIP_FRAMES`] only move the
    /// frame number. Returns the frame's number (from 1) and whether it was
    /// counted.
    pub fn record(&self, counts: DrawCounts, mode: CountMode) -> (u32, bool) {
        let s = &self.0;
        let seen = s.seen.fetch_add(1, Ordering::Relaxed).saturating_add(1);
        s.mode.store(mode.to_u8(), Ordering::Relaxed);
        if seen <= SKIP_FRAMES {
            return (seen, false);
        }
        let parts = [counts.indirect, counts.direct, counts.sorted];
        for (i, v) in parts.into_iter().enumerate() {
            s.last[i].store(v, Ordering::Relaxed);
            s.max[i].fetch_max(v, Ordering::Relaxed);
        }
        let total = counts.total();
        s.max_total.fetch_max(total, Ordering::Relaxed);
        s.sum.fetch_add(u64::from(total), Ordering::Relaxed);
        let bucket = usize::try_from(total.min(HISTOGRAM_MAX)).unwrap_or(0);
        s.histogram[bucket].fetch_add(1, Ordering::Relaxed);
        s.frames.fetch_add(1, Ordering::Relaxed);
        (seen, true)
    }

    /// The read-out so far.
    pub fn summary(&self) -> DrawSummary {
        let s = &self.0;
        let frames = s.frames.load(Ordering::Relaxed);
        let load = |a: &[AtomicU32; 3]| DrawCounts {
            indirect: a[0].load(Ordering::Relaxed),
            direct: a[1].load(Ordering::Relaxed),
            sorted: a[2].load(Ordering::Relaxed),
        };
        let mean = if frames == 0 {
            0.0
        } else {
            s.sum.load(Ordering::Relaxed) as f64 / f64::from(frames)
        };
        // The smallest total with at least 95 % of the frames at or below it.
        let rank = u64::from(frames).saturating_mul(95).div_ceil(100);
        let mut seen = 0u64;
        let mut p95 = 0;
        for (total, n) in (0u32..).zip(s.histogram.iter()) {
            seen += u64::from(n.load(Ordering::Relaxed));
            if frames > 0 && seen >= rank {
                p95 = total;
                break;
            }
        }
        DrawSummary {
            frames,
            last: load(&s.last),
            max: s.max_total.load(Ordering::Relaxed),
            max_by_source: load(&s.max),
            mean,
            p95,
            mode: CountMode::from_u8(s.mode.load(Ordering::Relaxed)),
        }
    }
}

impl DrawSummary {
    /// The `draws ...` exit line (without the `frame_stats: ` prefix);
    /// `fight_max` adds the largest total over the fight window's frames.
    pub fn line(&self, fight_max: Option<u32>) -> String {
        let mut line = format!(
            "draws frames={} mean={:.1} p95={} max={} indirect_max={} direct_max={} sorted_max={} mode={}",
            self.frames,
            self.mean,
            self.p95,
            self.max,
            self.max_by_source.indirect,
            self.max_by_source.direct,
            self.max_by_source.sorted,
            self.mode.name()
        );
        if let Some(fight) = fight_max {
            line.push_str(&format!(" fight_max={fight}"));
        }
        line
    }

    /// The dev panel's row: `<last> (max <max>) / 500  ind <a> dir <b>
    /// sorted <c>`.
    pub fn panel_text(&self) -> String {
        format!(
            "{} (max {}) / {DRAW_BUDGET}  ind {} dir {} sorted {}",
            self.last.total(),
            self.max,
            self.last.indirect,
            self.last.direct,
            self.last.sorted
        )
    }

    /// The panel shows the row in amber from [`DRAW_BUDGET`] draws on.
    pub fn over_budget(&self) -> bool {
        self.last.total() >= DRAW_BUDGET
    }
}

/// Step 1: one draw per allocated indirect-parameters entry of one phase.
pub fn phase_indirect_draws(buffers: &UntypedPhaseIndirectParametersBuffers) -> u32 {
    let n = buffers
        .indexed
        .batch_count()
        .saturating_add(buffers.non_indexed.batch_count());
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Step 1 over every phase.
pub fn indirect_draws(buffers: &IndirectParametersBuffers) -> u32 {
    buffers
        .buffers
        .values()
        .map(phase_indirect_draws)
        .fold(0, u32::saturating_add)
}

/// Step 2 for one view's binned phase drawn without indirect parameters:
/// one draw per multidrawable batch set (empty in such a view: `add`
/// turns them into batchable bins outside the `Culling` mode) and per
/// batchable bin, one per unbatchable entity and per non-mesh item.
pub fn binned_direct_draws<BPI: BinnedPhaseItem>(phase: &BinnedRenderPhase<BPI>) -> u32 {
    let unbatchable: usize = phase
        .unbatchable_meshes
        .values()
        .map(|u| u.entities.len())
        .sum();
    let non_mesh: usize = phase
        .non_mesh_items
        .values()
        .map(|n| n.entities.len())
        .sum();
    let n =
        phase.multidrawable_meshes.len() + phase.batchable_meshes.len() + unbatchable + non_mesh;
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Step 3 over one sorted phase's items in draw order, as `(batch range,
/// extra index)`: `render_range` draws once per item with a non-empty batch
/// range and skips `batch_range.len()` items; an item whose extra index is
/// an indirect-parameters index is not counted again here.
pub fn sorted_draws(items: impl IntoIterator<Item = (Range<u32>, PhaseItemExtraIndex)>) -> u32 {
    let items: Vec<(Range<u32>, PhaseItemExtraIndex)> = items.into_iter().collect();
    let mut draws = 0u32;
    let mut index = 0;
    while index < items.len() {
        let (range, extra) = &items[index];
        if range.is_empty() {
            index += 1;
        } else {
            if !matches!(extra, PhaseItemExtraIndex::IndirectParametersIndex { .. }) {
                draws = draws.saturating_add(1);
            }
            index += range.len();
        }
    }
    draws
}

/// [`sorted_draws`] over the sorted phase of the view `view` in `phases`.
fn sorted_view_draws<I: SortedPhaseItem>(
    phases: Option<&ViewSortedRenderPhases<I>>,
    view: &RetainedViewEntity,
) -> u32 {
    phases.and_then(|p| p.get(view)).map_or(0, |phase| {
        sorted_draws(
            phase
                .items
                .values()
                .map(|item| (item.batch_range().clone(), item.extra_index())),
        )
    })
}

/// [`binned_direct_draws`] over the binned phase of the view `view`.
fn binned_view_draws<BPI: BinnedPhaseItem>(
    phases: Option<&ViewBinnedRenderPhases<BPI>>,
    view: &RetainedViewEntity,
) -> u32 {
    phases
        .and_then(|p| p.get(view))
        .map_or(0, binned_direct_draws)
}

/// Render world, every frame: steps 1 to 3 over every extracted view,
/// recorded into [`DrawStats`], and the `debug` line the trace cross-check
/// reads.
#[allow(clippy::too_many_arguments)] // Bevy system: one phase resource each
pub fn count_draws(
    stats: Res<DrawStats>,
    support: Option<Res<GpuPreprocessingSupport>>,
    indirect: Option<Res<IndirectParametersBuffers>>,
    views: Query<(&ExtractedView, Has<NoIndirectDrawing>)>,
    opaque: Option<Res<ViewBinnedRenderPhases<Opaque3d>>>,
    alpha_mask: Option<Res<ViewBinnedRenderPhases<AlphaMask3d>>>,
    shadow: Option<Res<ViewBinnedRenderPhases<Shadow>>>,
    transparent: Option<Res<ViewSortedRenderPhases<Transparent3d>>>,
    transmissive: Option<Res<ViewSortedRenderPhases<Transmissive3d>>>,
    ui: Option<Res<ViewSortedRenderPhases<TransparentUi>>>,
) {
    let mut counts = DrawCounts {
        indirect: indirect.as_deref().map_or(0, indirect_draws),
        ..default()
    };
    for (extracted, no_indirect) in &views {
        let view = &extracted.retained_view_entity;
        if no_indirect {
            counts.direct = counts
                .direct
                .saturating_add(binned_view_draws(opaque.as_deref(), view))
                .saturating_add(binned_view_draws(alpha_mask.as_deref(), view))
                .saturating_add(binned_view_draws(shadow.as_deref(), view));
        }
        counts.sorted = counts
            .sorted
            .saturating_add(sorted_view_draws(transparent.as_deref(), view))
            .saturating_add(sorted_view_draws(transmissive.as_deref(), view))
            .saturating_add(sorted_view_draws(ui.as_deref(), view));
    }
    let mode = support.map_or(CountMode::Unknown, |s| s.max_supported_mode.into());
    let (frame, counted) = stats.record(counts, mode);
    if counted {
        debug!(
            "draw_stats: frame {frame} total={} indirect={} direct={} sorted={}",
            counts.total(),
            counts.indirect,
            counts.direct,
            counts.sorted
        );
    }
}

/// Triangles drawn by the main pass and in the whole scene, per frame.
#[derive(Resource, Debug, Default)]
pub struct FrameTriangles {
    /// Triangle count per mesh asset, computed once.
    cache: HashMap<AssetId<Mesh>, u32>,
    /// Frames sampled.
    pub frames: u32,
    /// Sum of the main-pass triangles over the sampled frames.
    pub main_sum: u64,
    /// Most main-pass triangles in one frame.
    pub main_max: u64,
    /// Most triangles over every mesh entity in one frame.
    pub scene_max: u64,
}

impl FrameTriangles {
    /// The `triangles ...` exit line (without the prefix).
    pub fn line(&self) -> String {
        let mean = if self.frames == 0 {
            0
        } else {
            self.main_sum / u64::from(self.frames)
        };
        format!(
            "triangles main_mean={mean} main_max={} scene={}",
            self.main_max, self.scene_max
        )
    }

    /// Triangles of `mesh`, cached by asset id; 0 for a mesh that is not
    /// loaded or not a triangle list.
    fn of(&mut self, id: AssetId<Mesh>, meshes: &Assets<Mesh>) -> u32 {
        if let Some(n) = self.cache.get(&id) {
            return *n;
        }
        let Some(mesh) = meshes.get(id) else {
            return 0;
        };
        let n = mesh_triangles(mesh);
        self.cache.insert(id, n);
        n
    }
}

/// Triangles of a triangle-list mesh: a third of its indices, or of its
/// vertices when it has none; 0 for any other topology.
pub fn mesh_triangles(mesh: &Mesh) -> u32 {
    if mesh.primitive_topology() != PrimitiveTopology::TriangleList {
        return 0;
    }
    let n = match mesh.indices() {
        Some(Indices::U16(i)) => i.len(),
        Some(Indices::U32(i)) => i.len(),
        None => mesh.count_vertices(),
    };
    u32::try_from(n / 3).unwrap_or(u32::MAX)
}

/// Main world, every frame after the visibility pass: the triangles of the
/// meshes the RTS camera sees (its `VisibleEntities`, not `ViewVisibility`,
/// which the shadow cascades set too) and of every mesh entity.
fn sample_triangles(
    cameras: Query<&VisibleEntities, With<RtsCamera>>,
    mesh_entities: Query<&Mesh3d>,
    meshes: Option<Res<Assets<Mesh>>>,
    mut tris: ResMut<FrameTriangles>,
) {
    let Some(meshes) = meshes else {
        return;
    };
    let Ok(visible) = cameras.single() else {
        return;
    };
    let mut main = 0u64;
    for entity in visible.iter(TypeId::of::<Mesh3d>()) {
        if let Ok(mesh) = mesh_entities.get(*entity) {
            main += u64::from(tris.of(mesh.id(), &meshes));
        }
    }
    let mut scene = 0u64;
    for mesh in &mesh_entities {
        scene += u64::from(tris.of(mesh.id(), &meshes));
    }
    tris.frames += 1;
    tris.main_sum += main;
    tris.main_max = tris.main_max.max(main);
    tris.scene_max = tris.scene_max.max(scene);
}

/// The largest per-frame total among the fight window's frames.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct FightDraws(pub u32);

/// Main world, every frame of the fight window (the `enemy_outpost`
/// scenarios and `art_bench`): the latest counted frame's total.
fn sample_fight_draws(
    stats: Res<DrawStats>,
    fight: Option<Res<FightWindow>>,
    sim: NonSend<SimHandle>,
    mut fight_draws: ResMut<FightDraws>,
) {
    let Some(fight) = fight else {
        return;
    };
    if fight.is_fight_frame(sim.tick()) {
        let summary = stats.summary();
        if summary.frames > 0 {
            fight_draws.0 = fight_draws.0.max(summary.last.total());
        }
    }
}

/// Runs in `Last`, after `bevy::window::ExitSystems`: on the frame an
/// `AppExit` was written, print the `draws` and `triangles` lines.
fn print_draws_on_exit(
    mut exits: MessageReader<AppExit>,
    stats: Res<DrawStats>,
    tris: Res<FrameTriangles>,
    fight: Option<Res<FightWindow>>,
    fight_draws: Res<FightDraws>,
) {
    if exits.is_empty() {
        return;
    }
    exits.clear();
    let fight_max = fight.map(|_| fight_draws.0);
    println!("{STDOUT_PREFIX}{}", stats.summary().line(fight_max));
    println!("{STDOUT_PREFIX}{}", tris.line());
}

/// Parse [`WGPU_DISABLE_ENV`]'s value: comma-separated `wgpu::Features`
/// names in any case (`indirect_first_instance`); an unknown name is an
/// error, so a typo never runs the cross-check in the wrong mode.
pub fn parse_disabled_features(value: &str) -> Result<WgpuFeatures, String> {
    let mut features = WgpuFeatures::empty();
    for name in value.split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let flag = WgpuFeatures::from_name(&name.to_ascii_uppercase())
            .ok_or_else(|| format!("{WGPU_DISABLE_ENV}: unknown wgpu feature `{name}`"))?;
        features |= flag;
    }
    Ok(features)
}

/// Counts the draws, enabled by `--draw-stats` and in `dev` builds.
pub struct DrawStatsPlugin;

impl Plugin for DrawStatsPlugin {
    fn build(&self, app: &mut App) {
        let stats = DrawStats::default();
        app.insert_resource(stats.clone())
            .init_resource::<FrameTriangles>()
            .init_resource::<FightDraws>()
            .add_systems(Update, sample_fight_draws)
            .add_systems(Last, sample_triangles.before(print_draws_on_exit))
            .add_systems(Last, print_draws_on_exit.after(bevy::window::ExitSystems));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(stats).add_systems(
                Render,
                count_draws
                    .in_set(RenderSystems::PrepareBindGroups)
                    .after(bevy::ui_render::prepare_uinodes),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::render::render_phase::{
        BinnedRenderPhaseType, DrawFunctionId, InputUniformIndex, PhaseItem, PhaseItemBatchSetKey,
    };
    use bevy::render::sync_world::MainEntity;

    #[test]
    fn indirect_buffers_count_one_draw_per_entry() {
        let mut phase = UntypedPhaseIndirectParametersBuffers::new(false);
        assert_eq!(phase_indirect_draws(&phase), 0);
        phase.allocate(true, 5);
        phase.add_batch_set(true, 0);
        assert_eq!(phase_indirect_draws(&phase), 5, "one draw per entry");
        phase.allocate(false, 2);
        assert_eq!(phase_indirect_draws(&phase), 7, "indexed and non-indexed");
        let mut shadow = UntypedPhaseIndirectParametersBuffers::new(false);
        shadow.allocate(true, 3);
        let mut all = IndirectParametersBuffers::default();
        all.buffers.insert(TypeId::of::<Opaque3d>(), phase);
        all.buffers.insert(TypeId::of::<Shadow>(), shadow);
        assert_eq!(indirect_draws(&all), 10, "every phase");
        // `clear_indirect_parameters_buffers` (PrepareViews) empties them.
        for buffers in all.buffers.values_mut() {
            buffers.clear();
        }
        assert_eq!(indirect_draws(&all), 0);
    }

    fn items(ranges: &[Range<u32>]) -> Vec<(Range<u32>, PhaseItemExtraIndex)> {
        ranges
            .iter()
            .map(|r| (r.clone(), PhaseItemExtraIndex::None))
            .collect()
    }

    #[test]
    fn sorted_items_count_one_draw_per_batch() {
        // A batch of three (the two empty items after it are inside it),
        // then two single draws.
        assert_eq!(sorted_draws(items(&[0..3, 3..3, 3..3, 3..4, 4..5])), 3);
        // Empty items outside a batch are skipped one by one.
        assert_eq!(sorted_draws(items(&[0..0, 0..0, 0..2, 2..2])), 1);
        assert_eq!(sorted_draws(items(&[])), 0);
        // Gizmos and UI batches of one each.
        assert_eq!(sorted_draws(items(&[0..1, 1..2, 2..3, 3..4])), 4);
        // A dynamic offset (WebGL) is still a direct draw.
        let dynamic = vec![(0..1, PhaseItemExtraIndex::DynamicOffset(64))];
        assert_eq!(sorted_draws(dynamic), 1);
    }

    #[test]
    fn indirect_sorted_items_are_not_counted_twice() {
        let indirect = |range: Range<u32>| PhaseItemExtraIndex::IndirectParametersIndex {
            range,
            batch_set_index: None,
        };
        // A transparent mesh batch drawn indirectly (step 1 counted its
        // entry), then a gizmo: one sorted draw.
        let mixed = vec![
            (0..2, indirect(4..5)),
            (2..2, PhaseItemExtraIndex::None),
            (2..3, PhaseItemExtraIndex::None),
        ];
        assert_eq!(sorted_draws(mixed), 1);
        assert_eq!(sorted_draws(vec![(0..1, indirect(0..1))]), 0);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    struct TestSet(bool);

    impl PhaseItemBatchSetKey for TestSet {
        fn indexed(&self) -> bool {
            self.0
        }
    }

    /// A minimal binned phase item, so a real `BinnedRenderPhase` can be
    /// filled without a GPU.
    struct TestItem {
        entity: (Entity, MainEntity),
        batch_range: Range<u32>,
        extra_index: PhaseItemExtraIndex,
    }

    impl PhaseItem for TestItem {
        fn entity(&self) -> Entity {
            self.entity.0
        }
        fn main_entity(&self) -> MainEntity {
            self.entity.1
        }
        fn draw_function(&self) -> DrawFunctionId {
            DrawFunctionId(0)
        }
        fn batch_range(&self) -> &Range<u32> {
            &self.batch_range
        }
        fn batch_range_mut(&mut self) -> &mut Range<u32> {
            &mut self.batch_range
        }
        fn extra_index(&self) -> PhaseItemExtraIndex {
            self.extra_index.clone()
        }
        fn batch_range_and_extra_index_mut(
            &mut self,
        ) -> (&mut Range<u32>, &mut PhaseItemExtraIndex) {
            (&mut self.batch_range, &mut self.extra_index)
        }
    }

    impl BinnedPhaseItem for TestItem {
        type BinKey = u32;
        type BatchSetKey = TestSet;
        fn new(
            _: TestSet,
            _: u32,
            entity: (Entity, MainEntity),
            batch_range: Range<u32>,
            extra_index: PhaseItemExtraIndex,
        ) -> Self {
            Self {
                entity,
                batch_range,
                extra_index,
            }
        }
    }

    /// A view without indirect drawing draws each bin once, each
    /// unbatchable entity and each non-mesh item once: what step 2 counts
    /// on a real `BinnedRenderPhase`.
    #[test]
    fn no_indirect_views_count_bins_and_unbatchables() {
        let mut world = World::new();
        let mut entity = || {
            let e = world.spawn_empty().id();
            (e, MainEntity::from(e))
        };
        let view = RetainedViewEntity::new(entity().1, None, 0);
        let mut phases = ViewBinnedRenderPhases::<TestItem>::default();
        phases.prepare_for_new_frame(view, GpuPreprocessingMode::PreprocessingOnly);
        let phase = phases.get_mut(&view).expect("prepared");
        let input = InputUniformIndex(0);
        // Bin 1: three batchable entities, one draw. Bin 2: two, one draw.
        for _ in 0..3 {
            phase.add(
                TestSet(true),
                1,
                entity(),
                input,
                BinnedRenderPhaseType::BatchableMesh,
            );
        }
        for _ in 0..2 {
            phase.add(
                TestSet(true),
                2,
                entity(),
                input,
                BinnedRenderPhaseType::BatchableMesh,
            );
        }
        assert_eq!(binned_direct_draws(phase), 2);
        // A multidrawable mesh in a view that is not culling on the GPU is
        // binned as batchable: bin 3, one more draw.
        phase.add(
            TestSet(false),
            3,
            entity(),
            input,
            BinnedRenderPhaseType::MultidrawableMesh,
        );
        assert!(phase.multidrawable_meshes.is_empty());
        assert_eq!(binned_direct_draws(phase), 3);
        // Two unbatchable entities in one bin draw twice; a non-mesh item once.
        for _ in 0..2 {
            phase.add(
                TestSet(true),
                4,
                entity(),
                input,
                BinnedRenderPhaseType::UnbatchableMesh,
            );
        }
        phase.add(
            TestSet(true),
            5,
            entity(),
            input,
            BinnedRenderPhaseType::NonMesh,
        );
        assert_eq!(binned_direct_draws(phase), 6);
        // Through the view lookup `count_draws` uses.
        assert_eq!(binned_view_draws(Some(&phases), &view), 6);
        let other = RetainedViewEntity::new(entity().1, None, 1);
        assert_eq!(binned_view_draws(Some(&phases), &other), 0);
        assert_eq!(binned_view_draws::<TestItem>(None, &view), 0);
    }

    #[test]
    fn draw_line_reports_mean_p95_max_and_mode() {
        let stats = DrawStats::default();
        let frame = |indirect, direct, sorted| DrawCounts {
            indirect,
            direct,
            sorted,
        };
        // The start-up frames are seen but not counted.
        for i in 1..=SKIP_FRAMES {
            assert_eq!(
                stats.record(frame(9000, 0, 0), CountMode::Culling),
                (i, false)
            );
        }
        assert_eq!(stats.summary().frames, 0);
        assert_eq!(stats.summary().max, 0);
        // Twenty frames of 1..=20 draws, the last split by source.
        for n in 1..=19 {
            stats.record(frame(n, 0, 0), CountMode::Culling);
        }
        assert_eq!(
            stats.record(frame(12, 3, 5), CountMode::Culling),
            (SKIP_FRAMES + 20, true)
        );
        let s = stats.summary();
        assert_eq!(s.frames, 20);
        assert_eq!(s.max, 20);
        assert_eq!(s.p95, 19, "p95 of 1..=20 is the 19th");
        assert!((s.mean - 10.5).abs() < 1e-9, "{}", s.mean);
        assert_eq!(s.last, frame(12, 3, 5));
        assert_eq!(s.max_by_source, frame(19, 3, 5));
        assert_eq!(s.mode, CountMode::Culling);
        assert_eq!(
            s.line(None),
            "draws frames=20 mean=10.5 p95=19 max=20 indirect_max=19 direct_max=3 sorted_max=5 mode=culling"
        );
        assert!(s.line(Some(17)).ends_with(" mode=culling fight_max=17"));
        assert_eq!(s.panel_text(), "20 (max 20) / 500  ind 12 dir 3 sorted 5");
        assert!(!s.over_budget());
        stats.record(frame(400, 90, 10), CountMode::PreprocessingOnly);
        let s = stats.summary();
        assert!(s.over_budget(), "500 is over the budget's line");
        assert_eq!(s.mode, CountMode::PreprocessingOnly);
        assert!(s.line(None).ends_with(" mode=preprocessing_only"));
        // A total past the histogram lands in its last bucket; the max is exact.
        let big = DrawStats::default();
        for _ in 0..SKIP_FRAMES {
            big.record(frame(0, 0, 0), CountMode::None);
        }
        big.record(frame(HISTOGRAM_MAX + 100, 0, 0), CountMode::None);
        let s = big.summary();
        assert_eq!((s.max, s.p95), (HISTOGRAM_MAX + 100, HISTOGRAM_MAX));
        assert!(s.line(None).ends_with(" mode=none"));
        assert_eq!(DrawStats::default().summary().mode, CountMode::Unknown);
    }

    #[test]
    fn triangles_come_from_indices_or_vertices() {
        let cube = Mesh::from(Cuboid::new(1.0, 1.0, 1.0));
        assert_eq!(mesh_triangles(&cube), 12);
        let mut flat = cube.clone();
        flat.duplicate_vertices();
        assert_eq!(
            mesh_triangles(&flat),
            12,
            "unindexed: a third of the vertices"
        );
        let lines = Mesh::new(
            PrimitiveTopology::LineList,
            bevy::asset::RenderAssetUsages::default(),
        );
        assert_eq!(mesh_triangles(&lines), 0);
        let mut meshes = Assets::<Mesh>::default();
        let id = meshes.add(cube).id();
        let mut tris = FrameTriangles::default();
        assert_eq!(tris.of(id, &meshes), 12);
        assert_eq!(tris.of(AssetId::default(), &meshes), 0, "not loaded");
        tris.frames = 2;
        tris.main_sum = 30;
        tris.main_max = 20;
        tris.scene_max = 40;
        assert_eq!(tris.line(), "triangles main_mean=15 main_max=20 scene=40");
    }

    #[test]
    fn wgpu_features_are_disabled_by_name() {
        assert_eq!(
            parse_disabled_features("indirect_first_instance"),
            Ok(WgpuFeatures::INDIRECT_FIRST_INSTANCE)
        );
        assert_eq!(
            parse_disabled_features(" INDIRECT_FIRST_INSTANCE , immediates "),
            Ok(WgpuFeatures::INDIRECT_FIRST_INSTANCE | WgpuFeatures::IMMEDIATES)
        );
        assert_eq!(parse_disabled_features(""), Ok(WgpuFeatures::empty()));
        let err = parse_disabled_features("no_such_wgpu_feature").unwrap_err();
        assert!(err.contains("no_such_wgpu_feature"), "{err}");
    }
}
