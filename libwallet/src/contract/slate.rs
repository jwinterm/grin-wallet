// Copyright 2022 The Grin Developers
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Contract functions on the Slate

use crate::backend::WalletBackend;
use crate::grin_core::libtx::build;
use crate::grin_core::libtx::proof::ProofBuilder;
use crate::grin_keychain::Keychain;
use crate::grin_util::secp::key::{PublicKey, SecretKey};
use crate::slate::{Slate, SlateState};
use crate::types::{Context, NodeClient};
use crate::Error;

use super::types::ProofArgs;
use crate::contract::proofs::InvoiceProof;
use ed25519_dalek::VerifyingKey as DalekPublicKey;

/// Add payment proof data to slate, noop for sender
pub fn add_payment_proof<C, K>(
	w: &mut WalletBackend<C, K>,
	slate: &mut Slate,
	keychain_mask: Option<&SecretKey>,
	context: &Context,
	net_change: &Option<i64>,
	proof_args: &ProofArgs,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	// FUTURE: move proof handling onto Slate itself so it can be versioned (slate.add_payment_proof_data()).
	debug!("contract::slate::add_payment_proof => called");
	if !proof_args.suppress_proof {
		super::proofs::check_proof_type(&proof_args.proof_type)?;
	}
	// If we're a recipient, generate proof unless explicity told not to
	if let Some(ref c) = net_change {
		if *c > 0 && !proof_args.suppress_proof && slate.payment_proof.is_none() {
			super::proofs::add_payment_proof(w, keychain_mask, slate, &context, proof_args)?;
		}
	}

	Ok(())
}

/// Verify payment proof signature
pub fn verify_payment_proof(
	slate: &Slate,
	net_change: i64,
	recipient_address: &DalekPublicKey,
) -> Result<(), Error> {
	// FUTURE: move proof verification onto Slate itself so it can be versioned (slate.verify_payment_proof_sig()).
	debug!("contract::slate::verify_payment_proof => called");
	if net_change > 0 && slate.payment_proof.is_some() {
		let invoice_proof = InvoiceProof::from_slate(&slate, 1, None)?;
		invoice_proof.verify_promise_signature(&recipient_address)?;
	}
	Ok(())
}

/// Adds inputs and outputs to slate
pub fn add_outputs<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &Context,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	add_inputs_to_slate(w, keychain_mask, slate, context)?;
	add_outputs_to_slate(w, keychain_mask, slate, context)?;
	// Adjust the offset for the added input and outputs
	let keychain = &w.keychain(keychain_mask)?;
	slate.adjust_offset(keychain, &context)?;

	Ok(())
}

/// Contribute inputs to slate
fn add_inputs_to_slate<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &Context,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	debug!("contract::slate::add_inputs_to_slate => adding inputs to slate");
	let keychain = w.keychain(keychain_mask)?;
	let batch = w.batch(keychain_mask)?;
	for (key_id, mmr_index, _) in context.get_inputs() {
		// We have no information if the input is a coinbase or not, so we fetch the data from DB
		let coin = batch.get(&key_id, &mmr_index)?;
		if coin.is_coinbase {
			slate.add_transaction_elements(
				&keychain,
				&ProofBuilder::new(&keychain),
				vec![build::coinbase_input(coin.value, coin.key_id.clone())],
			)?;
			debug!(
				"contract::slate::add_inputs_to_slate => added coinbase input id: {}, value: {}",
				coin.key_id.clone(),
				coin.value
			);
		} else {
			slate.add_transaction_elements(
				&keychain,
				&ProofBuilder::new(&keychain),
				vec![build::input(coin.value, coin.key_id.clone())],
			)?;
			debug!(
				"contract::slate::add_inputs_to_slate => added regular input id: {}, value: {}",
				coin.key_id.clone(),
				coin.value
			);
		}
	}

	Ok(())
}

/// Contribute outputs to slate
fn add_outputs_to_slate<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &Context,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	debug!("contract::slate::add_outputs_to_slate => start");
	let keychain = w.keychain(keychain_mask)?;
	// Iterate over outputs in the Context and add the same output to the slate
	for (key_id, _, amount) in context.get_outputs() {
		slate.add_transaction_elements(
			&keychain,
			&ProofBuilder::new(&keychain),
			vec![build::output(amount, key_id.clone())],
		)?;
		debug!(
			"contract::slate::add_outputs_to_slate => added output to slate. Output id: {}, amount: {}",
			key_id.clone(),
			amount
		);
	}

	Ok(())
}

/// Transition the slate state to the next one
pub fn transition_state(slate: &mut Slate) -> Result<(), Error> {
	// We don't really use these states right now apart from leaving it to derive expected net_change.
	// This suggests these can't be used for manipulation. It doesn't hurt to think a bit more if that's the case.
	let new_state = match slate.state {
		SlateState::Invoice1 => SlateState::Invoice2,
		SlateState::Invoice2 => SlateState::Invoice3,
		SlateState::Standard1 => SlateState::Standard2,
		SlateState::Standard2 => SlateState::Standard3,
		// Unknown, or a slate that has already reached its final state, is not something
		// we can advance. We have signed by this point and the wallet state is persisted
		// next, so report it rather than reporting a Standard3 that never happened.
		ref s => {
			return Err(Error::GenericError(format!(
				"Cannot advance a contract slate in state {}",
				s
			)))
		}
	};
	slate.state = new_state;
	// NOTE: It's possible to never reach the step3. A self-spend has only 2 steps: new -> sign.
	Ok(())
}

