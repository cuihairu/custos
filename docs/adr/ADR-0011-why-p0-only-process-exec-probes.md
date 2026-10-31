# ADR-0011: Why P0 Ships Only Process and Exec Probes

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/probe-survey.md §26.11, roadmap P0

## Background

Probe types are unlimited (survey §26 lists 20+), but P0 must prove the architecture: probe → normalization → diagnosis → dependency check → recovery → independent verification → persistence → audit, all correct. Spreading across 20 probe types makes each shallow.

## Decision

P0 implements only Process (pid file + liveness) and Exec (controlled command + timeout + output cap), proving the full loop and acceptance with them; other probe types are added in the survey §26.11 recommended order, each with independent tests, cross-platform semantics (Linux/Windows), and normalization rules (connection refused ≠ timeout, Unreachable ≠ Unhealthy), one tick per probe.

## Alternatives

- **Implement all probe types at once**: spread too thin; normalization rules never polished per-type — false positives and negatives only surface in real deployments.
- **Only the Exec universal probe**: loses normalization (command output is not structured health state), cannot support verification and dependency conditions.

## Pros and Cons

Pros: loop quality first; each probe type has independent semantic boundaries and tests (now 13 high-priority items done, 174/174 green); clear extension template.
Cons: fewer monitorable objects early (users use Exec as fallback).

## Follow-up Impact

- probe-survey §26.11 order is the roadmap; each probe type is one module (icmp.rs/database.rs…).
- New probes must provide: normalization rules, timeout/permission semantics, Windows behavior (cfg or cross-check).
