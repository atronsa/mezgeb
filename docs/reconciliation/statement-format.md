# Bank Statement Format

[Reconciliation](README.md) · Code: [`crates/engine/src/recon/statement.rs`](../../crates/engine/src/recon/statement.rs) · Decision: [ADR-0020](../adr/0020-reconciliation.md) decision 3

Mezgeb reads one CSV layout of its own, and any bank's CSV export through a
small mapping file that names the bank's columns. Nothing in a statement is
rounded or guessed: a line that does not read exactly is refused with its
line number, and nothing from that file is imported.

## Mezgeb's layout

A header row, then one line per movement:

```csv
line_id,booking_date,amount,currency,reference
TXN-20261116-0001,2026-11-16,1000.00,ETB,HANA
TXN-20261116-0002,2026-11-16,-15.00,ETB,"monthly charge, Nov"
```

| Column | Required | Meaning |
|---|---|---|
| `line_id` | Yes | The bank's own ID for the line, 1 to 100 characters, unique for the account. Importing the same line again is skipped; the same ID with different content is a `statement_conflict` |
| `booking_date` | Yes | The date the bank booked it, `YYYY-MM-DD` |
| `amount` | Yes | Signed, with a `.` and at most the currency's decimal places: **positive is money into the account**, negative is money out |
| `currency` | Yes | ISO 4217, and the account's currency |
| `reference` | No | The payment reference as the bank shows it, up to 200 characters |

Fields may be quoted (`"…"`, with `""` for a quote). Blank lines are
ignored, and a leading byte-order mark is allowed. Zero amounts are refused.

## A bank's own layout: the mapping file

A mapping is a TOML file that describes one bank's export. Keep one per
bank, next to the operator's other configuration, and pass it with
`mezgeb-cli recon import … --mapping FILE`.

```toml
name = "Example Bank CSV export"
delimiter = ";"
skip_lines = 2                   # title lines before the header row
date_format = "DD/MM/YYYY"
decimal_separator = ","
thousands_separator = "."
currency = "ETB"                 # when there is no currency column

[columns]
line_id = "Txn ID"
date = "Value date"
debit = "Debit"                  # money out
credit = "Credit"                # money in
reference = "Narrative"
```

| Key | Default | Meaning |
|---|---|---|
| `name` | | A name for the format, recorded with each import |
| `delimiter` | `,` | The field separator |
| `skip_lines` | 0 | Lines before the header row |
| `date_format` | | `YYYY-MM-DD`, `YYYYMMDD`, `DD/MM/YYYY`, `MM/DD/YYYY`, `DD.MM.YYYY` or `DD-MM-YYYY` |
| `decimal_separator` | `.` | Must differ from the delimiter and the thousands separator |
| `thousands_separator` | none | Removed from amounts |
| `currency` | the account's | The currency of every line, when there is no currency column |
| `sign` | `credit_positive` | For one signed amount column: `credit_positive` (money in is positive) or `debit_positive` (money out is positive) |
| `columns.line_id`, `columns.date` | | The header names of those columns |
| `columns.amount` | | One signed amount column; **or** |
| `columns.debit` and `columns.credit` | | Separate columns, exactly one filled per line |
| `columns.reference`, `columns.currency` | none | Optional columns |

Columns are found by their header name, so their order does not matter and
other columns are ignored. An amount may carry a leading `+` or `-`, or be
in parentheses for a negative; banks' leading zeros are allowed.

## What the ledger side needs

Reconciliation matches a statement line with a ledger line of the same
account (ADR-0020 decision 5). When a client books a movement of 1010, it
should record in the entry's metadata:

| Key | Value | Used by |
|---|---|---|
| `bank_ref` | The statement line's `line_id` | The exact pass |
| `payment_ref` | The statement line's `reference` | The rule pass: same amount, same reference, dates at most 3 days apart, one candidate on each side |

Without `bank_ref`, only the rule pass can match, and two payments of the
same amount and reference within the window are left as breaks rather than
guessed between.

## Adding a bank

1. Ask the bank for a sample export of a quiet day, and its column list.
2. Write the mapping. Check it reads by importing the sample into a test
   database: `mezgeb-cli recon import 1010 sample.csv --mapping bank.toml`.
3. If the bank offers ISO 20022 camt.053, it can be added as a second
   reader later; the matching does not change.
