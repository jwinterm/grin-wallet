# Wallet contracts

Contracts use the same flow for standard sends and invoice transactions. Signing is kept
as an explicit step. Each side can check the slate it receives and its own contribution,
but not commitments added by the other side later in the flow.

A two-party contract is `new`, `sign`, `sign`. A self-spend is `new`, `sign`. There is no
separate `setup` command. Setup is done by `new`, or by the first `sign` when needed.

The owner API provides all four commands. The Rust foreign API also provides `new` and
`sign` for receiving contracts, but they are not exposed by the foreign JSON-RPC API. The
API does not require manual confirmation, so a wallet using it still needs to show the
transaction and ask the user before signing. The reference CLI does not show the full
inputs, outputs, fees and balance changes yet.

The CLI exchanges Slatepacks. The last `sign` broadcasts the transaction unless
`--no-broadcast` is used.

## Amounts and fees

`--send` and `--receive` are the amount being transferred, before each side pays its own
fee. The sender spends the amount plus its fee. The receiver gets the amount minus its
fee. Each side pays for the inputs and outputs it adds, plus its rounded-up share of the
kernel fee.

CLI amounts are human readable. Values passed to the API, including `make_outputs`, are in
nanogrin. Outputs requested with `make_outputs` are added together with the output needed
to balance that side of the transaction.

Payjoin is used by default. A receiver adds an input when one is available. An empty wallet
can still receive; the fee is then taken from the received output. `--no-payjoin` prevents
the receiver from adding an input and cannot be used together with `--use-inputs`.

Inputs and outputs are normally picked when signing. `--add-outputs` picks and locks them
during `new`, but they are still only added to the slate when signing. `--use-inputs` and
`--make-outputs` limit the selection but do not lock anything early. Each party chooses
`--min_conf` when it first joins and cannot change it later. The same limit is used for the fee
estimate, so any required inputs need enough confirmations at that point. A value of `0` allows
unconfirmed non-coinbase outputs. Open contracts do not reserve funds, so a late-locked contract
can fail if another transaction spends the available outputs first.

## View and revoke

`view` reads a slate or Slatepack and shows the participants, signatures, suggested or
agreed amount change, whether the local transaction is confirmed, and whether it contains
unexpected inputs or outputs from this wallet. This check is reported as unknown after
the private context has been removed, when input features are missing, or when the slate
does not contain a transaction. `view` does not show all inputs, outputs and fees, and it
cannot find a contract by id.

`revoke` cancels the local transaction. When the wallet added an input, it returns a
self-spend of that input. The caller still has to post it, and either transaction can win
if the original is already in the mempool. When the wallet added no input there is no
replacement transaction. The replacement does not use a higher fee. An interrupted
`revoke` can be run again. Transaction ids belong to an account, so the CLI uses the
account selected with `--account`.

## Current limitations

* Only one or two participants are supported
* Early payment proofs are only implemented for contracts and are only available through
  the API. Only invoice proofs are supported and they require the experimental Slate V5
  format. The full proposal is described in
  [Early Payment Proofs](https://github.com/mimblewimble/grin-rfcs/pull/70)
* Custom fee rates and contract TTLs are not supported
* There are no contract-specific history, lookup or transport commands. Payment proof
  memos cannot be set through the contract API or CLI. The current proof stores a memo
  type and 32 bytes instead of the text and hash described by the early payment proofs
  proposal
* If writing the signed transaction file fails, the wallet state has already been saved.
  Cancelling the transaction releases the locked inputs

Implementation notes and remaining work are kept in
[`libwallet/src/contract/actions/README.md`](../libwallet/src/contract/actions/README.md).

## References

* [Contract prototype discussion](https://forum.grin.mw/t/grin-wallet-contract-prototype/9745)
* [Manual confirmation proposal](https://github.com/mimblewimble/grin-rfcs/pull/84) (open)
* [Early payment proofs proposal](https://github.com/mimblewimble/grin-rfcs/pull/70) (open)
* [RFC 0006: Payment Proofs](https://github.com/mimblewimble/grin-rfcs/blob/master/text/0006-payment-proofs.md)
* [RFC 0012: Compact Slates](https://github.com/mimblewimble/grin-rfcs/blob/master/text/0012-compact-slates.md)
* [RFC 0015: Slatepack](https://github.com/mimblewimble/grin-rfcs/blob/master/text/0015-slatepack.md)
* [RFC 0017: Fix Fees](https://github.com/mimblewimble/grin-rfcs/blob/master/text/0017-fix-fees.md)
