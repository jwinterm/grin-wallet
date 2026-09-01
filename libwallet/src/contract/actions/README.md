# Contract actions

The workflow, command behaviour and current limitations are documented in
[`doc/contracts.md`](../../../../doc/contracts.md). This file tracks implementation details
and unfinished work.

### Rust implementation

Every contract action on a slate is divided in 3 parts:
1. compute the new state
2. save the new state
3. return slate

Putting this into code, it looks like the following:
```rust
// Compute the new state (both of the Slate and the Context)
let (slate, context) = compute(slate, args);
// Atomically commit the new state
contract_utils::save_step(slate, context, ...);
// Return the newly produced slate
return slate;
```

`save_step` writes the context, tx log, outputs and input locks in one LMDB batch. The
signed transaction is stored separately outside LMDB and cannot share that batch.

Custom input and output selection is only accepted during setup. `add_outputs` selects and
locks them during setup; otherwise this happens during signing. In either case they are
only added to the slate when signing, so the counterparty does not see them earlier.

Ideally we'd also separate side effects out of these functions e.g. computing the current_height
or refreshing the outputs with updater::refresh_outputs(...). The current_height could be
communicated through a &ChainState parameter which would collect these values before the call.
Additionally, we could fetch the existing Context before the call to avoid doing db fetch.
Separating side effects until the 'save_step' part would make these functions much easier to test.

#### TODOs

 - `next_child_for` reads the derivation index and writes it back in two batches, so
   concurrent steps can be handed the same child key. `next_child` has the same shape.
 - Honour `min_input_confirmation`; contract input selection currently uses one confirmation.
 - Decide whether `--use-inputs` or `--make-outputs` should imply early locking; currently
   only `--add-outputs` locks during `new`.
 - Reserve funds across open late-locked contracts, or make the possible signing failure
   clearer to callers.
 - Check the commitments we contributed are the ones that come back, so a counterparty
   can't return a slate carrying inputs or outputs of ours that we didn't put there.
 - Handle the coinbase flows. Coinbase inputs are built into the slate, but nothing else
   is exercised.
 - For a payjoin, `--use-inputs any` picks an input for us; naming the commitment would be
   clearer, for the API as much as the CLI.
 - `new` has no `--no-setup`.
 - Detect a reversed `--send` or `--receive` when signing a slate for the first time.
 - Let `view` report whether a slatepack was encrypted for this wallet.
 - Show all inputs, outputs, fees and resulting balance changes before the CLI signs.
 - Distinguish the transfer amount from the fee-adjusted balance change in `view`.
 - Raise the replacement fee when `revoke` races a transaction already in the mempool.
 - Merge invoice-proof retrieval with the existing payment-proof API.
 - Confirm that invoice-proof retrieval excludes the transaction fee from the amount.
 - Add a Grin node API for kernel lookup by MMR index, then use the witness index instead
   of searching by commitment.
 - Align the proof memo encoding with the early payment proofs proposal.
 - Implement sender-nonce proofs for the RSR flow.
 - Move payment-proof creation and verification onto `Slate` so it can also be used
   outside contracts and is versioned with the slate format.
 - Keep V5 when decoding Slatepacks through Owner RPC.
 - Preserve kernel features and their arguments across V4 and V5; see #793.
 - Choose the sender derivation path explicitly and keep encrypted outgoing files
   identifiable locally.
 - Decode the sender and slate from an incoming Slatepack in one pass.
 - Decide whether `target_slate_version` should control Slatepack output or be removed.
 - Add contract history, lookup by id, transport support and configurable proof memos.
 - Add full API and RPC examples once the contract interface is stable.
 - Separate side effects out from the computation, as described above.
 - Decide whether `contract_accounts_switch.rs` is a legitimate scenario, or whether
   signing under a different account than setup should error.

#### Tests

 - signing the same slate twice, which `verify_not_signed` guards
 - zero-value outputs, and `--make-outputs` beyond what is available
 - a receiver with no spendable input, through lack of funds or of confirmations
 - invalid or inconsistent slates, including changed commitments and unexpected states
 - the CLI conflict between `--no-payjoin` and `--use-inputs`
 - a committed fee that no longer covers the selected inputs and outputs
 - slate contents at each step, not only the end state
 - the foreign API entry points for `new` and `sign`
 - Slatepack version negotiation with the last supported wallet release

#### save_step

    // TODO:
    //  - is_signed should be derived from the slate
    //  - Consider taking ownership of Context here. It should not be used after this is called.

### Side-effects

#### Setup
	// Side-effects:
	//  - height = w.w2n_client().get_chain_tip()?.0;
	//  - maybe_context = w.get_private_context(keychain_mask, sl.id.as_bytes())
	//  - create_contract_ctx -> updater::refresh_outputs(wallet, keychain_mask, parent_key_id, false)?;
	//  - add_outputs -> let current_height = w.w2n_client().get_chain_tip()?.0;
	//  - add_outputs -> add_outputs_to_ctx -> w.next_child_for(...)

#### Sign
	// Side-effects:
	//  - contract_utils::check_already_signed -> tx_log_iter
	//  - contract_utils::get_net_change -> context and net_change
	//  - everything from 'setup'
