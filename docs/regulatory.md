# Regulatory Applicability

> **Status:** Questions logged, answers pending. Nothing here is legal advice.
> No requirement is treated as binding until a legal advisor confirms it
> ([architecture §6](architecture.md#6-standards-mandatory-useful-optional)).

## Who is regulated

Mezgeb is open-source software. The organisation that **operates** it to hold
customer money (a wallet provider, bank or payment company) is the regulated
party, not the Mezgeb project. Mezgeb's job is to make compliance possible:
the records, controls and reports a regulated operator needs must exist and be
configurable, not hard-coded to one reading of one rule.

The questions below are about **Ethiopia** first, because the National Bank
of Ethiopia (NBE) regulates the first expected operators. Other countries are
added when an operator needs them.

## How to use this file

1. Each question has an ID (`R-nn`), why it matters to Mezgeb, the candidate
   sources, and a status.
2. A legal advisor answers each question and names the exact article.
3. The answer and the advisor's name and date are recorded here. If an
   answer changes the design, it gets an [ADR](adr/README.md).

| Status | Meaning |
|---|---|
| **Open** | Not yet researched or answered |
| **Source found** | A likely source is identified; not confirmed |
| **Confirmed** | Answered by a legal advisor, with the article cited |
| **Not applicable** | Confirmed not to apply |

## Candidate sources

Found through secondary sources (law-firm notes and policy trackers) on
2026-11-10. **None has been checked against the official text.** Versions and
article numbers must be confirmed with a legal advisor.

| ID | Source | Notes |
|---|---|---|
| S1 | National Payment System Proclamation No. 718/2011 | The main payment systems law. Listed on the [NBE proclamations page](https://nbe.gov.et/proclamations/national-payment-system/). |
| S2 | National Payment System (Amendment) Proclamation No. 1282/2023 | Amends S1. Reported to widen NBE powers and allow foreign investment in payment instrument issuers. |
| S3 | Licensing and Authorization of Payment Instrument Issuers Directive No. ONPS/01/2020 | Defines electronic money, capital and float rules. Reported amendments: ONPS/06/2022, ONPS/09/2023 and possibly ONPS/10/2025. **Which version is in force is unclear.** |
| S4 | Personal Data Protection Proclamation No. 1321/2024 | Reported to require local storage of personal data, limits on cross-border transfer, and registration of data controllers and processors. |
| S5 | Prevention and Suppression of Money Laundering and Financing of Terrorism Proclamation No. 780/2013, and related directives | Record keeping and suspicious transaction reporting. Number and current version to confirm. |
| S6 | ISO 4217 | Currency codes and minor units. |

## Questions

### Licensing and scope

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-01 | Does publishing or supplying open-source ledger software require any NBE licence or approval? | Decides whether the project itself has obligations, or only operators do | S1, S2 | Open |
| R-02 | Which licence does an operator running Mezgeb as a wallet need: payment instrument issuer, payment system operator, or bank? | Decides which directive's technical requirements Mezgeb must support | S1, S2, S3 | Source found |
| R-03 | Which version of the payment instrument issuer directive is currently in force? | Sources disagree (ONPS/09/2023 or ONPS/10/2025). Every other answer depends on it | S3 | Open |
| R-04 | May an operator outsource hosting of the ledger to a third party or a cloud provider, and does that need NBE approval? | Affects deployment options (step 3.11) | S3, S4 | Open |

### Customer funds (float)

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-05 | How must the float backing electronic money be held: bank deposits only, government securities, at which banks? | Decides the asset accounts in the chart (1010) and the safeguarding check (MON-1) | S3 | Source found |
| R-06 | Must the float be reconciled against outstanding electronic money daily, and reported to the NBE? | Decides how often MON-1 runs and what report it produces (steps 1.13, 2.15) | S3 | Open |
| R-07 | Can the float earn interest, and who is entitled to it? | Decides whether an interest income account and a policy are needed | S3 | Open |

### Limits and offline payments

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-08 | What wallet balance and transaction limits apply per customer level (KYC tier)? | Limits are client policy, but the ledger may need per-account caps as a backstop (step 2.08) | S3 | Open |
| R-09 | Are offline electronic money payments permitted? If so, are there limits on amount, duration or device type? | Decides offline allowance limits per trust tier ([ADR-0010](adr/0010-offline-escrow-model.md), steps 2.10–2.13) | S3 | Open |
| R-10 | Must merchants be settled within a set time (for example, T+1)? | Affects settlement scheduling (E15, step 3.07) | S3 | Open |

### Records and reporting

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-11 | How long must transaction records be kept, and in what form? | Decides retention and archival (step 3.04); history is append-only ([ADR-0011](adr/0011-append-only-history.md)) | S3, S5 | Open |
| R-12 | Which periodic reports must an operator send to the NBE (transactions, float, complaints, incidents), how often and in what format? | Decides the reports the ledger must produce (step 3.05) | S3 | Open |
| R-13 | What must be recorded and exported for anti-money-laundering checks and suspicious transaction reports? | Decides which fields and exports the ledger keeps (steps 2.09, 3.05) | S5 | Open |
| R-14 | Must security incidents be reported to a regulator, and within what time? | Affects incident runbooks (step 3.09) | S3 | Open |

### Data protection

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-15 | Are ledger records (account references and amounts, without names) personal data? | Decides whether the ledger itself falls under data protection rules | S4 | Source found |
| R-16 | Must the ledger database and its backups be stored on servers in Ethiopia? | Decides deployment and backup locations (steps 3.10, 3.11) | S4 | Source found |
| R-17 | Is the operator of the ledger a data controller or a processor, and must it register? | Decides operator obligations and the documentation Mezgeb should provide | S4 | Open |

### Accounts and customers

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-18 | What happens to dormant wallets and unclaimed balances? | May require a dormant status and a transfer to an unclaimed funds account | S3 | Open |
| R-19 | Are there required timelines for handling complaints, disputes and refunds? | Affects refund and dispute flows (step 2.04) | S3 | Open |
| R-20 | Must operators connect to the national switch for interoperability, and how is settlement with other operators booked? | Affects clearing accounts and settlement (steps 2.02, 3.13) | S2, S3 | Open |

### Currency

| ID | Question | Why it matters to Mezgeb | Sources | Status |
|---|---|---|---|---|
| R-21 | What is the currency code and minor unit for the Ethiopian birr? | Money is stored in minor units ([ADR-0005](adr/0005-money-as-i64-minor-units.md)) | S6 | **Answered:** `ETB`, numeric code 230, 2 decimal places (1 birr = 100 santim). Standard, not legal; no advisor needed. |

## Summary

| Status | Count |
|---|---:|
| Open | 16 |
| Source found | 4 |
| Answered | 1 |
| Confirmed by legal advisor | 0 |

## Design rules until answers arrive

- **Configurable, not hard-coded.** Retention periods, limits, offline
  allowances and report schedules are settings an operator chooses.
- **Keep more, not less.** History is append-only and nothing is deleted by
  default, so a long retention requirement can always be met.
- **No compliance claims.** The README and docs never say Mezgeb "complies
  with" any regulation until an advisor confirms it for a specific operator.
