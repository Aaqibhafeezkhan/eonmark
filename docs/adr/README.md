# Architecture decision records

This directory holds Eonmark's architecture decision records. An ADR is a short, numbered document that captures one decision that is expensive to reverse: what was decided, why, what else was considered and what follows from it. It is written when the decision is made, not after, and it is never edited to change the decision; a new ADR supersedes it instead. Scope changes requested by the owner go through [ROADMAP.md](../ROADMAP.md) and an ADR. Everything else (bug fixes, tuning, features inside the agreed scope) does not need one.

## Index

| Number | Title | Status | Date |
|---|---|---|---|
| [0001](0001-license.md) | Licensing of code, data and assets | Accepted | 2026-10-05 |
| [0002](0002-bevy-0-20-migration.md) | Migration to Bevy 0.20 | Proposed | |

## Statuses

| Status | Meaning |
|---|---|
| Proposed | Written, not yet in force; may have unmet trigger conditions |
| Accepted | In force |
| Superseded by NNNN | Replaced; the text stays for history |
| Rejected | Considered and declined; kept so the question is not reopened without new facts |

## When to write one

Write an ADR for a decision that changes a public boundary or is costly to undo: a licence, an engine or major dependency version, the simulation model, the data format, a networking approach, a change to governance invariants. Do not write one for a tuning change, a refactor inside a crate or a new RON field.

## Naming

`NNNN-short-kebab-title.md`, four digits, zero-padded, in order of creation. Update the index table above in the same PR.

## Template

```markdown
# NNNN: Title

Status: Proposed | Accepted | Superseded by NNNN | Rejected
Date: YYYY-MM-DD (date the status was last changed)

## Context

What is the situation and what forces are at play. Facts, with dates and sources where they matter. Short.

## Decision

What was decided, in the imperative. One paragraph or a short list.

## Alternatives considered

| Alternative | Why not |
|---|---|
| | |

## Consequences

What becomes easier, what becomes harder, what must now be done. Include CI or doc changes that enforce the decision.
```
