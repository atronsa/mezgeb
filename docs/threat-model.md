# Threat Model v0

> **Version:** v0. Reviewed again at the phase 2 security review (step 2.16)
> and before release (phase 4).

A threat model lists who might attack the ledger, what they want, how they
could try, and what stops them. Every threat here has an **owner** who is
responsible for it, and at least one **control**, with the roadmap step that
builds it.

Threats are classified with **STRIDE**:

| Letter | Threat | Means | Example |
|---|---|---|---|
| **S** | Spoofing | Pretending to be someone else | Using a stolen API key |
| **T** | Tampering | Changing data or code | Editing a journal row |
| **R** | Repudiation | Denying an action that happened | "I never made that payment" |
| **I** | Information disclosure | Reading data you should not | Listing another client's accounts |
| **D** | Denial of service | Making the system unavailable | Flooding the API |
| **E** | Elevation of privilege | Gaining rights you should not have | A support agent approving their own adjustment |

## Assets

What an attacker wants, and what must be protected:

| ID | Asset | Why it matters |
|---|---|---|
| A1 | **Balances and the journal** | They are the money. A wrong balance is lost or created money. |
| A2 | **Signing keys**: ledger receipt key, voucher issuer key, device keys | A stolen key can forge receipts or offline allowances. |
| A3 | **Audit log and hash chain** | The proof of what happened. If it can be rewritten, theft can be hidden. |
| A4 | **API credentials** | Whoever holds them can post entries as that client. |
| A5 | **Availability** | Payments stop when the ledger is down. |
| A6 | **Customer data** | The ledger stores only references (`external_ref`), not names or IDs, but references and amounts are still private. |

## Trust boundaries

```
  Customer device ──(offline receipts)──▶ Merchant terminal
        │                                       │
        ▼                                       ▼
  ┌───────────────────────── Internet ──────────────────────────┐
        │                                       │
        ▼                                       ▼
  Client application server  ──── (1) REST + credentials ────▶  Ledger service
  (users, KYC, fees, limits)                                    (api, ops, engine)
                                                                      │
                                    Operators ── (3) admin access ──▶ │ (2) own DB role
                                                                      ▼
                                                                PostgreSQL
```

1. **Client → ledger API.** Every request is authenticated and limited to
   what that client may do.
2. **Ledger service → database.** The service's database role can insert
   entries but cannot update or delete them ([ADR-0001](adr/0001-separate-service-own-database.md)).
3. **Operators → production.** Administrators are trusted to run the system,
   not to move money alone.

The customer device and merchant terminal are **untrusted**: anything they
send is a claim, verified before it is settled ([ADR-0010](adr/0010-offline-escrow-model.md)).

## Owners

| Owner | Who | Responsible for |
|---|---|---|
| **Maintainers** | The Mezgeb project | Code: engine, invariants, protocol, defaults that are safe out of the box |
| **Operator** | Whoever deploys and runs a Mezgeb instance | Infrastructure, keys, access, monitoring, backups, incident response |
| **Client** | The platform that calls the ledger | Its own server, users, business rules (KYC, limits, fees, fraud) |

## Threats

Status of every control is **Planned** until the step named builds and tests
it. Invariants are in the [invariant catalogue](invariants.md).

### External attacker

Someone on the internet with no legitimate access.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-01 | Steals or guesses a client's API credentials and posts entries as that client | S | A1, A4 | Strong per-client credentials (mutual TLS or signed requests), rotation, scoped permissions, alerts on unusual volume | Operator | 2.08 |
| TM-02 | Captures a valid request and replays it to post the same payment twice | T | A1 | TLS everywhere; idempotency key (INV-13); signed requests with a timestamp window | Maintainers | 1.11, 2.08 |
| TM-03 | Floods the API so payments cannot be posted | D | A5 | Rate limits per client, request size limits, timeouts, load shedding before the database | Operator | 2.07, 3.01 |
| TM-04 | Sends crafted input: SQL injection, huge payloads, amounts that overflow | T, E | A1 | Parameterised SQL only (SQLx); checked arithmetic ([ADR-0005](adr/0005-money-as-i64-minor-units.md)); strict input parsing; fuzzing | Maintainers | 1.04, 2.07, 2.16 |
| TM-05 | Guesses account or entry IDs to read another client's data | I | A6 | Every read is scoped to the calling client; ID secrecy is never relied on | Maintainers | 2.08 |
| TM-06 | Ships malicious code through a compromised dependency | T, E | A1, A2 | Locked dependencies, `cargo-deny` and `cargo-audit` in CI, few dependencies, review of updates | Maintainers | 3.11 |

### Compromised client server

An attacker controls the client application's server, its code and its
ledger credentials. This is the most likely serious attack, because the
client's server is large and exposed.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-07 | Posts entries that create money, such as crediting a wallet from fee revenue | T, E | A1 | Clients post only through templates they are allowed to use, never raw entries; each template fixes which account types it may touch; per-client limits | Maintainers | 2.01, 2.08 |
| TM-08 | Connects to the ledger database directly to change balances | T, E | A1 | Separate database with its own credentials, unreachable from client networks ([ADR-0001](adr/0001-separate-service-own-database.md)) | Operator | 1.08, 3.11 |
| TM-09 | Reports a deposit that never arrived, to mint money in a wallet | S, T | A1 | Deposits pass through clearing before wallets where needed; daily reconciliation of 1010 against the bank statement; safeguarding check (MON-1) | Operator | 2.02, 2.15 |
| TM-10 | Moves money between its own users within the allowed templates and limits | T | A1 | Cannot be prevented by the ledger; limited by per-client and per-template limits, detected by reconciliation and volume alerts. Accepted risk, documented for clients. | Client | 2.08, 2.15 |

