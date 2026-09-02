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

//! Implementation of contract sign

use crate::backend::WalletBackend;
use crate::contract;
use crate::contract::actions::setup;
use crate::contract::types::ContractSetupArgsAPI;
use crate::error::Error;
use crate::grin_keychain::Keychain;
use crate::grin_util::secp::key::SecretKey;
use crate::slate::Slate;
use crate::types::{Context, NodeClient};

/// Sign a contract
pub fn sign<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &Slate,
	setup_args: &ContractSetupArgsAPI,
) -> Result<Slate, Error>
where
	C: NodeClient,
	K: Keychain,
{
	// Compute if we will add outputs at this step. Only a missing context means we
	// still need to add them; propagate any other error.
	let existing_context = match w.get_private_context(keychain_mask, slate.id.as_bytes()) {
		Ok(context) => Some(context),
		Err(Error::NotFoundErr(_)) => None,
		Err(e) => return Err(e),
	};
	let will_add_outputs = existing_context
		.as_ref()
		.map(|context| context.get_inputs().len() + context.get_outputs().len() == 0)
		.unwrap_or(true);
	// Compute state for 'sign'
	let (sl, mut context) = compute(
		w,
		keychain_mask,
		slate,
		setup_args,
		existing_context.as_ref(),
	)?;

	// Atomically commit state
	contract::utils::save_step(w, keychain_mask, &sl, &mut context, will_add_outputs, true)?;

	Ok(sl)
}

/// Compute logic for sign
pub fn compute<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &Slate,
	setup_args: &ContractSetupArgsAPI,
	existing_context: Option<&Context>,
) -> Result<(Slate, Context), Error>
where
	C: NodeClient,
	K: Keychain,
{
	let mut sl = slate.clone();
	contract::utils::verify_not_signed(w, sl.id)?;

	// Ensure net_change has been provided
	let expected_net_change =
		contract::utils::get_net_change(existing_context, setup_args.net_change)?;

	// Define the values that must be provided in the setup phase at the sign step
	let mut setup_args = setup_args.clone();
	setup_args.net_change = Some(expected_net_change);
	setup_args.num_participants = sl.num_participants;
	setup_args.add_outputs = true; // we add outputs to the Context in case we haven't done that yet
	contract::slate::verify_incoming_own_commitments(w, keychain_mask, &sl)?;

	// Ensure Setup phase is done and that inputs/outputs have been added to the Context
	let (mut sl, mut context) = setup::compute(w, keychain_mask, &mut sl, &setup_args)?;
	// Add outputs to the slate, verify the payment proof and sign the slate
	contract::slate::add_outputs(w, keychain_mask, &mut sl, &context)?;
	contract::slate::verify_own_commitments(w, keychain_mask, &sl, &context)?;
	contract::slate::verify_invoice_promise(&sl, &w.keychain(keychain_mask)?, &context)?;

	contract::slate::sign(w, keychain_mask, &mut sl, &mut context)?;
	contract::slate::transition_state(&mut sl)?;

	// If we have all the partial signatures, finalize the tx
	if contract::slate::can_finalize(&sl) {
		contract::slate::finalize(w, keychain_mask, &mut sl)?;
	}

	Ok((sl, context))
}
