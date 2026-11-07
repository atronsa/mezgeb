//! Stable, enumerated results (spec §4.1). Never reorder or renumber:
//! these codes are part of the public API.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAccountError {
    IdMustNotBeZero,
    LedgerMustNotBeZero,
    FlagsAreMutuallyExclusive,
    ExistsWithDifferentFields,
    Exists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateTransferError {
    IdMustNotBeZero,
    AmountMustBePositive,
    AccountsMustBeDifferent,
    DebitAccountNotFound,
    CreditAccountNotFound,
    AccountsMustHaveSameLedger,
    TransferMustHaveAccountLedger,
    AccountClosed,
    ExceedsCredits,
    ExceedsDebits,
    Overflow,
    PendingTransferNotFound,
    PendingTransferAlreadyResolved,
    PendingTransferExpired,
    LinkedEventFailed,
    ExistsWithDifferentFields,
    Exists,
}
