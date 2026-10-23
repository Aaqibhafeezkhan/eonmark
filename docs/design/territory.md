# Territory

This document specifies the `TerritoryField`, the structure that turns Towns and Watchtowers into borders. Borders decide where a player may build, where attrition applies, and who wins a 45-minute tiebreak, so the field must be exact, cheap to update and a pure function of its sources. Every number here is a starting value in `data/rules/rules.ron` or `data/rules/buildings.ron` and may move during M6 tuning. The sim code is `crates/sim/src/territory.rs`; the visual is `crates/game/src/field.rs` and its shader `field.wgsl` (the ground mesh they draw on is `crates/game/src/ground.rs`). For the design lineage see [prior-art.md](prior-art.md).

| Field | Value |
| --- | --- |
| Milestone | M3a (sim), M3b (visual) |
| Labels | `area:sim`; `area:game` for the overlay and shader |
| Rules files | `data/rules/rules.ron` (Town radius 20 -> 24 at 5 distinct buildings, Statecraft push 1), `data/rules/buildings.ron` (Watchtower `border_radius_tiles: Some(2)`, footprints) |
| Sim module | `crates/sim/src/territory.rs` |
| Sub-hash | `territory` |

## Target numbers

| Parameter | Start value | Where |
| --- | --- | --- |
| Map | 128 x 128 tiles, flat, mirrored (`plains_1v1`) | `data/maps/plains_1v1.ron` |
| Town radius, tier 1 | 20 tiles | `rules.ron` |
| Town radius, tier 2 (5 distinct building kinds attributed to the Town) | 24 tiles | `rules.ron` |
| Statecraft border push | +1 tile to every own Town radius per level (max +3) | `techs.ron` Modifier row `town_border_radius Add 1` |
| Watchtower border push | own source with radius 2 | `buildings.ron` (`watchtower` `border_radius_tiles`); M9 faction row `Add -1` makes it 1 |
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
| Watchtower | 2 (`border_radius_tiles` in `buildings.ron`) |

A border source needs odd footprint sides so its centre is a tile: the Town is 3x3 and the Watchtower 1x1, and `data-check` rejects an even side on any kind that projects a border. A kind's border radius and its vision radius are separate fields: the Watchtower has `border_radius_tiles: Some(2)` and `vision_tiles: 12`, and every unit and building kind carries its own `vision_tiles` (see [fog.md](fog.md)).

Each source stamps a disc kernel into its owner's strength layer:

```text
K_R(dx, dy) = max(0, R - d)          where d = isqrt(dx*dx + dy*dy)
```

`isqrt` is the floored integer square root, so the kernel is integer and symmetric under the map mirror. The kernel is positive exactly where `dx² + dy² < R²`, so a stamp touches `|dx|, |dy| <= R - 1`, clipped at the map edge. Unclipped discs cover 9 tiles at R = 2, 1245 at R = 20, 1789 at R = 24 and 2285 at R = 27 (`territory_kernel_is_the_floored_disc`). The Watchtower's R = 2 therefore claims a 3x3 block: strength 2 on the tower's tile and 1 on the eight around it, so it pushes a border by about one tile beyond the tower. [ai.md](ai.md) describes the Watchtower as pushing the border two tiles; which of the two is meant is open question 6. M3a computes the kernel per stamp instead of caching a `(2R+1)^2` table per `R`: a stamp at R = 27 is at most 2809 integer square roots, which keeps the field free of derived state (nothing to rebuild on restore) and stays far inside the bench budget. A cache can come back later as derived state rebuilt in `rebuild_derived` if a profile ever asks for it.

### Layers and ownership

```text
strength[l][tile]  : u16, sum of every kernel stamped into layer l that covers tile
                     (layer 0 = neutral pseudo-player, layer p + 1 = PlayerId(p))
owner[tile]        : u8, 0 = neutral, otherwise player index + 1

best   = max over every layer l (neutral included) of strength[l][tile]
count  = number of layers with strength[l][tile] == best
owner  = if best == 0 or count >= 2 { neutral } else { argmax l }   // argmax 0 is neutral
```

Because layer 0 is the neutral layer, the owner code is the index of the winning layer: a tile where the neutral kernel wins outright, or ties, is neutral. Owner is a pure function of the layers, and the layers are a pure function of the source set. Nothing depends on the previous tick.

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

`TerritoryField` is hashed sim state and has its own sub-hash (`territory`). Iteration over sources is in `BuildingId` order. There is no floating point anywhere in the field; `isqrt` is integer. Snapshot and restore serialize the layers and owner grid verbatim, each as one little-endian byte string: the M1 proptests hash the whole state every tick, and per-cell varints over about 65k cells would cost seconds of the 10 s `cargo test -p sim` budget. Deserializing checks the field against a recompute of its sources, so a corrupt snapshot fails to decode instead of panicking on a later edit.

