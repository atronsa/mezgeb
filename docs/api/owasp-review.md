# OWASP API Security Top 10 Review (v1-draft)

[API](README.md) · Contract: [`openapi/mezgeb.v1.yaml`](../../openapi/mezgeb.v1.yaml) · Threats: [threat-model.md](../threat-model.md)

Every endpoint group of the v1-draft contract, reviewed against the
[OWASP API Security Top 10 (2023)](https://owasp.org/API-Security/editions/2023/en/0x11-t10/).
For each risk: what the contract does about it, and what steps 2.07
(implementation), 2.08 (authentication and authorisation) and 2.16
(security review) must still prove. "Contract" means the rule is already in
`openapi/mezgeb.v1.yaml`; "must" is a requirement on the implementation.

## The groups

| Group | Endpoints | Who calls them |
|---|---|---|
| Accounts | `/v1/accounts…` | Client platforms (their own accounts); operators (status) |
| Templates and operations | `/v1/templates…`, `/v1/operations/*` | Client platforms |
| Entries | `/v1/entries/{id}`, `/v1/entries/by-idempotency-key/{key}` | Client platforms |
| Corrections | `/v1/entries/{id}/reversal`, `/v1/adjustments…` | Operators only |
| Holds | `/v1/holds…` | Client platforms |
| Periods and reports | `/v1/periods…`, `/v1/reports/*` | Operators only (periods: read by clients) |
| Offline | `/v1/offline/*` | Planned; `501` |

## Review

| Risk | Where it applies | Contract | The implementation must |
|---|---|---|---|
| **API1 Broken object level authorisation** | Every `{account_id}`, `{entry_id}`, `{hold_id}`; every account named in an operation body | A client sees only its own accounts and the entries that touch them; anything else is `404 not_found`, never `403`, so IDs cannot be probed (TM-05). Idempotency keys are per client | Check ownership on every read and on every account bound in a body, in one place, before any engine call. A client may bind a system account (`1010`, `4010`) only to a role whose chart codes allow it. Test that every endpoint returns `404` for another client's object (2.08) |
| **API2 Broken authentication** | All | OAuth 2.0 client credentials on every operation; no endpoint is anonymous | Short-lived tokens, verified signature and audience; no API keys in URLs; reject on clock skew; rate-limit the token endpoint (2.08) |
| **API3 Broken object property level authorisation** | Request and response bodies | Every request schema is closed (`additionalProperties: false`), so no field can be smuggled in (mass assignment). The requester and approver of a correction come from the token, never from the body. Responses carry no internal fields (no `version`, no row locks, no other client's data) | Reject unknown fields; build responses from explicit types, never from database rows |
| **API4 Unrestricted resource consumption** | Lists, history, reports, posting | `limit` is 1–100; history needs a bounded `from`/`to`; metadata is at most 32 keys and 4 KB; an entry at most 100 lines; `429` and `Retry-After` are part of the contract | Enforce body size limits, request timeouts and per-client rate limits; cap report ranges; the 30-second transaction limit already bounds each posting |
| **API5 Broken function level authorisation** | Corrections, account status, periods, reports | Each operation names its scope. Operator functions have operator-only scopes (`accounts:admin`, `corrections:write`, `adjustments:request`, `adjustments:approve`, `periods:close`, `reports:read`) | Never issue operator scopes to client platforms; check the scope before parsing the body; test every operation with a token lacking its scope (2.08) |
| **API6 Unrestricted access to sensitive business flows** | Refunds, disputes, adjustments, withdrawals | Clients post only through templates; there is no raw-lines endpoint except the adjustment, which needs a second, different person (`maker_is_checker`). Refunds are capped by the payment they name (INV-15). Withdrawals and disputes move whole items once | Per-client and per-template limits and velocity alerts (refund ratio, TM-22); log every correction with both people (2.09) |
| **API7 Server-side request forgery** | None | The API never fetches a URL given by a caller: no webhooks, callbacks or URL fields in v1 | Keep it so; any future webhook needs an allow-list (step 3.06) |
| **API8 Security misconfiguration** | All | Errors are Problem Details with a stable code and a short `detail`: no stack traces, SQL or internal IDs. Retryable errors say so | TLS only; no CORS (server-to-server API); security headers; no default credentials; the error mapper is the only path to a response body |
| **API9 Improper inventory management** | All | One versioned contract (`/v1`), checked against the code by `crates/api/tests/openapi.rs`; planned endpoints are marked `x-mezgeb-status: planned` and answer `501`; the compatibility policy is written down | Serve exactly the documented paths; no debug or admin routes outside the contract; publish the contract with each release |
| **API10 Unsafe consumption of APIs** | Offline receipts (planned) | Offline receipts will be signed by devices and verified before settlement (spec §5, INV-22) | Treat bank statements and device uploads as untrusted input; verify signatures and bounds before posting (2.12, 2.15) |

## Findings fixed in the draft

Reviewing the endpoints against the list changed the design in three places:

1. **Adjustments are two-step.** A single endpoint taking `requested_by` and
   `approved_by` in the body would let one operator claim a second
   approver (API3, API6). The contract has a request, then an approval or a
   rejection by a different authenticated person.
2. **Reversals take no actor fields.** The requester comes from the token,
   so it cannot be spoofed (API3).
3. **Not found means not found or not yours.** Using `404` for another
   client's object, instead of `403`, stops ID probing (API1).

## Open for later steps

- **2.08:** the token format, issuer and lifetime; the mapping from token to
  client; ownership checks; scope enforcement; tests of each.
- **2.16:** an external review of this table against the implementation.
