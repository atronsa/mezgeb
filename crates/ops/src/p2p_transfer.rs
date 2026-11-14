//! `p2p_transfer.v1`: one customer sends money to another, with no fee
//! (worked entry E06, `docs/accounting/entries/payments.md`).
//!
//! | Role | Account | Dr | Cr |
//! |---|---|---:|---:|
//! | `payer_wallet` | 2010 Customer wallets | amount | |
//! | `payee_wallet` | 2010 Customer wallets | | amount |
//!
//! Account codes are from the wallet-provider chart
//! (`charts/wallet-provider.toml`). Whether the payer may send the money
//! (KYC, limits, fraud rules) is the client's decision; whether the payer
//! has it is checked by the posting rules (INV-8).

use mezgeb_core::PositiveAmount;

use crate::error::TemplateError;
use crate::template::{Accounts, Input, Leg, Role, Template};

/// 2010 Customer wallets.
const CUSTOMER_WALLETS: &[u16] = &[2010];

/// Version 1 of the P2P transfer.
#[derive(Debug, Clone, Copy)]
pub struct P2pTransferV1;

/// The typed request for [`P2pTransferV1`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct P2pTransferRequest {
    /// What the payer sends and the payee receives.
    pub amount: PositiveAmount,
}

impl Template for P2pTransferV1 {
    const NAME: &'static str = "p2p_transfer";
    const VERSION: u16 = 1;
    const ROLES: &'static [Role] = &[
        Role::new("payer_wallet", CUSTOMER_WALLETS),
        Role::new("payee_wallet", CUSTOMER_WALLETS),
    ];
    const AMOUNTS: &'static [&'static str] = &["amount"];

    type Request = P2pTransferRequest;

    fn request(input: &Input<'_>) -> Result<P2pTransferRequest, TemplateError> {
        Ok(P2pTransferRequest {
            amount: input.amount("amount")?,
        })
    }

    fn book(request: &P2pTransferRequest, _: &Accounts<'_>) -> Result<Vec<Leg>, TemplateError> {
        Ok(vec![
            Leg::debit("payer_wallet", request.amount),
            Leg::credit("payee_wallet", request.amount),
        ])
    }
}
