# Mezgeb Ledger API (v1-draft)

[Documentation](../README.md) · Contract: [`openapi/mezgeb.v1.yaml`](../../openapi/mezgeb.v1.yaml) · Errors: [errors.md](errors.md) · Security review: [owasp-review.md](owasp-review.md) · Decision: [ADR-0009](../adr/0009-rest-api-with-axum.md)

The public API is designed before it is built (step 2.06). The contract,
an OpenAPI 3.1 document, is the promise to every client; step 2.07 builds
the service against it, and a test keeps the two in step.

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
action; designed in step 2.08. A client sees only its own accounts and
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
