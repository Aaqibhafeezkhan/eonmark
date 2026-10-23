# Art style

This document is the visual contract for Eonmark. It fixes the palette, material and lighting settings, the readability rule, the HUD look, the primitives-first approach and the rule for swapping in real models, the glTF import rules, the allowed asset sources and licences, and the screenshot checklist that every art PR is judged against. The look is calm, matte, flat-shaded low-poly 3D on a flat ground plane. The contract applies from M0 (the ground plane) through M7 (the look pass); the hex values are starting suggestions and are finalized at the M7 screenshot sign-off. Licensing rules are in [adr/0001-license.md](adr/0001-license.md).

## The look in one paragraph

A board game seen from a fixed-pitch camera. Flat colours, soft shadows, no shine, no glow. Units and buildings are simple silhouettes in team colour. Owned land is tinted a little and bounded by a soft line. The HUD is numbers and borders on matte panels. Nothing flashes, pulses or blooms. Readability at the far zoom beats detail at the near zoom.

## Palette

At most 12 base hues plus 2 team colours. Keep saturation low and value in the middle; nothing pure white, nothing pure black. These are suggestions in a matte range; the final values are measured at M7 and recorded here.

| Role | Suggested hex | Notes |
|---|---|---|
| Grass | `#7A9B5C` | Ground plane default |
| Forest floor | `#5C7A4A` | Forest tiles; tree clusters sit on it |
| Rock | `#8C8578` | Mountain tiles |
| Water | `#5B7C8F` | The impassable strip; flat, no animation |
| Soil and paths | `#A08C6A` | Build footprints |
| Timber | `#8B6B4A` | Lumber Yard, Supply Wain |
| Stone | `#B5AFA3` | Towns, Watchtower, Engine Yard |
| Thatch | `#B08D57` | Croft roofs |
| Ore | `#6E6A72` | Ore Pit, Mangonel frame |
| Grain gold | `#C9A961` | Grain resource icon and node |
| Lore ink | `#4F5B7A` | Scriptorium accent, Lore icon |
| Neutral and fog | `#3A3A3F` | Neutral territory, fog overlay colour, panel base |

| Team | Suggested hex | Notes |
|---|---|---|
| Blue | `#3F6FA8` | Player 1 default; match to the KayKit blue atlas swatch at M7 |
| Red | `#B04A3F` | Player 2 default; match to the KayKit red atlas swatch at M7 |

HUD accents are drawn from the same table plus two states:

| HUD role | Suggested hex |
|---|---|
| Text | `#ECE9E2` |
| Panel | `#26262A` at 92% opacity |
| Disabled | `#6B6B70` |
| Yield Cap amber | `#D29B3B` |
| Affordable or valid placement | `#6FA06A` |
| Unaffordable or invalid placement | `#B04A3F` |

## Material settings

One `StandardMaterial` per kind and team, shared by every instance of that kind.

| Property | Value |
|---|---|
| `perceptual_roughness` | 0.95 |
| `reflectance` | 0.2 |
| `metallic` | 0.0 |
| `emissive` | black; never used for gameplay signalling |
| Alpha | opaque, except HUD panels; territory is drawn inside the opaque ground material by `FieldExt`, not as a separate overlay |

## Lighting and post-processing

| Setting | Value |
|---|---|
| Lights | One `DirectionalLight` with soft cascaded shadows, plus `AmbientLight` |
| Tonemapping | `Tonemapping::TonyMcMapface` |
| Anti-aliasing | `Msaa::Sample4` |
| Bloom | none |
| Emissive glow | none |
| SSAO | optional, low strength only; off by default |
| Other post-processing | none |
| Present mode | `PresentMode::AutoVsync` (Mailbox is unsupported on Metal) |

## Readability at 40 m

The camera zooms between 15 m and 60 m at a pitch of about 55 degrees. The acceptance distance is 40 m: at that zoom, in a 1280x720 screenshot, every unit kind must be identifiable by silhouette and every team by colour. Shape carries identity, colour carries ownership. If a kind is only distinguishable up close, change its silhouette, not its texture.

Primitive silhouettes (M2 to M7):

