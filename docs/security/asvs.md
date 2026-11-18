# ASVS Level 2 Self-Assessment

[Security](README.md) · Findings: [findings.md](findings.md) · API review: [owasp-review.md](../api/owasp-review.md) · Threats: [threat-model.md](../threat-model.md)

| | |
|---|---|
| Standard | OWASP Application Security Verification Standard 5.0, Level 2 |
| Scope | `mezgeb-api` (the REST service) with the engine and database behind it, at commit `main` after step 2.16; `mezgeb-cli` where it handles secrets and keys |
| Out of scope | The client platforms' own applications; the deployment (proxy, TLS, network, hosts), which is listed as requirements |
| Date | 2026-11-18 |
| Result | **No open high or critical finding.** Five findings fixed in this step, two accepted, five open with a step that owns them ([findings.md](findings.md)) |

Mezgeb is a server-to-server ledger. It has no browser front end, no
human users of its own, no sessions, no file uploads and no redirects, so
large parts of ASVS do not apply; each is listed with the reason. Where a
requirement is met, the evidence is a test or a file, not a statement of
intent.

**Status:** **Met**, **Partly** (a finding says what is missing), **N/A**
(does not apply, and why), **Deployment** (outside the service; a
requirement on whoever runs it).

## V1 Encoding and sanitization

