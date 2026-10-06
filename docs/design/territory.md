# Territory

This document specifies the `TerritoryField`, the structure that turns Towns and Watchtowers into borders. Borders decide where a player may build, where attrition applies, and who wins a 45-minute tiebreak, so the field must be exact, cheap to update and a pure function of its sources. Every number here is a starting value in `data/rules/rules.ron` and may move during M6 tuning. The sim code is `crates/sim/src/territory.rs`; the visual is `crates/game/src/ground.rs`. For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M3a (sim), M3b (visual) |
| Labels | `area:sim`; `area:game` for the overlay and shader |
| Rules files | `data/rules/rules.ron` (Town radius 20 -> 24 at 5 distinct buildings, Watchtower push 2, Statecraft push 1) |
| Sim module | `crates/sim/src/territory.rs` |
| Sub-hash | `territory` |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Map | 128 x 128 tiles, flat, mirrored (`plains_1v1`) | `data/maps/plains_1v1.ron` |
| Town radius, tier 1 | 20 tiles | `rules.ron` |
| Town radius, tier 2 (5 distinct building kinds attributed to the Town) | 24 tiles | `rules.ron` |
| Statecraft border push | +1 tile to every own Town radius per level (max +3) | `techs.ron` Modifier row `town_border_radius Add 1` |
| Watchtower border push | own source with radius 2 | `rules.ron`; M9 faction row `Add -1` makes it 1 |
| Strength layer type | `u16` per tile per player, plus one neutral pseudo-player layer | code |
| Tie rule | exact tie between the two strongest layers makes the tile neutral | code, no knob |
| Hysteresis | none | code |
| Bench budget | mean incremental recompute < 2 ms for 16 sources and 100 edits | M3a acceptance |

## Algorithm

### Sources and kernels

A source is `(owner, centre tile, R)`. Owners are players or the neutral pseudo-player used by an annexing Town (see [towns.md](towns.md)). Sources today:

| Source | R |
| --- | --- |
| Town, tier 1 | 20 + Statecraft level of the owner |
| Town, tier 2 | 24 + Statecraft level of the owner |
| Watchtower | 2 |

Each source stamps a disc kernel into its owner's strength layer:

```text
K_R(dx, dy) = max(0, R - d)          where d = isqrt(dx*dx + dy*dy)
```

`isqrt` is the floored integer square root, so the kernel is integer and symmetric under the map mirror. Kernels are cached per distinct `R` (a `(2R+1)^2` table, at most 27 distinct values) and clipped at the map edge.

### Layers and ownership

```text
strength[p][tile]  : u16, sum of every kernel owned by p that covers tile
owner[tile]        : u8, 0 = neutral, otherwise player index + 1

best   = max over p of strength[p][tile]
count  = number of p with strength[p][tile] == best
owner  = if best == 0 or count >= 2 { neutral } else { argmax p }
```

Owner is a pure function of the layers, and the layers are a pure function of the source set. Nothing depends on the previous tick.

### Incremental recompute

```text
fn add_source(src):
    for (dx, dy) in kernel bbox of src.R:
        strength[src.owner][src.centre + (dx, dy)] += K_R(dx, dy)
    recompute_owner(bbox)

fn remove_source(src):
    same with -=

fn change_source(old, new):
    remove_source(old); add_source(new)

fn recompute_owner(rect):
    for tile in rect: owner[tile] = rule above
```

Because layer values are exact integer sums, adding then removing a source restores the layer bit for bit, so incremental recompute equals a full recompute from the source list. `full_recompute()` clears every layer, re-adds every source in `BuildingId` order and recomputes the whole map; the property test asserts `hash(incremental) == hash(full)` after 200 random source edits.

Events that edit sources: a Town is founded; a Town reaches tier 2; the owner researches a Statecraft level (every own Town changes `R`); a Watchtower is completed or destroyed; a Town enters, leaves or completes annexation (its kernel moves between the owner's layer and the neutral layer, then to the new owner's layer).

### Queries

| Query | Meaning |
| --- | --- |
| `owner_at(tile) -> Option<PlayerId>` | `None` for neutral |
| `is_buildable(player, tile) -> bool` | `owner_at(tile) == Some(player)`. Terrain and occupancy are checked separately by construction with their own rejection reasons. |
| `owned_tile_count(player) -> u32` | For the HUD and the 45-minute tiebreak (percentage of 16384 tiles) |
| `strength_at(player, tile) -> u16` | For the field texture and debug panels |

