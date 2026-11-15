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
| Offline | Devices, vouchers, receipts: answer `501` until steps 2.11 and 2.12 |

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

**Authentication.** OAuth 2.0 client credentials, with a scope per kind of
action; designed in step 2.08 (until then, see
[Interim authentication](#interim-authentication)). A client sees only its own accounts and
entries. Operator scopes (`accounts:admin`, `corrections:write`,
`adjustments:*`, `periods:close`, `reports:read`) are never given to client
platforms.

**Batches.** Not in v1. Each request posts one entry, which keeps every
entry atomic and idempotent on its own. A batch endpoint would be a later,
additive change if clients need one.

**Receipts.** Every entry has a `receipt` field, the signed ledger receipt
of spec §7.2. It is `null` until step 2.09.

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
curl -s http://localhost:8080/v1/templates -H 'Authorization: Bearer dev-client-token'
```

Or from a checkout, against the compose PostgreSQL:

```sh
cargo run -p mezgeb-cli -- migrate     --database-url postgres://mezgeb_owner:mezgeb_owner@localhost:55432/mezgeb
MEZGEB_DATABASE_URL=postgres://mezgeb_app:mezgeb_app@localhost:55432/mezgeb MEZGEB_TOKENS=deploy/tokens.dev.toml cargo run -p mezgeb-api
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
| Strict bodies | Unknown fields, JSON numbers for amounts, more decimal places than the currency, and unknown currencies are `400 invalid_request`, with a JSON Pointer to the field in `errors` |
| Timeout | After `MEZGEB_REQUEST_TIMEOUT_MS` (10 s), the client gets `503 busy` with `Retry-After`. **The write is not cancelled:** it runs in its own task and commits or fails on its own, so a retry with the same key returns its entry |
| Access log | One JSON line per request: method, route pattern (never the path with its IDs), status, latency, request ID and client. No bodies, tokens, amounts, account references or idempotency keys |
| Errors | Every error is Problem Details. An unexpected error is logged with its cause and answered with `500 internal_error`, which says nothing about the cause |
| Shutdown | On SIGTERM or Ctrl-C, the service stops accepting connections, finishes the requests under way, then waits for every write task before exiting |

**Replays.** A repeated write with the same key answers `200` with
`Idempotent-Replayed: true` and the original result; opening an account
too. Requesting an adjustment answers `202` both times, with the replay
header the second time.

### Interim authentication

Step 2.08 builds OAuth 2.0. Until then the service reads static bearer
tokens from the file named by `MEZGEB_TOKENS`, which holds only their
SHA-256 hashes, each with its client ID, actor and scopes (format in
[`crates/api/src/auth/tokens.rs`](../../crates/api/src/auth/tokens.rs)). Every route
checks its scope: default deny. [`deploy/tokens.dev.toml`](../../deploy/tokens.dev.toml)
has three tokens for development and CI, and must never be used anywhere
else.

What the interim scheme does not do yet, and step 2.08 must: token expiry
and rotation, a clients table, and limiting each client to its own accounts
and entries (the API records who opened each account, ready for that).
Until then, run the service only where every token holder is trusted.

### Testing it

```sh
docker compose up -d
export MEZGEB_TEST_DATABASE_URL=postgres://postgres:postgres@localhost:55432/postgres
cargo test -p mezgeb-api
```

| Test | What it proves |
|---|---|
| `tests/contract.rs` | One scenario calls all 42 operations of the contract and checks every response (status, headers, body) against the contract's schema for it; it fails if an operation is never called |
| `tests/http.rs` | A client timeout followed by a retry gives one entry; bad, negative, float and oversized amounts and bodies are refused with their codes; a graceful shutdown under load loses no acknowledged request |
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