| Area | Status | Evidence |
|---|---|---|
| Injection into SQL | Met | Every query is parameterised through SQLx. The SQL built with `format!` (under `AssertSqlSafe`) interpolates only column lists, fixed fragments and IDs read from the database; checked one by one in this review ([findings.md](findings.md#not-findings)) |
| Output encoding | Met | Responses are JSON written by `serde_json` from typed structs (`crates/api/src/wire/`); errors are RFC 9457 Problem Details built in one place (`problem/`) |
| Log injection | Met | Logs are structured JSON (`tracing`); a caller's `X-Request-Id` is kept only if it is a UUID (`a_callers_request_id_is_kept_only_if_it_is_a_uuid`); the access log records the route pattern, never the raw path |

## V2 Validation and business logic

| Area | Status | Evidence |
|---|---|---|
| Input validation | Met | Every JSON request type refuses unknown fields (`deny_unknown_fields`, 35 of 35); amounts are strict decimal strings in minor units (`amounts_are_strict_decimal_strings`); 33 malformed amounts are refused without a balance moving (AC-01); random mutated requests never get a `5xx` (`tests/fuzz.rs`) |
| Business rules in the right place | Met | Clients post only through templates; the engine and the database enforce the invariants a second and third time ([invariants.md](../invariants.md)) |
| Limits and overflow | Met | Checked arithmetic on `i64` minor units ([ADR-0005](../adr/0005-money-as-i64-minor-units.md)); a balance at `i64::MAX` refuses one santim more (AC-02) |
| Replay and idempotency | Met | Every posting takes an `Idempotency-Key`, per client; the same key with another body is `409` (AC-03; [ADR-0008](../adr/0008-idempotency-in-posting-transaction.md)) |
| Concurrency abuse | Met | Concurrent overspending, refunds past the cap, ends of one hold, vouchers per device and receipt uploads each have one correct outcome (AC-04 to AC-07), and the ledger's full verification passes after each |
| Anti-automation | Met | Per-client rate limits; failed logins limited per credential and for the whole service; optional limit per address ([ADR-0021](../adr/0021-unauthenticated-traffic-limits.md)) |
| Rounding | Met, one risk accepted | The only fee the service computes rounds down; splitting saves at most a santim per split (F-07) |

## V3 Web frontend security

| Area | Status | Evidence |
|---|---|---|
| Browser protections | Met where they apply | No browser client and no CORS (server to server). Every response is `no-store`, `nosniff`, `DENY` framing and a `Content-Security-Policy` that allows nothing (F-02, `every_answer_carries_the_security_headers`) |
| Cookies, CSRF, client-side storage | N/A | No cookies; bearer tokens in a header only |

## V4 API and web service

| Area | Status | Evidence |
|---|---|---|
| Content types | Met | A JSON body needs `Content-Type: application/json`; every answer's type and body are checked against the contract by every API test and the fuzzer |
| Request size and time | Met | `MEZGEB_BODY_LIMIT_BYTES` (64 KiB) and `MEZGEB_REQUEST_TIMEOUT_MS`; a timeout never cancels a write part-way (`a_client_timeout_then_a_retry_gives_one_entry`) |
| Inventory | Met | One versioned contract; the service serves exactly its paths (`tests/openapi.rs`; unknown routes are `404`) |
| Response size | Partly | Lists are paginated (1–100); two operator reports are not (F-08) |

## V5 File handling

| Area | Status | Evidence |
|---|---|---|
| Uploads through the API | N/A | The API takes no files |
| Bank statements | Met | Imported by an operator with `mezgeb-cli recon import`, parsed as data with explicit field mappings; a line seen before is skipped, a changed one is a conflict ([statement-format.md](../reconciliation/statement-format.md)) |

## V6 Authentication

| Area | Status | Evidence |
|---|---|---|
| Credentials | Met | OAuth 2.0 client credentials; secrets are 256 random bits, shown once, stored as Argon2id (m=19456, t=2, p=1) ([ADR-0017](../adr/0017-authentication-and-authorization.md)) |
| Brute force | Met | Per credential (2/s, burst 10), for the whole service (20 failed/s, burst 100), optional per address; reserved before hashing (F-01) |
| Enumeration | Met | An unknown credential is checked against a dummy hash, so it takes as long as a wrong secret, and both are `invalid_client` |
| Default credentials | Met, with a note | No credential exists until an administrator issues one. `deploy/clients.dev.toml` holds development secrets in the clear, loaded only by an explicit `mezgeb-cli client seed` of that file |
| Passwords, MFA, recovery, human login | N/A | No human users; the client platforms authenticate their own users |

## V7 Session management

| Area | Status | Evidence |
|---|---|---|
| Tokens as sessions | Met | Opaque random tokens, stored as SHA-256, living 15 minutes; revoking a credential or disabling a client ends its tokens at once (`revoked_credentials_and_disabled_clients_stop_at_once`) |
| Cookies, idle timeout, logout | N/A | No cookies and no interactive sessions |

## V8 Authorization

| Area | Status | Evidence |
|---|---|---|
| Default deny, function level | Met | Each operation checks its scope before reading the request; all 63 operations × 6 roles, and without a token, match the permission matrix exactly (`every_role_gets_exactly_what_the_matrix_allows`) |
| Object level | Met | Another client's object is answered as nothing, `404` or `422` alike (`a_client_cannot_touch_another_clients_accounts`, AC-09; F-03 fixed) |
| Separation of duties | Met | Adjustments need two different people (`maker_is_checker`; TM-12) |

## V9 Self-contained tokens

| Area | Status | Evidence |
|---|---|---|
| JWTs and signed tokens | N/A | Access tokens are opaque and looked up on every request ([ADR-0017](../adr/0017-authentication-and-authorization.md)) |

## V10 OAuth and OIDC

| Area | Status | Evidence |
|---|---|---|
| Token endpoint | Met | Client authentication by HTTP Basic only (secrets in the body are refused); answers `no-store`; a token's scope can only narrow (`scopes_can_only_narrow`); RFC 6749 errors |
| Redirects, authorization codes, PKCE, OIDC | N/A | Client credentials only; no user-facing flow |

## V11 Cryptography

| Area | Status | Evidence |
|---|---|---|
| Algorithms and libraries | Met | Ed25519 receipts and checkpoints, P-256 device and voucher signatures, SHA-256 chains, Argon2id, from maintained crates; no cryptography of our own |
| Randomness | Met | Secrets, tokens and keys from the operating system (`getrandom`) |
| Key management | Partly | Keys are files today (F-12, step 3.12); the published development key is refused (F-05) |

## V12 Secure communication

| Area | Status | Evidence |
|---|---|---|
| TLS to clients | Deployment | The service speaks plain HTTP. **Requirement:** TLS 1.2 or later terminated by a proxy the operator controls, HSTS set there, and the service unreachable except through it (F-11) |
| TLS to the database | Deployment | `MEZGEB_DATABASE_URL` carries `sslmode`. **Requirement:** `sslmode=verify-full` wherever the database is not on the same host; the database unreachable from client networks (TM-08) |

## V13 Configuration

| Area | Status | Evidence |
|---|---|---|
| Configuration and secrets | Met | Configuration from the environment ([`config.rs`](../../crates/api/src/config.rs)); keys from files, never logged (`Signer`'s `Debug` prints no key); secrets never in URLs or bodies |
| Least privilege in the database | Met | Separate roles for migration, administration, the service and verification; history is append-only by trigger and grant ([db-schema.md](../db-schema.md)) |
| Dependencies | Met | `Cargo.lock` committed and the image built `--locked`; cargo-deny and cargo-audit on every change and daily (F-04, F-06) |
| No debug features | Met | No debug or admin routes outside the contract; errors carry no stack traces |

## V14 Data protection

| Area | Status | Evidence |
|---|---|---|
| Personal data | Met | The ledger holds no names or national IDs, only references chosen by clients (TM-16) |
| Caching and exposure | Met | Every response `no-store` (F-02); responses built from typed structs, never from rows |
| Reads across clients | Met | Only `accounts:read_all` reads across clients, and every such read is an audit event |

## V15 Secure coding and architecture

| Area | Status | Evidence |
|---|---|---|
| Memory safety | Met | `unsafe_code = "forbid"` and `unwrap_used = "deny"` for the workspace |
| Architecture | Met | Three layers: the client decides, templates book, the engine posts ([ADR-0002](../adr/0002-three-layer-separation.md)); a separate service with its own database ([ADR-0001](../adr/0001-separate-service-own-database.md)) |
| Concurrency | Met | Ordered row locks and one transaction per posting ([ADR-0007](../adr/0007-read-committed-ordered-row-locks.md)); the race tests and the simulators |

## V16 Security logging and error handling

| Area | Status | Evidence |
|---|---|---|
| What is logged | Met | One access log line per request (no bodies, tokens or amounts); an audit event for every change, refusal and read across clients, hash-chained ([ADR-0018](../adr/0018-tamper-evidence.md), `every_api_mutation_produces_an_audit_event`) |
| Log protection | Met | The audit chain is append-only and verified (`mezgeb-cli verify --chain`); unauthenticated refusals summarised so they cannot flood it (F-01) |
| Errors | Met | A storage error's message is logged and the client gets `internal_error` and the request ID |
| Failure of logging | Partly | A lost audit event is logged, not alerted (F-09, step 3.09) |

## V17 WebRTC

| Area | Status | Evidence |
|---|---|---|
| All | N/A | No WebRTC |

## How this was done

The code was read against each chapter: authentication, rate limits,
middleware, error mapping, configuration, every formatted SQL statement
and the request types. Then it was attacked: the abuse suite
(`crates/api/tests/abuse.rs`, AC-01 to AC-09), the API fuzzer
(`crates/api/tests/fuzz.rs`, 24,000 cases over eight seeds, and 50,000 a
night), the authorization matrix re-run, cargo-deny and cargo-audit. What
was found is in [findings.md](findings.md).
