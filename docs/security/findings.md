# Security Findings

[Security](README.md) · Assessment: [asvs.md](asvs.md) · Threats: [threat-model.md](../threat-model.md)

Every finding of the phase 2 security review (step 2.16), with its
severity, what was done, and the evidence. The exit gate of phase 2 is **no
open high or critical finding**: none is open.

## Severity

| Severity | Meaning |
|---|---|
| Critical | Money can be created, moved or lost, or the ledger's history changed, by someone outside the system |
| High | Money or ledger data is at risk from an authenticated client, or the service can be made unavailable by anyone |
| Medium | A protection is weaker than the design says, or a likely mistake would make it so |
| Low | Defence in depth, a bounded loss, or a risk only an operator or insider can reach |

## Register

| ID | Severity | Finding | Found by | Status |
|---|---|---|---|---|
| F-01 | High | A caller with no valid credential could make the service do unbounded work: inventing a new credential ID for each token request passed the per-credential limit and cost one Argon2 hash each; every refused request also appended a row to the append-only, hash-chained audit log, under the chain head's lock | Code review | **Fixed** (#160, #161): failed logins share one bucket for the service, reserved before any lookup or hash; an optional limit per address; refusals of unauthenticated callers audited in summary ([ADR-0021](../adr/0021-unauthenticated-traffic-limits.md)). Test: `a_caller_without_a_secret_costs_bounded_work_and_audit_events` |
| F-02 | Medium | Only the token endpoint sent `Cache-Control: no-store`; no response sent `X-Content-Type-Options`, `X-Frame-Options` or a `Content-Security-Policy` | Code review against [owasp-review.md](../api/owasp-review.md) (API8) | **Fixed** (#162). Test: `every_answer_carries_the_security_headers` |
| F-03 | Medium | A refund naming another client's entry was answered differently from one naming no entry, so a client could learn which entry IDs exist (TM-05, API1); the missing case was an undocumented `404` | The API fuzzer, first run | **Fixed** (#164): both are `422 refunded_entry_not_found`. Test: `ac09_a_refund_cannot_probe_another_clients_entries` |
| F-04 | Medium | Dependency advisories were checked only when dependencies changed and weekly, by cargo-deny alone | Step plan | **Fixed** (#166): cargo-deny and cargo-audit on every push and pull request, and daily |
| F-05 | Medium | The service started with the development signing key published in the repository (`deploy/signing.dev.hex`), so a deployment copied from `compose.yaml` would sign receipts and checkpoints anyone could forge (TM-11) | Code review | **Fixed** (2.16): refused unless `MEZGEB_ALLOW_DEVELOPMENT_KEY=true`. Test: `the_development_key_is_refused_and_others_are_not` |
| F-06 | Low | RUSTSEC-2026-0459 (C SQLite CVE, medium) in `libsqlite3-sys` 0.37.0, which is in `Cargo.lock` only as an optional dependency of `sqlx` that nothing enables | cargo-audit | **Accepted**: never compiled (`cargo tree --all-features --target all` finds no path; cargo-deny, which checks the built graph, passes). Ignored in `.cargo/audit.toml` until a `sqlx` release allows the patched version |
| F-07 | Low | The offline merchant fee rounds down, so splitting one payment into *n* receipts saves at most *n* − 1 santim of fee | Abuse case AC-08 | **Accepted**: bounded and proven by `splitting_a_payment_saves_at_most_one_santim_of_fee`; each receipt needs a signed device counter inside a capped voucher |
| F-08 | Low | The trial balance and ageing reports return every matching account in one response, with no pagination; their size grows with the number of accounts | Code review (API4) | **Open**, step 3.01: operator-only (`reports:read`); paginate or stream when the performance work measures it |
| F-09 | Low | An audit event that cannot be written is logged as an error, and the request it records still succeeds (fail open) | Code review (ASVS V16) | **Open**, step 3.09: the log line `an audit event was not recorded` gets an alert. Refusing work when the audit log is down would make it a way to stop payments |
| F-10 | Low | Rate limits are kept in each process: several instances each allow the full rate | Known since step 2.08 | **Open**, step 3.03 (shared limits with horizontal scaling) |
| F-11 | Deployment | The service speaks plain HTTP; TLS, HSTS and the database's TLS are the deployment's | Code review (ASVS V12) | **Open**, deployment requirement: written in [asvs.md](asvs.md#v12-secure-communication); checked in the readiness review (step 4.02) |
| F-12 | Deployment | The receipt and offline issuer keys are read from files | Known (TM-14) | **Open**, step 3.12 (a key service or HSM) |

## Not findings

- **The fuzzer's dropped connections.** When the service refuses a body over
  the limit before reading it, it answers and closes the connection; a
  client still sending, or one that reuses that connection, may see the
  connection drop. That is ordinary HTTP behaviour. The fuzzer opens a new
  connection for each case (#165 and its follow-up).
- **Formatted SQL.** The engine builds some SQL with `format!` under
  `AssertSqlSafe`. Every value interpolated is a column list constant, a
  fixed fragment such as `" FOR UPDATE"`, an ID read from the database, or
  a database name in a test tool; no request data reaches SQL except as a
  bound parameter.
