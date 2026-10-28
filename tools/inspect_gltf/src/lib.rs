//! `inspect_gltf`: the art pass's asset tool (`docs/ROADMAP.md`, "Art pass
//! (pre-stream)"; `docs/ART_STYLE.md`). It reads glTF 2.0 files with the
//! `gltf` crate (no `import` feature) and PNG textures with `png`, and has
//! no engine dependency: no Bevy, no `glam`, no `image` crate. The
//! subcommands are `inspect`, `swatches`, `palette`, `bake` and `check`;
//! `src/main.rs` is the command line.
//!
//! Downloaded files are data: the tool only parses them, URIs must be
//! plain relative paths, and nothing it reads is ever executed.

pub mod bake;
pub mod check;
pub mod glob;
pub mod image;
pub mod inspect;
pub mod math;
pub mod model;
pub mod palette;
pub mod sources;
pub mod swatches;
pub mod uv;
pub mod writer;

#[cfg(test)]
pub(crate) mod testkit;
