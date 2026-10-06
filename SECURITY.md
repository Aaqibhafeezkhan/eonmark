# Security policy

This document says how to report a security problem in Eonmark and what happens after you do. Eonmark is a single-player game with no network code before v0.2, so the attack surface is small, but file parsing (replays, RON data, settings) and the build and release tooling still deserve care.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

1. Preferred: GitHub private vulnerability reporting on this repository. Open the Security tab and choose "Report a vulnerability", or go to https://github.com/tonianev/eonmark/security/advisories/new.
2. Fallback: email tonianev@gmail.com with "Eonmark security" in the subject.

Include the commit or release version, your macOS version and chip, the steps to reproduce, and, when relevant, the replay or data file that triggers the problem.

## What to expect

| Step | Commitment |
|---|---|
| Acknowledgement | Within 7 days of your report. |
| Assessment | You hear what was found and what will be done, or why the report is declined. |
| Fix | Ships in the next tagged release. A severe problem gets a point release. |
| Credit | In the release notes, if you want it. |

## Scope

In scope:

- The `eonmark` binary and the `Eonmark.app` bundle published on GitHub Releases (from v0.1.0, M8).
- The `sim-cli` tool.
- The scripts under `scripts/` and the GitHub Actions workflows under `.github/workflows/`.

Out of scope:

- Vulnerabilities in third-party crates. Report them upstream; `cargo deny` checks advisories in CI.
- Problems that need a modified binary or physical access to the machine.
- The GitHub platform itself.

## Supported versions

Pre-alpha. Fixes land on `main`. From v0.1.0 only the latest tagged release receives fixes.

## No bounty

There is no bug bounty program.

## Verifying a download

Releases are ad-hoc signed and not notarized. Check the zip against the `SHA256SUMS` file on the release page as described in [README.md](README.md#installing-a-release).
