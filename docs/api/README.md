# Mezgeb Ledger API (v1-draft)

[Documentation](../README.md) · Contract: [`openapi/mezgeb.v1.yaml`](../../openapi/mezgeb.v1.yaml) · Errors: [errors.md](errors.md) · Security review: [owasp-review.md](owasp-review.md) · Decision: [ADR-0009](../adr/0009-rest-api-with-axum.md)

The public API is designed before it is built (step 2.06). The contract,
an OpenAPI 3.1 document, is the promise to every client; step 2.07 built
the service, `mezgeb-api`, against it, and the contract tests keep the two
in step (see [The service](#the-service)).

**Status: v1-draft.** It is frozen for implementation: a change before the
v1 release is a reviewed change to the contract first, never a quiet change
in code. After the v1 release, only backward-compatible changes are made to
`/v1` (see [Compatibility](#compatibility)).

| File | What it is |
|---|---|
| [`openapi/mezgeb.v1.yaml`](../../openapi/mezgeb.v1.yaml) | The contract: 41 paths, the schemas, an example of every request and of every error |
| [errors.md](errors.md) | The error catalogue: every `code`, its HTTP status and the rule behind it |
| [owasp-review.md](owasp-review.md) | Every endpoint group reviewed against the OWASP API Security Top 10 (2023) |
| [`postman/collections/mezgeb-api-v1`](../../postman/collections/mezgeb-api-v1) | A Postman collection generated from the contract, with every example |
| [`postman/collections/mezgeb-api-smoke`](../../postman/collections/mezgeb-api-smoke) | A scripted Postman run against a live service, run by CI against the container |

## Resources

| Group | Endpoints |
|---|---|
| Accounts | Open, get, list or find by reference; balance at a point in time; history (general ledger); freeze, unfreeze or close |
| Templates | List the posting templates, get one |
| Operations | `POST /v1/operations/{template}`, one per posting template: payments, deposits and withdrawals, refunds and disputes |
| Entries | Get an entry; find one by idempotency key |
| Corrections | Reverse an entry; request, approve or reject an adjustment |
| Holds | Place, get, capture, release |
| Periods | List, get the open one, get one, close |
| Reports | Trial balance (now, at a period's end, or at a time); ageing of open items |
| Offline | Register and revoke devices, issue vouchers, list the issuer keys (step 2.11); upload receipts and list a voucher's receipts (step 2.12); shortfalls and, for operators, disputes (step 2.13). See the [offline runbook](../runbooks/offline.md) |
| Reconciliation | Compare a client's cached balances with the ledger; for operators and auditors, list breaks and runs; for operators, resolve a money break with its suspense entry or explain another (step 2.15). See the [reconciliation runbook](../runbooks/reconciliation.md) |

## Conventions

**Money.** An amount is a decimal string with its currency,
`{"amount": "39.20", "currency": "ETB"}`, never a JSON number. It may not
have more decimal places than the currency (ETB: 2). The service converts it
to minor units at the boundary (ADR-0005); the ledger never rounds.

**Accounts in requests.** A request names an account by its ID or by its
chart reference (`"2010:cust-abebe"`, `"4010"`). References make
integrations readable and let a client bind system accounts such as fee
revenue without looking up their IDs.

**Clients post through templates.** Each posting template has its own
endpoint, with a typed body: the account for each role, each amount, and the
metadata it requires. A client never sends lines. The one endpoint that
takes arbitrary lines is the operator-only adjustment, which needs a second
person to approve it (INV-25).

**Idempotency.** Every write that posts an entry or places a hold takes an
`Idempotency-Key` header (ADR-0008). The same key with the same request
returns the original result with `200` and `Idempotent-Replayed: true`. The
same key with a different request is `409 idempotency_key_reused`. Keys are
per client. A client that lost a response looks the entry up with
`GET /v1/entries/by-idempotency-key/{key}`. Closing a period is idempotent
by its `next_label` instead.

**Errors.** RFC 9457 Problem Details (`application/problem+json`), with a
stable `code` and, for posting and template rules, the numeric `rule`. The
[catalogue](errors.md) lists every code.

**Pagination.** Lists take `limit` (1–100, default 50) and an opaque
`cursor`, and answer with `next_cursor` (`null` on the last page).

**Times.** RFC 3339, in UTC, with microseconds. An entry has two: the time it
was recorded (`recorded_at`, its posting date, which decides its period and
every report), and the optional `event_time`, when the event happened.

**Authentication.** OAuth 2.0 client credentials (ADR-0017): a client
exchanges its credential's ID and secret at `POST /oauth/token` for a
15-minute token, and every operation needs one scope. A client sees and
binds only its own accounts. See
[Authentication and authorization](#authentication-and-authorization).

**Batches.** Not in v1. Each request posts one entry, which keeps every
entry atomic and idempotent on its own. A batch endpoint would be a later,
additive change if clients need one.

**Receipts.** Every entry has a `receipt`: its signed link in the ledger's
hash chain (spec §7.2, ADR-0018). Verify it with the public key from
`GET /v1/keys` alone; keep it as proof the ledger recorded the entry. See
[Tamper evidence](#tamper-evidence).

## Compatibility

Within `/v1`, the API only grows:

- New endpoints, new optional request fields, new response fields, new error
  codes and new template versions may be added. Clients ignore response
  fields they do not know.
- Nothing is removed or renamed, no field changes type or meaning, no
  optional request field becomes required, and no error `code` changes.
- A posting template version, once released, never changes (step 2.01). A
  changed booking is a new version with a new endpoint.

A breaking change is a new version of the API (`/v2`), with both versions
served during a published migration window.

## The service

`mezgeb-api` (crate `crates/api`) serves the contract with Axum. Each
handler checks the caller's scope, parses the body strictly, and calls the
posting templates and the engine; it never builds lines itself.

```sh
docker compose --profile api up -d --build   # PostgreSQL, migrations, the service on :8080
# A token for the development wallet credential (deploy/clients.dev.toml):
TOKEN=$(curl -s -u 0192f3c4-0000-7000-8000-0000000c0001:dev-wallet-secret-0001     -d grant_type=client_credentials http://localhost:8080/oauth/token | jq -r .access_token)
curl -s http://localhost:8080/v1/templates -H "Authorization: Bearer $TOKEN"
```

Or from a checkout, against the compose PostgreSQL:

```sh
cargo run -p mezgeb-cli -- migrate     --database-url postgres://mezgeb_owner:mezgeb_owner@localhost:55432/mezgeb
cargo run -p mezgeb-cli -- client seed deploy/clients.dev.toml     --database-url postgres://mezgeb_admin:mezgeb_admin@localhost:55432/mezgeb
MEZGEB_DATABASE_URL=postgres://mezgeb_app:mezgeb_app@localhost:55432/mezgeb \
MEZGEB_SIGNING_KEY_FILE=deploy/signing.dev.hex cargo run -p mezgeb-api
```

The configuration is read from the environment; every variable is listed in
[`crates/api/src/config.rs`](../../crates/api/src/config.rs). The service
connects as `mezgeb_app` and never migrates: `mezgeb-cli migrate` does that,
as `mezgeb_owner`, before a new version starts.

**What every request goes through.**

| Layer | Behaviour |
|---|---|
| Request ID | A UUIDv7 per request, or the caller's `X-Request-Id` if it is a UUID. Returned in `X-Request-Id`, and the `instance` of every problem (`urn:uuid:<id>`) |
| Body limit | `MEZGEB_BODY_LIMIT_BYTES`, 64 KiB by default. A larger body is `400 invalid_request` |
| Security headers | Every response, errors and unknown routes included: `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Content-Security-Policy: default-src 'none'; frame-ancestors 'none'`. HSTS is set by whatever terminates TLS in front of the service |
| Failed logins | 20 a second in the whole service, in bursts of 100, reserved before any secret is hashed; over it, `429 slow_down` ([ADR-0021](../adr/0021-unauthenticated-traffic-limits.md)) |
| Failures per address | Off by default. `MEZGEB_ADDRESS_FAILURE_LIMIT` failed logins and refused tokens a second per source address (the TCP peer, or with `MEZGEB_CLIENT_ADDRESS=x-forwarded-for` the last `X-Forwarded-For` entry); over it, `429` before any lookup |
| Refusals in the audit log | A refusal of an unauthenticated caller is its own audit event only if it is the first for its actor in the minute, and at most 60 a minute; the rest are counted in one `refusals.suppressed` event a minute |
| Strict bodies | Unknown fields, JSON numbers for amounts, more decimal places than the currency, and unknown currencies are `400 invalid_request`, with a JSON Pointer to the field in `errors` |
| Timeout | After `MEZGEB_REQUEST_TIMEOUT_MS` (10 s), the client gets `503 busy` with `Retry-After`. **The write is not cancelled:** it runs in its own task and commits or fails on its own, so a retry with the same key returns its entry |
| Access log | One JSON line per request: method, route pattern (never the path with its IDs), status, latency, request ID and client. No bodies, tokens, amounts, account references or idempotency keys |
| Errors | Every error is Problem Details. An unexpected error is logged with its cause and answered with `500 internal_error`, which says nothing about the cause |
| Shutdown | On SIGTERM or Ctrl-C, the service stops accepting connections, finishes the requests under way, then waits for every write task before exiting |

**Replays.** A repeated write with the same key answers `200` with
`Idempotent-Replayed: true` and the original result; opening an account
too. Requesting an adjustment answers `202` both times, with the replay
header the second time.

### Authentication and authorization

Designed in [ADR-0017](../adr/0017-authentication-and-authorization.md).

**Clients and credentials.** A *client* is a platform: it owns the accounts
it opens, its idempotency keys and its rate limit. A *credential* is one key
for one actor of a client (a service, or a person on the operator's staff)
with one or more roles. The operator manages them with `mezgeb-cli client`
and `mezgeb-cli credential`, as `mezgeb_admin`
([runbook](../runbooks/clients-and-credentials.md)); the service itself can
never create one.

**Tokens.** `POST /oauth/token`, with HTTP Basic (the credential's ID and
secret) and `grant_type=client_credentials`, answers a random token that
lives `MEZGEB_TOKEN_TTL_SECONDS` (900). It carries the scopes the roles
grant, or the subset asked for with `scope`. The secret is kept only as an
Argon2id hash, the token only as its SHA-256; revoking the credential or
disabling the client ends its tokens at once. Token errors follow RFC 6749
(`{"error": "invalid_client"}`).

**Roles.** The permission matrix is
[`crates/api/permissions.toml`](../../crates/api/permissions.toml), built
into the service:

| Role | May |
|---|---|
| `poster` | open its accounts, post through the templates it lists, place holds |
| `reader` | read its own accounts, entries and holds, and the periods |
| `operator` | reversals, adjustment requests, the period close; read every client |
| `approver` | approve or reject adjustment requests |
| `auditor` | read everything, reports; change nothing |
| `admin` | open system accounts; freeze, unfreeze or close accounts |

One credential is never both `operator` and `approver` (the engine, the
matrix and the database each refuse it), and the approver must be a
different person from the requester (INV-25).

**Authorization comes first.** Each handler's first extractor is
`Auth<Scope>`: a caller without the scope gets `403 forbidden` before its
query, body or idempotency key is read; then the template, for an
operation.

**Ownership.** A client reads only accounts it opened, and binds in a
posting or a hold only its own accounts or system accounts (no sub-account,
such as `1010`). It reads entries it posted or that touch its accounts, and
holds on its accounts. Anything else is answered exactly as if it did not
exist (`404`, or `422 bound_account_not_found` in a body). Only
`accounts:read_all` (operator, auditor) reads across clients.

**Rate limits.** Each client has a token bucket (`rate_per_second`,
`burst`); over it, `429 rate_limited` with `Retry-After`. The token endpoint
allows 2 attempts per second per credential, in bursts of 10, before any
hashing (`429 slow_down`). The buckets are per process.

### Tamper evidence

[ADR-0018](../adr/0018-tamper-evidence.md); the operator's procedure is the
[tamper evidence runbook](../runbooks/tamper-evidence.md).

- **Receipts.** Every posted entry returns its receipt, signed with the key
  in `MEZGEB_SIGNING_KEY_FILE` (required). `GET /v1/keys` lists the public
  keys.
- **Checkpoints.** Each period close signs a checkpoint of the chains;
  `GET /v1/checkpoints` lists them, each with a one-line text form. A client
  that keeps those lines can later prove the ledger's history was not
  rewritten.
- **Audit events.** Every request that changes something, every `401` and
  `403`, and every read by a caller with `accounts:read_all` is recorded:
  actor, client, action, target, request ID, outcome and details, never a
  body or a secret. `GET /v1/audit-events` needs `audit:read` (auditors).

### Testing it

```sh
docker compose up -d
export MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres
cargo test -p mezgeb-api
```

| Test | What it proves |
|---|---|
| `tests/contract/` | One scenario calls all 46 operations of the contract, the token endpoint included, and checks every response (status, headers, body) against the contract's schema for it; it fails if an operation is never called |
| `tests/authz.rs` | Every operation × every role, generated from the contract and the permission matrix: allowed exactly when the matrix says, `403` otherwise, `401` without a token. A client cannot touch another client's accounts, entries or holds; a revoked credential or a disabled client stops at once |
| `tests/audit.rs` | Every API mutation produces an audit event (generated from the contract), with its outcome and details; receipts verify with the published key; a period closed through the API gets a checkpoint, and the chains verify against a client's copy |
| `tests/threats.rs` | The threat model's attacks through the API: a compromised client minting money (TM-07), an insider reopening a period (TM-13) |
| `tests/http.rs` | A client timeout followed by a retry gives one entry; bad, negative, float and oversized amounts and bodies are refused with their codes; a graceful shutdown under load loses no acknowledged request; a missing scope is refused before the request is read; rate limits; failed logins and refused tokens bounded per service and per address, refusals audited in summary (ADR-0021); security headers on every response |
| `tests/abuse.rs` | Business-logic abuse (step 2.16), AC-01 to AC-09: malformed and overflowing amounts, replays and keys per client, concurrent refunds past the cap, concurrent overspending, concurrent ends of one hold, concurrent vouchers and receipt uploads, a refund probing another client's entries; each ends with full verification |
| `tests/fuzz.rs` | Random requests (step 2.16): the contract's request examples, mutated with hostile values, missing and extra fields, IDs the service returned, bodies that are not JSON, strange path parameters, queries, keys and tokens, eight at a time. No `5xx`, nothing the contract does not document, and full verification afterwards. `MEZGEB_FUZZ_SEED` and `MEZGEB_FUZZ_CASES` (1,500 by default) choose the run |
| `tests/openapi.rs` | The contract itself, against the template registry and the error rules (step 2.06) |
| CI `api-container` | The image, as compose runs it, and the Postman smoke collection against it |
| [`scripts/chaos-api.sh`](../../scripts/chaos-api.sh) | The step 1.15 chaos run through the API: PostgreSQL or the service killed under load, then every request checked |

## Working on the contract

The contract is edited by hand from here on. After every change:

```sh
# Syntax lint (offline). Governance rules need `postman login`.
postman spec lint openapi/mezgeb.v1.yaml --no-report-events --issueType syntax

# The contract tests: one operation per template with its exact shape, an
# example of every error, raw lines only on the adjustment, every write
# idempotent and every operation scoped.
cargo test -p mezgeb-api --test openapi

# Regenerate the collection from the contract, then lint it.
postman spec sync collection openapi/mezgeb.v1.yaml
postman collection lint postman/collections/mezgeb-api-v1
```

A sync also regenerates the sample values in the generated examples (IDs
and times), so expect a wide diff; review the requests, not the samples.
The generator writes some RFC 3339 query values without quotes, which the
collection lint then reports as `FMT209`. Fix each with
`postman collection request update "<request>" --collection postman/collections/mezgeb-api-v1 --folder <folder> --url "<url>"`
(the three affected requests are the account balance, the account history
and the trial balance).
