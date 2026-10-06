# data/ai/

This directory will hold the scripted opponent's data: build orders,
personalities and the difficulty table. It is empty in M0 on purpose. The
`ai` crate currently ships two bots that need no data, `ai::Passive` (never
acts) and `ai::Scripted` (replays a fixed command list for `sim-cli
selftest`). The data-driven bot arrives in M5b and its tuning in M6.

## Planned files (M5b, M6)

| Path | Contents |
|------|----------|
| `build_orders/*.ron` | Ordered build and research steps per personality (rush, boom, tower) |
| `difficulty.ron` | Easy, Standard, Hard rows: bot income interval and aggression flag, documented as a cheat |
| `personalities.ron` | Weights for expansion, towers and attack timing |

The schema will live in `crates/rules` next to the other loaders, use
`deny_unknown_fields`, and be validated by `sim-cli data-check` like
everything else under [`data/`](../README.md). Until then `Rules::load`
ignores this directory.

Rule for every bot: it is a deterministic function of `sim::SimView` and the
`Pcg32` stream inside the sim. No clocks, no floats, no randomness of its
own; `crates/ai/clippy.toml` enforces this.