/// Add partial signature to the slate.
// Nonce reuse is prevented by forgetting the signing context after a signed step:
// save_step deletes the private context (which holds sec_key/sec_nonce) once is_signed
// (except the deliberately-retained step2 context used for safe cancel). The context is
// keyed by slate.id, so a given nonce is only ever used for one message.
pub fn sign<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &mut Context,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	debug!("contract::slate::sign => called");
	// The counterparty controls the participant list, so require it to hold exactly the
	// participants the slate declares before we sign over it. Setup has already added our
	// own entry by this point.
	if slate.participant_data.len() != slate.num_participants as usize {
		return Err(Error::GenericError(format!(
			"Expected {} participant(s) before signing, found {}",
			slate.num_participants,
			slate.participant_data.len()
		)));
	}
	let keychain = w.keychain(keychain_mask)?;
	slate.fill_round_2(&keychain, &context.sec_key, &context.sec_nonce)?;
	debug!(
		"contract::sign => signed for slate fees: {}",
		slate.fee_fields
	);
	debug!("contract::slate::sign => done");

	Ok(())
}

/// We can finalize if all partial sigs are present
pub fn can_finalize(slate: &Slate) -> bool {
	let res = slate
		.participant_data
		.clone()
		.into_iter()
		.filter(|v| !v.is_complete())
		.count();

	// We can finalize if the number of partial sigs is the same as the number of participants
	res == 0 && slate.participant_data.len() == slate.num_participants as usize
}

/// Finalize slate
pub fn finalize<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	debug!("contract::slate::finalize => called");
	// Final transaction can be built by anyone at this stage
	trace!("Slate to finalize is: {}", slate);
	// At this point, everyone adjusted their offset, so we update the offset on the tx
	slate.tx_or_err_mut()?.offset = slate.offset.clone();
	slate.finalize(&w.keychain(keychain_mask)?)?;

	Ok(())
}

/// Perform 'setup' step for a contract. This adds our public key and nonce to the slate
/// The operation should be idempotent.
pub fn add_keys<K>(slate: &mut Slate, keychain: &K, context: &mut Context) -> Result<(), Error>
where
	K: Keychain,
{
	debug!("contract::slate::add_keys => called");
	let our_pub_key = PublicKey::from_secret_key(keychain.secp(), &context.sec_key)?;
	let our_pub_nonce = PublicKey::from_secret_key(keychain.secp(), &context.sec_nonce)?;
	// An entry is ours only when both keys match, which is what add_participant_info and
	// fill_round_2 use. Our excess paired with a nonce that is not ours cannot have come
	// from us, so reject it rather than treating the entry as ours: we would otherwise
	// append a further participant and sign nothing.
	if slate
		.participant_data
		.iter()
		.any(|p| p.public_blind_excess == our_pub_key && p.public_nonce != our_pub_nonce)
	{
		return Err(Error::GenericError(
			"Slate carries our public excess with a different nonce".to_string(),
		));
	}
	// Guard against a tampered slate that already carries the full participant set.
	// Re-adding our own info is idempotent, so only block when we are not already in it.
	let already_ours = slate
		.participant_data
		.iter()
		.any(|p| p.public_blind_excess == our_pub_key && p.public_nonce == our_pub_nonce);
	if !already_ours && slate.participant_data.len() >= slate.num_participants as usize {
		return Err(Error::GenericError(format!(
			"Slate already has the expected {} participant(s)",
			slate.num_participants
		)));
	}
	slate.add_participant_info(keychain, context, None)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::grin_keychain::{ExtKeychain, SwitchCommitmentType};
	use crate::slate::ParticipantData;

	fn keychain_and_context() -> (ExtKeychain, Context) {
		let keychain = ExtKeychain::from_random_seed(true).unwrap();
		let parent_key_id = ExtKeychain::derive_key_id(1, 0, 0, 0, 0);
		let context = Context::new(keychain.secp(), &parent_key_id, true, true);
		(keychain, context)
	}

	#[test]
	fn add_keys_rejects_our_excess_with_a_foreign_nonce() {
		let (keychain, mut context) = keychain_and_context();
		let our_excess = PublicKey::from_secret_key(keychain.secp(), &context.sec_key).unwrap();
		let other_id = ExtKeychain::derive_key_id(1, 1, 0, 0, 0);
		let other_key = keychain
			.derive_key(0, &other_id, SwitchCommitmentType::Regular)
			.unwrap();
		let other = PublicKey::from_secret_key(keychain.secp(), &other_key).unwrap();

		let mut slate = Slate::blank(2, false);
		slate.participant_data.push(ParticipantData {
			public_blind_excess: our_excess,
			public_nonce: other,
			part_sig: None,
		});
		assert!(add_keys(&mut slate, &keychain, &mut context).is_err());
	}

	#[test]
	fn transition_state_rejects_states_it_cannot_advance() {
		for state in [
			SlateState::Unknown,
			SlateState::Standard3,
			SlateState::Invoice3,
		] {
			let mut slate = Slate::blank(2, false);
			slate.state = state.clone();
			assert!(
				transition_state(&mut slate).is_err(),
				"expected {} to be rejected",
				state
			);
		}

		// The transitions the contract flows actually use are still accepted
		for (from, to) in [
			(SlateState::Standard1, SlateState::Standard2),
			(SlateState::Standard2, SlateState::Standard3),
			(SlateState::Invoice1, SlateState::Invoice2),
			(SlateState::Invoice2, SlateState::Invoice3),
		] {
			let mut slate = Slate::blank(2, false);
			slate.state = from;
			transition_state(&mut slate).unwrap();
			assert_eq!(slate.state, to);
		}
	}
}