### Malicious insider

An administrator, developer or support agent with legitimate access to
production or to the admin tools.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-11 | Edits or deletes journal rows directly in the database to hide a theft | T | A1, A3 | Update and delete blocked by trigger and role privileges (INV-9); hash chain (INV-11); signed receipts held by clients (INV-26); superuser access restricted and logged | Operator | 1.08, 2.09 |
| TM-12 | Posts an adjustment that credits an account they control | E | A1 | Maker-checker with different people (INV-25), reason codes, limits per person, audit events. **Built in 1.12:** maker ≠ checker and reason codes, in the engine and the database; see the [step 1.12 review](evidence/1.12-reversals-and-adjustments.md#exit-gate). | Maintainers | 1.12, 2.08 |
| TM-13 | Reopens a closed period to change past figures | T | A1, A3 | Closed periods immutable (INV-10); reopening needs maker-checker and leaves an audit event | Maintainers | 2.05, 2.08 |
| TM-14 | Copies the receipt or voucher signing key to forge receipts or offline allowances | S, T | A2 | Keys in a key management service or HSM, never in the database, code or environment files; key IDs and rotation; access logged | Operator | 3.12 |
| TM-15 | Disables the verification job or its alerts so that a broken ledger goes unnoticed | R, T | A3 | A missing verification run raises its own alert; verification also runs in CI and from an independent schedule; separation of duties. **Built in 1.13:** `mezgeb-cli verify`, its own `mezgeb_verifier` role, every run recorded in an append-only table, verification in CI. Scheduling and the missing-run alert come in 3.09. | Operator | 1.13, 3.09 |
| TM-16 | Exports account references and amounts in bulk | I | A6 | Least-privilege roles; read access to production data is logged; the ledger holds no names or national IDs | Operator | 2.08, 2.09 |

### Malicious device owner

A customer who controls their own phone or card and wants to spend more than
they have offline.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-17 | Clones the device and spends the same allowance twice | T | A1 | Escrow caps the total (INV-19); each `(device, counter)` settles once (INV-20); a conflicting receipt freezes the device (INV-21); limits per trust tier | Maintainers | 2.11–2.13 |
| TM-18 | Forges a receipt, or changes the amount or payee of a signed one | S, T | A1 | Only receipts with a valid device signature, an active voucher and an unbroken hash chain are settled (INV-22) | Maintainers | 2.12 |
| TM-19 | Changes the device clock to use an expired voucher or backdate a payment | T | A1 | Expiry is decided by the ledger's time at settlement, not the device's; `event_time` is informational only (INV-12); the counter gives the order | Maintainers | 2.12, 2.13 |
| TM-20 | Spends offline, then reports the device lost to get the escrow back | R | A1 | Escrow returns only after a grace period for late receipts; signed receipts prove the spending; written loss policy | Maintainers | 2.13 |
| TM-21 | Pays offline but never syncs, hoping the payment is never settled | R | A1 | The merchant's terminal can upload the receipt too; either side's upload settles it (spec §5.5) | Maintainers | 2.12 |

### Colluding merchant

A merchant working alone or together with a customer.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-22 | Issues fake refunds to move money out or to recover fees | T | A1 | Refunds reference the original payment and are capped by it per account, including its fee, and can only go back to whoever paid (INV-15, ADR-0015); refund-ratio monitoring | Client | 2.04 |
| TM-23 | Uploads made-up offline receipts | S | A1 | A receipt must be signed by the payer's device (INV-22); a merchant cannot sign as a payer | Maintainers | 2.12 |
| TM-24 | Refunds more than the merchant balance after settlement, leaving a debt it never repays | T | A1 | Merchant accounts do not allow negative balances (INV-8) unless a receivable policy is chosen; open question in the [chart of accounts](accounting/chart-of-accounts.md#open-questions) | Client | 2.04, 3.07 |

### Faults and accidents

Not attackers, but the same assets are at risk.

| ID | Threat | STRIDE | Asset | Controls | Owner | Step |
|---|---|---|---|---|---|---|
| TM-25 | A bug in the engine posts a wrong entry or updates totals wrongly | T | A1 | Invariants enforced at several layers; property tests against a reference ledger; verification job (INV-2, INV-5) | Maintainers | 1.07, 1.13 |
| TM-26 | The process or database crashes in the middle of a posting | D, T | A1 | One transaction per posting; idempotency recorded with the entry; safe retries (INV-4, INV-13, [ADR-0008](adr/0008-idempotency-in-posting-transaction.md)) | Maintainers | 1.15 |
| TM-27 | Data is lost: disk failure, a bad migration, an operator mistake | D | A1, A3 | Point-in-time recovery from WAL archive; tested restores; full verification before reopening; forward-only, reviewed migrations | Operator | 3.10 |

## Accepted risks

- **A compromised client can misuse its own permissions (TM-10).** The
  ledger cannot tell a fraudulent payment from a real one if the client is
  allowed to make it. The ledger limits the damage and makes it visible; it
  cannot prevent it.
- **Offline loss up to the escrow amount.** By design, a cloned device can
  cost at most its reserved allowance, and a late receipt can cost at most
  what it settles under the loss policy.

## Out of scope for v0

- The client application's own security: user login, KYC, card data. The
  client owns these.
- Physical attacks on servers and hardware security modules.
- Legal and regulatory controls: see [regulatory.md](regulatory.md).