| Kind | Primitive | Notes |
|---|---|---|
| Units | One shape per kind (table below) | Since M4b each kind has its own silhouette and height, low-poly and flat-shaded; M2 drew every unit as a smooth `Capsule` |
| Buildings | `Cuboid` | Footprint from `buildings.ron` |
| Town | `Cylinder` | Larger when it reaches the second radius tier |
| Resource node | `Sphere` | Grain gold, timber, ore colours |
| Selection ring, rally marker, move marker | Retained gizmo lines | Separate entities keyed by id, never children of moving units |
| Attrition indicator, supply ring (M5a) | Retained gizmo lines | Same rule; a steady Yield Cap amber (#D29B3B) diamond under a unit bleeding on enemy land, a Timber ring around a selected Supply Wain; nothing pulses |
| Annexation timer (M5a) | Immediate gizmo lines, 3 px | A #3A3A3F track circle just outside a contested Town and the attacker's team-colour arc filling clockwise from north |
| Health bar | Flat quad entity | Same rule; unlit, a team-colour fill on a #3A3A3F track, no green-to-red, shown only while the target is damaged or selected (M4b, `health_bars.rs`) |

Unit looks (M4b, `data/visuals.ron`), one silhouette per kind. Width is the kind's collision diameter from `units.ron`; each mesh has 10 facets around (a 6-latitude capsule, an icosphere of 80 faces) and flat face normals, at most 600 vertices, one mesh per kind and one material per kind and team:

| Kind | Look | Silhouette |
|---|---|---|
| Yeoman | `Primitive(Capsule)` | Upright pill, 0.7 m wide, 1.19 m tall |
| Scribe | `Primitive(Cylinder)` | Drum, 0.7 x 0.7 m |
| Skirmisher | `Body(shape: Cone, height_tiles_x100: 150)` | Spike, 1.5 m |
| Shieldbearer | `Body(shape: Cuboid, height_tiles_x100: 130)` | Block, 0.8 x 1.3 x 0.8 m |
| Bowman | `Body(shape: Cylinder, height_tiles_x100: 190)` | Tall post, 1.9 m |
| Outrider | `Body(shape: Capsule, height_tiles_x100: 180, lying: true)` | Pill lying along its facing, 1.8 m long (twice its collision diameter; see the crowd check below), 0.9 m tall |
| Mangonel | `Primitive(Sphere)` | Ball, 1.0 m |
| Supply Wain | `Body(shape: Cuboid, height_tiles_x100: 60)` | Low box, 1.0 x 0.6 x 1.0 m |

Two kinds share a shape only when they differ by at least 1.4 times on some axis (the Scribe's drum and the Bowman's post, the Shieldbearer's block and the Wain's low box, the Yeoman's upright pill and the Outrider's lying one); `present::tests::every_unit_kind_has_its_own_silhouette` checks it. The 40 m view is `--scenario unit_looks`: one unit of each kind per team in two rows 15 m apart, framed at 40 m unless `--camera` is given (`target/ci/eonmark --background --scenario unit_looks --exit-after-seconds 40 --screenshot-at 100:looks.png`; CI's render smoke test renders it on every PR). The owner chose single-shape silhouettes over multi-part looks on 2026-10-22 (ROADMAP M4b, Notes, decision 11); multi-part looks wait for the art pass. The looks themselves are signed off on the 40 m screenshot, and that sign-off is still pending: the owner's run on the Mac, with the P3 and sRGB check and the checklist below.

The sign-off also looks at a crowd, because the lineup stands every unit alone 4 m from the next and cannot show how the looks pack in a fight: `target/ci/eonmark --background --scenario enemy_outpost --camera 100,32,40 --exit-after-seconds 60 --screenshot-at 250:crowd.png` (`just render-smoke out enemy_outpost 100,32,40 250` under Xvfb) frames the 200 vs 200 prologue at 40 m mid-fight. Every look is exactly its kind's collision diameter across except the Outrider's, whose lying pill is 1.8 m long against a 0.9 m diameter. Melee range is measured edge to edge (0.25 m for the Outrider) and the sim keeps units apart only by a soft push, so two Outriders holding on each other stand at most 1.15 m apart, centre to centre, while each pill reaches 0.9 m ahead of its centre: head-on they overlap by at least 0.65 m, and in a packed melee the pills visibly pass through each other. Shortening the pill is a data-only change, `height_tiles_x100` of `"outrider"` in `data/visuals.ron` (120 keeps a lying pill, cuts the head-on overlap to 0.05 m and still differs from the Yeoman's upright pill by 1.7 times along its length, and `present::tests::every_unit_kind_has_its_own_silhouette` passes with it, but reads closer to the Mangonel's 1.0 m ball); because `visuals.ron` is in `rules_hash`, it needs a `rules_version` bump and `just rerecord` in the same commit. The owner decides that trade at the sign-off.

Motion is procedural (bob, swing, scale pulse), not animation clips, in v0.1.

## Territory and fog

| Element | Value |
|---|---|
| Owned-land tint | about 12% team colour over the ground |
| Border | a 1-tile soft isoline where ownership changes |
| Neutral or tied tiles | no tint, except within half a tile of owned land, where the owner's tint fades from about half of the 12% at the edge to nothing at the tile centre, under the border line |
| Fog, explored | ground and buildings darkened to 0.45 |
| Fog, unexplored | 0.0 (hidden) |
| Supply coverage | a thin Timber (#8B6B4A) ring of radius 14.5 m around a selected Supply Wain (M5a, `markers.rs`); the subtle hatching from the same field texture's A channel waits for M7 with fog (M5a owner decision 10) |

The ground's material is the `FieldExt` extended material (`crates/game/src/field.rs`, shader `field.wgsl` embedded in the binary) reading an RGBA8 field texture with one texel per tile, 128x128 on the current maps (R owner, G strength, B fog, A supply). Each owner's membership is interpolated between tile centres with smoothstep weights; the tint follows it and the border is the isoline where it crosses one half, fading to nothing at the tile centres on either side, so the line follows the buildable tiles with rounded corners. M3b drew a vertex-coloured version first and replaced it within the milestone. The shader is mandatory by M7 because fog rides the same texture.

## HUD

`bevy_ui` only in shipped builds. Inter (variable weight) for all text. Kenney UI Pack 9-slice panels through `NodeImageMode::Sliced`. Matte panels, thin borders, no drop shadows, no glow. The top resource bar shows current stockpile and per-30-s rate per resource and turns the readout amber while production is being discarded at the Yield Cap. Disabled command-card buttons are greyed with a tooltip, not hidden. Layout-only root nodes are `Pickable::IGNORE` so empty HUD space passes clicks to the world. `bevy_egui` appears only behind the `dev` feature.

## P3 and sRGB check

wgpu on a wide-gamut display can render colours more saturated than authored. Every palette or art change is judged twice: once on the development Mac's P3 display, once as an sRGB screenshot opened in Preview with the display profile set to sRGB. Both must look matte. If a colour reads as neon in either, lower its saturation until both agree.

## Primitives first, models by data

Every unit kind is drawn through one `Visual` value from `data/visuals.ron`, keyed by its `units.ron` id (buildings are drawn by `crates/game/src/buildings.rs` until M7 moves them into the file; a building key fails `data-check` today):

```ron
"yeoman": Primitive(Capsule),
"skirmisher": Body(shape: Cone, height_tiles_x100: 150),
"outrider": Body(shape: Capsule, height_tiles_x100: 180, lying: true),
"mangonel": Scene(path: "models/mangonel.glb", scale_x100: 100, y_offset_tiles_x100: 0, yaw_deg: 0),
```

`Visual::Primitive(shape)` draws a team-coloured primitive sized from the kind's radius (`Capsule`, `Cuboid`, `Cylinder`, `Sphere` or `Cone`). `Visual::Body(shape, height_tiles_x100, lying)` (M4b) draws the same primitive two radii wide with its own height in hundredths of a metre (20 to 400; never a `Sphere`); `lying: true` lays a `Capsule` or `Cylinder` along the unit's facing, that long and two radii tall. `Visual::Scene(path, scale_x100, y_offset_tiles_x100, yaw_deg)` loads a glTF scene; the scale and offset are in hundredths and the yaw in whole degrees, so the file stays float-free. Swapping a cube for a model is a RON edit, never a Rust change. M0 to M6 ship primitives only; M7 maps every kind to CC0 models and keeps the primitive look as the fallback. Anything drawn above a unit (the M4b health bar) takes its height from `present::look_top_y`, computed from the `Visual`, so the art pass changes that one function. Until M7 the presenter draws a `Scene` as the fallback `Primitive(Capsule)` (`present::FALLBACK_VISUAL`).

## glTF import rules

| Rule | Value |
|---|---|
| Exporter | Blender to glTF Binary (`.glb`) |
| Up axis | +Y |
| Forward | -Z |
| Scale | 1 unit = 1 m; apply transforms before export |
| Origin | base centre of the model |
| Texture | one palette or gradient texture at most 256 px on a side; downsample KayKit 1024 atlases |
| Contents | meshes and materials only; no embedded lights or cameras |
| Naming | `kind_variant.glb`, lower case |
| Loading | `GltfAssetLabel::Scene(0)` |
| Inspection | `tools/inspect_gltf` prints scenes, meshes and bounds (M7) |

The glTF specification says +Z forward and Blender's default export writes -Z forward; kits differ in scale and origin. Normalize at import, record `scale`, `y_offset` and `yaw` in `visuals.ron`, and check the result at the standard camera.

## Allowed asset sources

| Source | Licence | Use |
|---|---|---|
| Kenney (UI Pack, Game Icons, Nature Kit, and other kits) | CC0-1.0 | UI 9-slice, icons, trees, props |
| KayKit (Medieval Builder, Medieval Hexagon, Adventurers) | CC0-1.0 | Buildings with team colours, units |
| Freesound, filtered to CC0 | CC0-1.0 | About 10 OGG clips (M7) |
| Inter | OFL-1.1 | The one UI font; licence text vendored beside the `.ttf` |
| Any other source | CC0-1.0, CC-BY-4.0 or OFL-1.1 only | Each with an `assets/ATTRIBUTION.md` row |

Excluded: Quaternius (proprietary licence since 2026-08-28), CC-BY-SA, CC-BY-NC, GPL art, Sampling+, and any material from the inspiration's fan wiki.

Each vendored file gets an `ATTRIBUTION.md` row: asset, author, source URL, licence, modifications, download date. In-house files are marked `original: yes` and are CC0. `scripts/check_assets.sh` fails CI if a non-allowlisted file lacks a row, exceeds 2 MB, or the total exceeds 300 MB. No Git LFS.

## Screenshot sign-off checklist

Attach one 1280x720 screenshot at 40 m to every PR that adds or changes art, and tick each line.

| Check | Yes |
|---|---|
| Every unit kind on screen is identifiable by silhouette alone | |
| Team colour is the only strong hue on units and buildings | |
| No surface shows specular highlight, glow or bloom | |
| Owned land reads as a soft tint with a visible but not hard border | |
| HUD panels are matte and text is legible at 100% scale | |
| The same screenshot looks matte on P3 and as an sRGB export | |
| Total hue count on screen, excluding team colours, is at most 12 | |
| New files have `ATTRIBUTION.md` rows and `scripts/check_assets.sh` passes | |
| `visuals.ron` changed, Rust did not (for a model swap) | |
| Draw-call counter in the dev panel is under 500 with the test scene | |

The last row has no counter to read yet: Bevy 0.19.1 keeps its batch counts `pub(crate)` and wgpu has no draw counters, so through M4b it is marked n/a in the PR with the expected counts instead (at 200 vs 200 about 75 to 100 batches, [design/ui.md](design/ui.md#health-bars-rings-and-markers)), and an Xcode Metal frame capture on the Mac stands in for it. The dev-panel counter arrives with the pre-stream art pass, where glTF models make it matter (ROADMAP M3b).

## Milestones

| Milestone | Visual deliverable |
|---|---|
| M0 | Flat matte ground plane, directional light, camera stub |
| M2 | Team-coloured primitives, gizmo rings and markers |
| M3b | Territory tint and border; build ghost in green or red |
| M4b | Five distinct unit silhouettes; health bars |
| M5a | Attrition indicator, supply ring, annexation timer |
| M7 | Fog through the field shader, minimap, Inter, Kenney panels, CC0 glTF models, audio; palette finalized here |