A `Build` whose tile fails `is_buildable` is rejected with `CommandRejected { reason: OutsideBorders }` and changes no state. Founding a Town is a `Build` of kind Town and obeys the same rule: a second Town is placed at the edge of existing borders and then extends them.

## Why ties go neutral

The map is mirrored, so the midline tiles see equal strength from both players' mirrored Towns. Three rules were considered:

| Rule | Pure function of sources | Symmetric on the mirror | Result |
| --- | --- | --- | --- |
| Incumbent hysteresis (first owner keeps the tile) | No: depends on history | No | Rejected. The property test incremental == full would fail on every midline tie. |
| Lowest `PlayerId` wins | Yes | No: player 0 always owns the midline | Rejected |
| Exact tie is neutral | Yes | Yes | Adopted |

With the adopted rule nobody can build on the contested line, which is a reasonable play outcome, and the soft isoline in the border shader hides any one-tile flicker when strengths cross. The property test stays honest, and `restore()` can rebuild nothing because the field is hashed state, not a cache.

## Determinism and hashing

`TerritoryField` is hashed sim state and has its own sub-hash (`territory`). Iteration over sources is in `BuildingId` order. There is no floating point anywhere in the field; `isqrt` is integer. Snapshot and restore serialize the layers and owner grid verbatim.

## Rendering handoff

The game mirrors the field into a 128 x 128 RGBA8 texture each tick that changed it (`sync_field_texture`):

| Channel | Content | Written by |
| --- | --- | --- |
| R | owner id (0 neutral, else player index + 1) | territory, M3b |
| G | strength of the owning layer at the tile, clamped to 255 | territory, M3b |
| B | fog state 0 / 1 / 2 (see [fog.md](fog.md)) | visibility, M7 |
| A | supply coverage (see [attrition.md](attrition.md)) | supply grid, M5a |

M3b ships a vertex-colored overlay mesh first (tint plus border line). The `FieldExt` material that samples this texture replaces it within M3b if time allows and is required by M7. The shader tints owned tiles about 12% toward the team colour and draws a 1-tile soft isoline where R changes. The CPU-painted minimap (M7) reads the same field. See [ui.md](ui.md) and `docs/ART_STYLE.md`.

## Interactions

| System | Interaction |
| --- | --- |
| [economy.md](economy.md) | Building is legal only inside borders. |
| [towns.md](towns.md) | Towns are the main sources; tier 2 radius, town limit and annexation all edit the field. |
| [tech-and-ages.md](tech-and-ages.md) | Statecraft levels add 1 to every own Town's radius. |
| [attrition.md](attrition.md) | Attrition applies to enemy land units on tiles you own. |
| [combat.md](combat.md) | Watchtowers are sources, shoot, and host Harrying research. |
| [ai.md](ai.md) | The bot places Watchtowers at the border edge and reads `owner_at` through `SimView`. |

## Acceptance checklist (from M3a)

- [ ] `cargo test -p sim territory`: the fixture with 2 Towns + 1 Watchtower yields the golden owned-tile count and the mirrored midline tiles are neutral; proptest over 200 random source edits asserts incremental recompute hash == full recompute hash; a Build outside own borders is rejected with `CommandRejected { reason: OutsideBorders }` and no state change.
- [ ] `cargo run -p sim-cli --release -- bench --territory --sources 16 --edits 100` reports mean incremental recompute < 2 ms.

```bash
cargo test -p sim territory
cargo run -p sim-cli --release -- bench --territory --sources 16 --edits 100
```

## Open tuning questions

1. Distance metric: floored Euclidean (`isqrt`) is the M3a reading of "disc kernel". Chebyshev would be cheaper but square.
2. Watchtower as an own source of radius 2 versus adding 2 to the nearest Town's radius. The own-source model rewards placing towers at the frontier, which matches the AI plan; confirm.
3. Town radii 20 and 24 on a 128-tile map leave a neutral band between distant Towns. Is that the intended early-game feel, or should tier 1 be larger?
4. Should the G channel carry the owner's strength or the margin over the runner-up? Margin would make the isoline softer.
5. Owner encoding (0 neutral, player index + 1) is proposed here for the R channel; confirm before M3b writes the shader.
