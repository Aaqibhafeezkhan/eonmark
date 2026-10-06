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
| Alpha | opaque, except the territory overlay and HUD panels |

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
| Units | `Capsule` | Height per kind from M4b; five distinct looks for the military kinds |
| Buildings | `Cuboid` | Footprint from `buildings.ron` |
| Town | `Cylinder` | Larger when it reaches the second radius tier |
| Resource node | `Sphere` | Grain gold, timber, ore colours |
| Selection ring, rally marker, move marker | Retained gizmo lines | Separate entities keyed by id, never children of moving units |
| Health bar | Flat quad entity | Same rule |

Motion is procedural (bob, swing, scale pulse), not animation clips, in v0.1.

## Territory and fog

| Element | Value |
|---|---|
| Owned-land tint | about 12% team colour over the ground |
| Border | a 1-tile soft isoline where ownership changes |
| Neutral or tied tiles | no tint |
| Fog, explored | ground and buildings darkened to 0.45 |
| Fog, unexplored | 0.0 (hidden) |
| Supply coverage | subtle hatching from the same field texture (M5a) |

M3b ships the border as a vertex-coloured overlay mesh first and upgrades to the `FieldExt` extended material sampling a 128x128 RGBA8 field texture (R owner, G strength, B fog, A supply). The shader is mandatory by M7 because fog rides the same texture.

## HUD

`bevy_ui` only in shipped builds. Inter (variable weight) for all text. Kenney UI Pack 9-slice panels through `NodeImageMode::Sliced`. Matte panels, thin borders, no drop shadows, no glow. The top resource bar shows current stockpile and per-30-s rate per resource and turns the readout amber while production is being discarded at the Yield Cap. Disabled command-card buttons are greyed with a tooltip, not hidden. Layout-only root nodes are `Pickable::IGNORE` so empty HUD space passes clicks to the world. `bevy_egui` appears only behind the `dev` feature.

## P3 and sRGB check

wgpu on a wide-gamut display can render colours more saturated than authored. Every palette or art change is judged twice: once on the development Mac's P3 display, once as an sRGB screenshot opened in Preview with the display profile set to sRGB. Both must look matte. If a colour reads as neon in either, lower its saturation until both agree.

## Primitives first, models by data

Every unit and building kind is drawn through one `Visual` value from `data/visuals.ron`:

```ron
Yeoman: Primitive(Capsule),
Town: Scene(path: "models/town.glb", scale: 1000, y_offset: 0, yaw: 0),
```

`Visual::Primitive(kind)` draws a team-coloured primitive. `Visual::Scene(path, scale, y_offset, yaw)` loads a glTF scene; `scale` is permille, `y_offset` is in millimetres, `yaw` is in integer degrees, so the file stays float-free. Swapping a cube for a model is a RON edit, never a Rust change. M0 to M6 ship primitives only; M7 maps every kind to CC0 models and keeps the primitive as the fallback.

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

## Milestones

| Milestone | Visual deliverable |
|---|---|
| M0 | Flat matte ground plane, directional light, camera stub |
| M2 | Team-coloured primitives, gizmo rings and markers |
| M3b | Territory tint and border; build ghost in green or red |
| M4b | Five distinct unit silhouettes; health bars |
| M5a | Attrition indicator, supply ring, annexation timer |
| M7 | Fog through the field shader, minimap, Inter, Kenney panels, CC0 glTF models, audio; palette finalized here |
