# ADR-0003: Why JSON-RPC over stdio

- Status: Accepted
- Date: 2026-10-31
- Related: docs/guide/agent-protocol.md, ADR-0002

## Background

The core and plugin subprocesses need a request/response channel for handshake, capability declaration, probe calls, and recovery actions. It must be zero-barrier for plugin authors and must not widen the attack surface.

## Decision

JSON-RPC 2.0 over stdio, newline-delimited (one JSON message per line): stdout carries **only** protocol messages; stderr carries **only** diagnostic logging (never parsed as protocol); requests carry an id and timeout; incomplete lines, malformed JSON, and unknown methods return a clear error instead of crashing; stdout lines have a size cap — over-limit lines are skipped, never fatal; stderr has a dedicated continuously-draining thread with capped tail retention.

## Alternatives

- **gRPC/protobuf**: strong typing and streaming are good, but code generation toolchains raise the plugin author's barrier; introduces an HTTP/2 stack — both complexity and attack surface grow.
- **HTTP on localhost**: exposes a port (other local processes can access it); lifecycle binding is complex.
- **Custom binary protocol**: self-maintained parser, large crash surface, hard to debug.

## Pros and Cons

Pros: any language can speak it in three lines; no ports, no network surface; the stdout/stderr semantic boundary makes "protocol polluted by logs" observable and preventable (cap + skip policy).
Cons: throughput lower than binary protocols (more than enough for alerting volume); the line protocol requires strict stdout discipline from plugins — so the host must also defensively handle undisciplined plugins.

## Follow-up Impact

- session.rs: read loop, deadline, cancellation, output cap, dying-words (crashed session's stderr tail).
- Outbound requests (host → plugin direction) have a size cap to prevent the host from being tricked into filling a pipe.