## Rendering handoff

The game mirrors the field into a map-sized RGBA8 texture (128 x 128 on `plains_1v1`) after a stepped tick in which the field's sources changed (`field::sync_field`):

| Channel | Content | Written by |
| --- | --- | --- |
| R | owner id (0 neutral, else player index + 1) | territory, M3b |
| G | strength of the owning layer at the tile, clamped to 255 | territory, M3b |
| B | fog state 0 / 1 / 2 (see [fog.md](fog.md)) | visibility, M7 |
| A | supply coverage (see [attrition.md](attrition.md)) | deferred to M7 with fog (M5a owner decision 10); M5a's game slice draws a supply radius ring around a selected Supply Wain instead (attrition.md, "M5a implementation notes") |

M3b drew a vertex-colored version first (tint plus a hard border band) and replaced it within the milestone with the `FieldExt` material that reads this texture (`crates/game/src/field.rs`, `field.wgsl`; required by M7 in any case). The shader tints owned land about 12% toward the team colour and draws a 1-tile soft isoline where R changes: each owner's membership is interpolated between the four nearest tile centres with smoothstep weights, and the line peaks where it crosses one half. The texture is rewritten after a stepped tick only when the field's sources change, since the sources determine every layer and owner. The CPU-painted minimap (M7) reads the same field. See [ui.md](ui.md) and `docs/ART_STYLE.md`.

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

- [x] `cargo test -p sim territory`: the fixture with 2 Towns + 1 Watchtower yields the golden owned-tile count and the mirrored midline tiles are neutral; proptest over 200 random source edits asserts incremental recompute hash == full recompute hash; a Build outside own borders is rejected with `CommandRejected { reason: OutsideBorders }` and no state change.
- [x] `cargo run -p sim-cli --release -- bench --territory --sources 16 --edits 100` reports mean incremental recompute < 2 ms.

```bash
cargo test -p sim territory
cargo run -p sim-cli --release -- bench --territory --sources 16 --edits 100
```

The field, its unit tests, the 200-edit proptests (on the default map and on an odd-width field, with mirrored source pairs), the bench and the criterion bench `territory_recompute` landed first, before construction wired the field into `State` (see "M3a implementation notes" below). Measured 2026-10-10 on a 4-vCPU Linux cloud VM: mean 0.036 ms, p95 0.057 ms per edit for 16 sources and 100 edits (budget 2 ms; [../BUILD_TIMES.md](../BUILD_TIMES.md)).

On the even-width mirrored `plains_1v1` no tile is exactly equidistant from two mirrored centres (they straddle x = 63.5), so midline ties come only from the floored square root: two Towns at (48, 64) and (79, 64) with R = 20 own 1167 tiles each, and exactly 8 tiles tie, all neutral (`territory_mirrored_towns_tie_only_by_isqrt_floor_and_ties_go_neutral`). On an odd-width field the centre column is an exact tie wherever a kernel reaches it (`territory_odd_width_centre_column_is_neutral_wherever_reached`).

## M3a implementation notes

Since the Town loop (`SIM_VERSION` 6) the field is part of the hashed `State` and fills the `territory` sub-hash. Where it is more precise than the sections above:

| Topic | As implemented |
| --- | --- |
| Sources | Each Seat projects from the standard start. A founded Town or a Watchtower projects when it completes (`BuildingCompleted`, then `TerritoryChanged`), never while it is a site. The centre is the footprint's centre tile, `nw + ((w - 1) / 2, (h - 1) / 2)`. Since the M4a tech slice (`SIM_VERSION` 12) Statecraft edits every own Town's source: completing a tech or advancing an age runs `town::refresh_all`, which calls `change_source` when the radius from `tech::town_radius` (the tier's base radius plus the `TownBorderRadius` rows) changes, and a Town that completes after Statecraft projects the pushed radius at once. Since the M4a combat core slice (`SIM_VERSION` 13) a destroyed building's source is removed with `remove_source` (see the Watchtower row). Since the M5a annexation slice (`SIM_VERSION` 16) annexation edits a Town's source three ways, each one `change_source` that keeps its centre and radius: the timer's entry moves it to the neutral layer (`SourceOwner::Neutral`; `AnnexationStarted`, `TerritoryChanged`), a reset moves it back to the owner's layer (`AnnexationReset`), and the flip moves it to the new owner's layer before `town::refresh` re-derives its radius for the new owner (`TownAnnexed`). The flip also moves the source of every other complete building that changes hands with the Town and projects its own border (a kind with `border_radius_tiles` and without `stays_on_annexation`; none in the shipped data, where the Watchtower stays) to the new owner's layer with one `change_source` each, so no transferred kernel keeps stamping the old owner's layer (`annexation_flip_moves_a_transferred_border_source_to_the_new_owner`). `town::refresh` changes only a source's radius, so a Statecraft level researched mid-timer keeps the kernel on the neutral layer. While the neutral kernel is the unique strongest layer the tile is neutral, so the Town's tiles read `owner_at == None` during the timer except where another kernel wins (`annexation_during_the_timer_owner_at_is_neutral_and_train_is_rejected`), and the field equals a full recompute at the entry, a reset and the flip (`annexation_field_equals_full_recompute_at_entry_reset_and_flip`). |
| Town growth | Since gathering (`SIM_VERSION` 8) a Town that reaches tier 2 (its complete attributed buildings cover five distinct kinds, see [towns.md](towns.md), and, since the M4a tech slice, the owner has reached `town_growth_age`, the Masonry Age) edits its source with `change_source` to radius 24 plus the owner's Statecraft level and emits `TownTierChanged { town, tier }`, then `TerritoryChanged`. The tier is re-derived when an attributed building completes and, since the M4a tech slice, for every own complete Town when the owner completes a tech or advances an age (`town::refresh`, see "M4a implementation notes" in [towns.md](towns.md)). `territory_town_grows_to_tier_two_at_five_distinct_kinds` checks the new radius and that the field equals a full recompute of its sources. |
| Watchtower | `Build` of a Watchtower takes the same path as any kind, but so far tests complete one only through `DebugPlaceBuilding` (no test gathers the Ore for one). Since the M4a combat core slice a complete Watchtower at 0 HP is destroyed in step 5c (see "M4a implementation notes" in [combat.md](combat.md)): its source leaves the field with one `remove_source` (the strength layers lose its kernel and the owner grid is recomputed inside the kernel's rectangle, as for any source edit), the tick's destroyed footprints are unblocked with one generation bump, one `TerritoryChanged` is emitted, and the Town the tower was attributed to is re-derived and may fall back to tier 1, a `change_source` of its own. `combat_watchtower_destroyed_frees_footprint_border_and_tier` checks that the tower's tile is passable, its source is gone, the Town is back at tier 1 with radius 20 and the field equals a full recompute. A Town is never destroyed, so its source stays (a Town at 0 HP keeps its kernel, on the neutral layer while its annexation timer runs, M5a); a site has no source. |
| Golden counts at sim level | From the standard start on `plains_1v1` each Seat owns 1245 tiles. With a P0 Watchtower completed at (43, 64), the "2 Towns + 1 Watchtower" fixture (the two Seats plus the tower) gives P0 1248 and P1 1245, and columns 63 and 64 are neutral on every row (`territory_fixture_two_towns_and_a_watchtower_has_golden_owned_tiles_and_a_neutral_midline`), the same counts as the field-level `territory_fixture_two_towns_and_a_watchtower_golden_counts_field_level`. |
| Outside borders | `OutsideBorders` is the seventh `Build` check since the M4a tech slice inserted `AgeRequired` as the fifth (the order is in [towns.md](towns.md)); every footprint tile must pass `is_buildable`. `territory_build_outside_own_borders_is_rejected_without_state_change` compares every hash and sub-hash with a control sim whose command at the same `(player, seq)` is rejected for another reason, over the following ticks, and checks that the `rng` sub-hash is unchanged. |

## Open tuning questions

1. Distance metric: floored Euclidean (`isqrt`) is the M3a reading of "disc kernel". Chebyshev would be cheaper but square.
2. Watchtower as an own source of radius 2 versus adding 2 to the nearest Town's radius. The own-source model rewards placing towers at the frontier, which matches the AI plan; confirm.
3. Town radii 20 and 24 on a 128-tile map leave a neutral band between distant Towns. Is that the intended early-game feel, or should tier 1 be larger?
4. Should the G channel carry the owner's strength or the margin over the runner-up? Margin would make the isoline softer.
5. Resolved in M3b: the R channel is 0 neutral, else player index + 1 (`field::field_texels`; `owner_code` in `field.wgsl`).
6. Watchtower radius: R = 2 (`border_radius_tiles` in `buildings.ron`) claims a 3x3 block, about one tile of push, while [ai.md](ai.md) says the Watchtower pushes the border two tiles. The owner is to confirm R against that wording (R = 3 would claim a 5x5 block, about two tiles of push); the data keeps 2 until then.
