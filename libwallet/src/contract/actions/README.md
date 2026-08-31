# Contract actions

### API endpoints

Four owner API methods, each with a matching `grin-wallet contract` subcommand: `new`,
`sign`, `view` and `revoke`. There is no `setup` endpoint; setup is a step rather than a
call, run by `new` for the initiator and by `sign` for the signer if it hasn't happened
yet. So a two party contract is `new`, `sign`, `sign`, and a self-spend is `new`, `sign`.

`view` summarises a slate: participants, signatures so far, the net change it suggests or
the one we already agreed to, and whether the transaction has confirmed. `revoke`
double-spends an input of a contract we signed but that hasn't confirmed, and is
idempotent, so an interrupted revoke can be run again.

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

We only allow contribution of custom inputs/outputs when we're doing the setup phase. Once the setup phase is done,
we no longer allow any customization of inputs. This means that the customization can only happen at contract setup phase which is the first time we see the contract. Additionally, if we customize output selection, we immediately pick the inputs/outputs which means it's an early lock. These are however not added to the slate until we reach the 'sign' phase of the contract. Counterparties don't need to see our inputs/outputs before that. This means we always add inputs/outputs only when we have to and never before.

Ideally we'd also separate side effects out of these functions e.g. computing the current_height
or refreshing the outputs with updater::refresh_outputs(...). The current_height could be
communicated through a &ChainState parameter which would collect these values before the call.
Additionally, we could fetch the existing Context before the call to avoid doing db fetch.
Separating side effects until the 'save_step' part would make these functions much easier to test.

#### Not supported

 - More than two parties. `num_participants` is validated to 1 (self-spend) or 2.
 - Payment proof types other than invoice (type 1). A variant that doesn't depend on your
   position in the contract is RFC work.
 - `--send` and `--receive` take a human readable amount, as `send` and `invoice` do.
   Everything reaching the API, including `--make-outputs`, is nanogrin.
 - Writing the signed transaction in the same commit as the wallet state. `store_tx`
   writes a file outside LMDB, as it does for every transaction in the wallet;
   `contract::utils::save_step` describes the window and how to recover.

#### TODOs

 - `next_child_for` reads the derivation index and writes it back in two batches, so
   concurrent steps can be handed the same child key. `next_child` has the same shape.
 - Contract input selection requires a single confirmation.
 - `--use-inputs` names the inputs to spend but doesn't lock them, unlike `--add-outputs`.
 - Check the commitments we contributed are the ones that come back, so a counterparty
   can't return a slate carrying inputs or outputs of ours that we didn't put there.
 - Handle the coinbase flows. Coinbase inputs are built into the slate, but nothing else
   is exercised.
 - For a payjoin, `--use-inputs any` picks an input for us; naming the commitment would be
   clearer, for the API as much as the CLI.
 - `new` has no `--no-setup`.
 - Separate side effects out from the computation, as described above.
 - Decide whether `contract_accounts_switch.rs` is a legitimate scenario, or whether
   signing under a different account than setup should error.
 - Decide whether `target_slate_version` should control Slatepack output or be removed.

#### Tests

 - signing the same slate twice, which `verify_not_signed` guards
 - zero-value outputs, and `--make-outputs` beyond what is available
 - a receiver with no spendable input, through lack of funds or of confirmations
 - contracts spending more than one input
 - slate contents at each step, not only the end state
 - the foreign API entry points for `new` and `sign`
 - fee contribution across one and two parties

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
	//  - add_outputs -> contribute_output -> let key_id = keys::next_available_key(wallet, keychain_mask).unwrap();
	//  - TODO: would we need to compute keys::next_available_key for as many outputs as we plan to contribute and pass
	//    them as a param to keep this without side effects?

#### Sign
	// Side-effects:
	//  - contract_utils::check_already_signed -> tx_log_iter
	//  - contract_utils::get_net_change -> context and net_change
	//  - everything from 'setup'
