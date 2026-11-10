# 0009. The API is REST over HTTP, built with Axum

[All ADRs](README.md)

| | |
|---|---|
| Status | Accepted |
| Date | 2026-11-10 |
| Answers | [Architecture §2](../architecture.md#2-stack-evaluation) |
| Implemented in | 1.02 (remove `proto/`), 2.06–2.07 (REST API design and implementation), 2.16 (security review) |

## Context

The scaffold contains a draft gRPC API in `proto/mezgeb.proto`. Mezgeb is
meant to be adopted by many platforms, reviewed by auditors, and tested with
widely available tools. The API style decides how easy all of that is.

## Decision

The public API is **REST over HTTPS with JSON**, built with **Axum**.

- Errors use **RFC 9457 Problem Details**, with stable, enumerated error
  codes.
- Amounts are sent as decimal strings with an explicit currency (for
  example, `{"amount": "39.20", "currency": "ETB"}`), converted to minor
  units at the boundary (ADR-0005).
- Every write takes an `Idempotency-Key` header (ADR-0008).
- The API is versioned in the path (`/v1/...`) and described by an OpenAPI
  document.
- The `proto/` directory is deleted.

## Alternatives considered

| Option | For | Against |
|---|---|---|
| **REST + JSON with Axum (chosen)** | Understood by every team and tool (curl, Postman, browsers, API gateways). Easy to read in logs and audits. Axum is the mainstream Rust choice, built on Tokio and Tower. | More bytes per call than binary formats. Schemas live in OpenAPI rather than the type system. |
| gRPC with Protocol Buffers | Compact, fast, strongly typed contracts, streaming. | A second toolchain for every client. Harder to inspect and test by hand. Its speed benefits do not matter at the expected scale. |
| GraphQL | Flexible reads. | Poor fit for idempotent, audited writes. Harder to rate-limit and authorise per operation. |

## Consequences

**Positive:**

- Any client language can integrate with an HTTP library.
- The Postman tooling already used for API work applies directly.

**Negative:**

- The API contract must be kept in step with the code. An OpenAPI document,
  checked in CI, prevents drift.
- If a streaming need appears later (for example, a live feed of entries),
  it is added as a separate interface, not by changing this decision.

**Makes out of date:** spec §9 (gRPC) and `proto/mezgeb.proto`. Both are
changed in step 1.02.
