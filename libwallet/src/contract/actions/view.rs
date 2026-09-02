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

//! Implementation of contract view

use crate::backend::WalletBackend;
use crate::contract;
use crate::contract::types::{ContractView, OwnCommitmentStatus};
use crate::error::Error;
use crate::grin_keychain::Keychain;
use crate::grin_util::secp::key::SecretKey;
use crate::internal::updater;
use crate::slate::{Slate, SlateState};
use crate::types::NodeClient;

fn suggested_net_change(state: &SlateState, amount: u64) -> Result<Option<i64>, Error> {
	let sign = match state {
		SlateState::Standard1 => 1,
		SlateState::Invoice1 => -1,
		_ => return Ok(None),
	};
	let amount = i64::try_from(amount)
		.map_err(|_| Error::GenericError(format!("Slate amount {} exceeds i64", amount)))?;
	Ok(Some(sign * amount))
}

/// View contract
pub fn view<C, K>(
	w: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	_encrypted_for: &str,
) -> Result<ContractView, Error>
where
	C: NodeClient,
	K: Keychain,
{
	// NOTE: This should only be run on slates that we received and were signed for us.
	// Otherwise, you can't really predict who the party doing the next step should be.

	// Reject a slate we can't interpret. Standard2/3 and Invoice2/3 are valid mid/late
	// flow states (they fall through to suggested_net_change = None below), so only the
	// Unknown state is rejected here.
	if slate.state == SlateState::Unknown {
		return Err(Error::GenericError(
			"Cannot view a slate with an Unknown state".to_string(),
		));
	}
	// Mirror the contract setup bound so a tampered slate can't surface a bogus count.
	if slate.num_participants < 1 || slate.num_participants > 2 {
		return Err(Error::GenericError(format!(
			"Unsupported num_participants: {} (expected 1 or 2)",
			slate.num_participants
		)));
	}
	let context = match w.get_private_context(keychain_mask, slate.id.as_bytes()) {
		Ok(context) => Some(context),
		Err(Error::NotFoundErr(_)) => None,
		Err(e) => return Err(e),
	};
	let suggested_net_change = suggested_net_change(&slate.state, slate.amount)?;
	// A contract is executed once the transaction it produced has confirmed. That is
	// recorded on our own tx log entry for this slate, so no chain lookup is needed; a
	// slate we have never signed simply has no entry and is not executed.
	let txs = updater::retrieve_txs(w, None, Some(slate.id), None, None, false)?;
	let is_executed = txs.iter().any(|tx| tx.confirmed);
	let has_signed = txs
		.iter()
		.any(|tx| contract::utils::is_signed_tx(tx, slate.id));
	// Once we have signed, the context identifies the commitments we added. If it has
	// already been removed, the slate no longer carries enough information to do that.
	let own_commitment_status = if slate.tx.is_none() || (context.is_none() && !txs.is_empty()) {
		OwnCommitmentStatus::Unknown
	} else {
		contract::slate::own_commitment_status(
			w,
			keychain_mask,
			slate,
			if has_signed { context.as_ref() } else { None },
		)?
	};
	// Count signatures present (a participant is "complete" once it has a partial sig).
	let num_sigs = slate
		.participant_data
		.clone()
		.into_iter()
		.filter(|v| v.is_complete())
		.count();

	// If we have a local context for this slate we've agreed on a net change; surface it.
	let agreed_net_change = context
		.as_ref()
		.and_then(|context| context.setup_args.as_ref())
		.and_then(|args| args.net_change);

	// TODO: Maybe we can know if the slate was meant for us if it was encrypted for us.
	// A possible issue is that one can encrypt the same slate for 10 people.
	let ct_view = ContractView {
		num_participants: slate.num_participants,
		suggested_net_change: suggested_net_change,
		agreed_net_change,
		num_sigs: num_sigs as u8,
		is_executed: is_executed,
		own_commitment_status,
		..Default::default()
	};
	Ok(ct_view)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn suggested_change_direction() {
		assert_eq!(
			suggested_net_change(&SlateState::Standard1, 10).unwrap(),
			Some(10)
		);
		assert_eq!(
			suggested_net_change(&SlateState::Invoice1, 10).unwrap(),
			Some(-10)
		);
		assert_eq!(
			suggested_net_change(&SlateState::Standard2, 10).unwrap(),
			None
		);
	}
}
