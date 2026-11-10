//! Posting templates (ADR-0002, layer 2).
//!
//! Named, versioned functions such as `p2p_transfer.v1` or
//! `merchant_payment.v1` that turn a typed request into a balanced entry.
//! They define *how* a business event is booked, never *whether* it is
//! allowed: that is the client application's decision.
//!
//! Each template maps to a worked entry in `docs/accounting/entries/`.
//! Built in step 2.01.
