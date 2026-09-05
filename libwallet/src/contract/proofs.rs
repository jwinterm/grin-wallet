// Copyright 2023 The Grin Developers
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

//! Experimental early payment proof functionality, currently only used
//! with contracts. Can move outside of this module if early proofs are adopted
//! by legacy transactions

use crate::backend::WalletBackend;
use crate::blake2::blake2b::blake2b;
use crate::contract::types::{ProofArgs, ProofType};
use crate::grin_core::libtx::aggsig;
use crate::grin_core::libtx::secp_ser;
use crate::grin_core::ser as grin_ser;
use crate::grin_core::ser::{Writeable, Writer};
use crate::grin_keychain::Keychain;
use crate::grin_util::secp::key::{PublicKey, SecretKey};
use crate::grin_util::secp::pedersen::Commitment;
use crate::grin_util::secp::Secp256k1;
use crate::grin_util::secp::Signature;
use crate::grin_util::static_secp_instance;
use crate::slate::{PaymentInfo, PaymentMemo, PaymentProofType, Slate};
use crate::slate_versions::ser as dalek_ser;
use crate::types::{Context, NodeClient};
use crate::{address, Error};
use byteorder::{BigEndian, ByteOrder};
use chrono::{DateTime, Utc};
use ed25519_dalek::Signature as DalekSignature;
use ed25519_dalek::SigningKey as DalekSecretKey;
use ed25519_dalek::VerifyingKey as DalekPublicKey;
use ed25519_dalek::{Signer, Verifier};
use grin_util::secp::Message;

pub(super) fn check_proof_type(proof_type: &ProofType) -> Result<(), Error> {
	match proof_type {
		ProofType::Invoice => Ok(()),
		_ => Err(Error::GenericError(
			"Only invoice contract proofs are supported".to_string(),
		)),
	}
}

fn verify_receiver_sig(
	secp: &Secp256k1,
	sig: &Signature,
	receiver_nonce: &PublicKey,
	pub_nonce_sum: &PublicKey,
	receiver_excess: &PublicKey,
	pub_blind_sum: &PublicKey,
	msg: &Message,
) -> Result<(), Error> {
	let receiver_nonce = receiver_nonce.serialize_vec(secp, true);
	if sig[0..32] != receiver_nonce[1..33] {
		return Err(Error::PaymentProofValidation(
			"Receiver nonce does not match the promise".into(),
		));
	}
	aggsig::verify_partial_sig(
		secp,
		sig,
		pub_nonce_sum,
		receiver_excess,
		Some(pub_blind_sum),
		msg,
	)?;
	Ok(())
}

/// All elements required to validate a proof within a single struct
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct ProofWitness {
	/// Kernel index, supplied so verifiers can look up kernel
	/// without an expensive lookup operation
	#[serde(with = "secp_ser::string_or_u64")]
	pub kernel_index: u64,
	/// Kernel commitment, supplied so prover can recompute index
	/// if required after a reorg
	#[serde(
		serialize_with = "secp_ser::as_hex",
		deserialize_with = "secp_ser::commitment_from_hex"
	)]
	pub kernel_commitment: Commitment,
	/// sender partial signature, used to recover receiver partial signature
	#[serde(with = "secp_ser::sig_serde")]
	pub sender_partial_sig: Signature,
}

/// Payment proof, to be extracted from slates for
/// signing (when wrapped as InvoiceProofBin) or json export from stored tx data
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct InvoiceProof {
	/// Proof type, 0x00 legacy (though this will use StoredProofInfo above, 1 invoice, 2 Sender nonce)
	#[serde(with = "crate::slate::payment_proof_type_serde")]
	pub proof_type: PaymentProofType,
	/// amount
	#[serde(with = "secp_ser::string_or_u64")]
	pub amount: u64,
	/// receiver's public nonce from signing
	#[serde(with = "secp_ser::pubkey_serde")]
	pub receiver_public_nonce: PublicKey,
	/// receiver's public excess from signing
	#[serde(with = "secp_ser::pubkey_serde")]
	pub receiver_public_excess: PublicKey,
	/// Sender's address
	#[serde(with = "dalek_ser::dalek_pubkey_serde")]
	pub sender_address: DalekPublicKey,
	/// Timestamp provided by recipient when signing
	pub timestamp: i64,
	/// Optional payment memo
	#[serde(skip_serializing_if = "Option::is_none")]
	pub memo: Option<PaymentMemo>,
	/// Not serialized in binary format
	#[serde(with = "dalek_ser::option_dalek_sig_serde")]
	pub promise_signature: Option<DalekSignature>,
	/// Not serialized in binary format, just a convenient place to insert
	/// the witness kernel commitment index
	#[serde(skip_serializing_if = "Option::is_none")]
	pub witness_data: Option<ProofWitness>,
}

struct InvoiceProofBin<'a>(&'a InvoiceProof);

impl Writeable for InvoiceProofBin<'_> {
	fn write<W: Writer>(&self, writer: &mut W) -> Result<(), grin_ser::Error> {
		writer.write_u8(self.0.proof_type.as_u8())?;

		// Amount field is 7 bytes, throw error if value is greater
		let mut amount_bytes = [0; 8];
		BigEndian::write_u64(&mut amount_bytes, self.0.amount);

		if amount_bytes[0] > 0 {
			return Err(grin_ser::Error::UnexpectedData {
				expected: [0u8].to_vec(),
				received: [amount_bytes[0]].to_vec(),
			});
		}
		writer.write_fixed_bytes(amount_bytes[1..].to_vec())?;
		{
			let static_secp = static_secp_instance();
			let static_secp = static_secp.lock();
			writer.write_fixed_bytes(
				self.0
					.receiver_public_nonce
					.serialize_vec(&static_secp, true),
			)?;
			writer.write_fixed_bytes(
				self.0
					.receiver_public_excess
					.serialize_vec(&static_secp, true),
			)?;
		}
		writer.write_fixed_bytes(self.0.sender_address.as_bytes())?;
		writer.write_i64(self.0.timestamp)?;
		let memo = self.0.memo.as_ref().map(PaymentMemo::as_str).unwrap_or("");
		writer.write_fixed_bytes(blake2b(32, &[], memo.as_bytes()).as_bytes())?;
		Ok(())
	}
}

impl InvoiceProof {
	/// Extracts as much data as possible from the slate to create an invoice proof
	pub fn from_slate(
		slate: &Slate,
		participant_index: usize,
		sender_address: Option<DalekPublicKey>,
	) -> Result<Self, Error> {
		// Bounds-check the participant index before indexing participant_data, so a
		// malformed slate returns an error rather than panicking.
		if participant_index >= slate.participant_data.len() {
			return Err(Error::GenericError(format!(
				"Participant index {} out of range for slate with {} participant(s)",
				participant_index,
				slate.participant_data.len()
			)));
		}
		// Sender address is either provided or in slate (or error)
		let sender_address = match sender_address {
			Some(a) => a,
			None => {
				if let Some(ref p) = slate.payment_proof {
					if let Some(a) = p.sender_address {
						a
					} else {
						return Err(Error::NoSenderAddressProvided);
					}
				} else {
					return Err(Error::NoSenderAddressProvided);
				}
			}
		};

		let (proof_type, timestamp) = match slate.payment_proof.as_ref() {
			Some(p) => (
				p.proof_type,
				p.timestamp
					.ok_or_else(|| Error::PaymentProof("Missing proof timestamp".to_string()))?
					.timestamp(),
			),
			None => (PaymentProofType::Invoice, 0),
		};

		let memo = match slate.payment_proof.as_ref() {
			Some(p) => p.memo.clone(),
			None => None,
		};

		let promise_signature = match slate.payment_proof.as_ref() {
			Some(p) => p.promise_signature.clone(),
			None => None,
		};

		Ok(Self {
			proof_type,
			amount: slate.amount,
			receiver_public_nonce: slate.participant_data[participant_index].public_nonce,
			receiver_public_excess: slate.participant_data[participant_index].public_blind_excess,
			sender_address,
			timestamp,
			memo,
			promise_signature,
			witness_data: None,
		})
	}

	/// Sign the invoice proof, provided all fields are populated
	pub fn sign(&self, sec_key: &SecretKey) -> Result<(DalekSignature, DalekPublicKey), Error> {
		let d_skey = DalekSecretKey::from_bytes(&sec_key.0);
		let pub_key = d_skey.verifying_key();
		let mut sig_data_bin = Vec::new();
		grin_ser::serialize_default(&mut sig_data_bin, &InvoiceProofBin(self)).map_err(|e| {
			Error::GenericError(format!("InvoiceProof serialization failed: {}", e))
		})?;

		Ok((d_skey.sign(&sig_data_bin), pub_key))
	}

	/// Verify the signature of the invoice proof
	pub fn verify_promise_signature(
		&self,
		recipient_address: &DalekPublicKey,
	) -> Result<(), Error> {
		self.proof_type.validate(PaymentProofType::Invoice)?;
		if self.promise_signature.is_none() {
			return Err(Error::PaymentProofValidation(
				"Missing promise signature".into(),
			));
		}

		// Rebuild message
		let mut sig_data_bin = Vec::new();
		grin_ser::serialize_default(&mut sig_data_bin, &InvoiceProofBin(self)).map_err(|e| {
			Error::GenericError(format!("InvoiceProof serialization failed: {}", e))
		})?;

		if recipient_address
			.verify(&sig_data_bin, self.promise_signature.as_ref().unwrap())
			.is_err()
		{
			return Err(Error::PaymentProof(
				"Invalid recipient signature".to_owned(),
			));
		};
		Ok(())
	}

	/// Verify signature and proof against a given kernel message (kernel lookup is beyond the scope
	/// of this module)
	pub fn verify_witness(
		&self,
		recipient_address: &DalekPublicKey,
		excess_sig: &Signature,
		msg: &Message,
	) -> Result<(), Error> {
		if self.witness_data.is_none() {
			return Err(Error::PaymentProofValidation("Missing witness data".into()));
		}

		self.verify_promise_signature(recipient_address)?;

		let wd = self.witness_data.as_ref().unwrap().clone();
		{
			let static_secp = static_secp_instance();
			let static_secp = static_secp.lock();

			let receiver_part_sig =
				aggsig::subtract_signature(&static_secp, &excess_sig, &wd.sender_partial_sig)?;

			// Retrieve the public nonce sum from the kernel excess signature
			let mut pub_nonce_sum_bytes = [3u8; 33];
			pub_nonce_sum_bytes[1..33].copy_from_slice(&excess_sig[0..32]);
			let pub_nonce_sum = PublicKey::from_slice(&static_secp, &pub_nonce_sum_bytes)?;

			// Retrieve the public key sum from the kernel excess
			let pub_blind_sum = wd.kernel_commitment.to_pubkey(&static_secp)?;
			if verify_receiver_sig(
				&static_secp,
				&receiver_part_sig.0,
				&self.receiver_public_nonce,
				&pub_nonce_sum,
				&self.receiver_public_excess,
				&pub_blind_sum,
				&msg,
			)
			.is_err()
			{
				// Try other possibility
				if let Some(s) = receiver_part_sig.1 {
					verify_receiver_sig(
						&static_secp,
						&s,
						&self.receiver_public_nonce,
						&pub_nonce_sum,
						&self.receiver_public_excess,
						&pub_blind_sum,
						&msg,
					)?;
				} else {
					return Err(Error::PaymentProofValidation(
						"Receiver signature does not match the promise".into(),
					));
				}
			}
		}
		Ok(())
	}
}

/// Adds all info needed for a payment proof to a slate, complete with signed recipient data
pub(super) fn add_payment_proof<C, K>(
	wallet: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &Context,
	proof_args: &ProofArgs,
) -> Result<(), Error>
where
	C: NodeClient,
	K: Keychain,
{
	let (invoice_proof, promise_signature, receiver_address) =
		generate_invoice_signature(wallet, keychain_mask, slate, context, proof_args)?;
	// Carry over the timestamp the promise signature was made over rather than reading
	// the clock a second time. The signature binds it, so a tick between the two reads
	// would leave a proof that cannot verify.
	let timestamp = DateTime::from_timestamp(invoice_proof.timestamp, 0).ok_or_else(|| {
		Error::GenericError(format!(
			"Invalid proof timestamp: {}",
			invoice_proof.timestamp
		))
	})?;

	let proof = PaymentInfo {
		proof_type: invoice_proof.proof_type,
		sender_address: proof_args.sender_address.clone(),
		receiver_address,
		timestamp: Some(timestamp),
		promise_signature: Some(promise_signature),
		memo: invoice_proof.memo,
	};
	slate.payment_proof = Some(proof);
	Ok(())
}

/// Generates a signature for proof type 'Invoice'
fn generate_invoice_signature<C, K>(
	wallet: &mut WalletBackend<C, K>,
	keychain_mask: Option<&SecretKey>,
	slate: &mut Slate,
	context: &Context,
	proof_args: &ProofArgs,
) -> Result<(InvoiceProof, DalekSignature, DalekPublicKey), Error>
where
	C: NodeClient,
	K: Keychain,
{
	let keychain = wallet.keychain(keychain_mask)?;
	let index = slate.find_index_matching_context(&keychain, context)?;
	let mut invoice_proof = InvoiceProof::from_slate(&slate, index, proof_args.sender_address)?;
	let derivation_index = match context.payment_proof_derivation_index {
		Some(i) => i,
		None => 0,
	};
	// Derive the proof address under the contract's account, not the active one.
	let parent_key_id = context.parent_key_id.clone();
	let recp_key =
		address::address_from_derivation_path(&keychain, &parent_key_id, derivation_index)?;

	invoice_proof.timestamp = Utc::now().timestamp();
	let (sig, addr) = invoice_proof.sign(&recp_key)?;
	Ok((invoice_proof, sig, addr))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::slate_versions::tests::populate_test_slate;

	#[test]
	fn rejects_unsupported_proofs() {
		assert!(check_proof_type(&ProofType::Invoice).is_ok());
		assert!(check_proof_type(&ProofType::Legacy).is_err());
		assert!(check_proof_type(&ProofType::SenderNonce).is_err());
	}

	#[test]
	fn checks_receiver_nonce() {
		let secp = Secp256k1::new();
		let sender_key = SecretKey::from_slice(&secp, &[1; 32]).unwrap();
		let receiver_key = SecretKey::from_slice(&secp, &[2; 32]).unwrap();
		let sender_nonce = SecretKey::from_slice(&secp, &[3; 32]).unwrap();
		let receiver_nonce = SecretKey::from_slice(&secp, &[4; 32]).unwrap();
		let sender_pub_nonce = PublicKey::from_secret_key(&secp, &sender_nonce).unwrap();
		let receiver_pub_nonce = PublicKey::from_secret_key(&secp, &receiver_nonce).unwrap();
		let sender_excess = PublicKey::from_secret_key(&secp, &sender_key).unwrap();
		let receiver_excess = PublicKey::from_secret_key(&secp, &receiver_key).unwrap();
		let pub_nonce_sum =
			PublicKey::from_combination(&secp, vec![&sender_pub_nonce, &receiver_pub_nonce])
				.unwrap();
		let pub_blind_sum =
			PublicKey::from_combination(&secp, vec![&sender_excess, &receiver_excess]).unwrap();
		let msg = Message::from_slice(&[5; 32]).unwrap();
		let sig = aggsig::calculate_partial_sig(
			&secp,
			&receiver_key,
			&receiver_nonce,
			&pub_nonce_sum,
			Some(&pub_blind_sum),
			&msg,
		)
		.unwrap();

		assert!(verify_receiver_sig(
			&secp,
			&sig,
			&receiver_pub_nonce,
			&pub_nonce_sum,
			&receiver_excess,
			&pub_blind_sum,
			&msg,
		)
		.is_ok());
		assert!(verify_receiver_sig(
			&secp,
			&sig,
			&sender_pub_nonce,
			&pub_nonce_sum,
			&receiver_excess,
			&pub_blind_sum,
			&msg,
		)
		.is_err());
	}

	#[test]
	fn ser_invoice_proof_bin() -> Result<(), Error> {
		let mut slate = populate_test_slate()?;
		slate.amount |= 0xFF00_0000_0000_0000;
		// Bin serialization doesn't include promise sig as it's used to create signature data
		slate.payment_proof.as_mut().unwrap().promise_signature = None;

		// Should fail, amount too big
		let invoice_proof = InvoiceProof::from_slate(&slate, 1, None)?;
		let mut vec = Vec::new();
		assert!(grin_ser::serialize_default(&mut vec, &InvoiceProofBin(&invoice_proof)).is_err());

		// Should be okay now
		slate.amount = 1234;
		let mut invoice_proof = InvoiceProof::from_slate(&slate, 1, None)?;
		let mut vec = Vec::new();
		grin_ser::serialize_default(&mut vec, &InvoiceProofBin(&invoice_proof))
			.expect("Serialization Failed");
		let memo = invoice_proof.memo.as_ref().unwrap().as_str();
		assert_eq!(
			&vec[vec.len() - 32..],
			blake2b(32, &[], memo.as_bytes()).as_bytes()
		);
		let proof_key = SecretKey::from_slice(&Secp256k1::new(), &[7; 32])?;
		let (signature, recipient) = invoice_proof.sign(&proof_key)?;
		invoice_proof.promise_signature = Some(signature);
		invoice_proof.verify_promise_signature(&recipient)?;
		invoice_proof.memo = Some(PaymentMemo::new("changed details".to_string())?);
		assert!(invoice_proof.verify_promise_signature(&recipient).is_err());

		let mut wrong_type = invoice_proof;
		wrong_type.proof_type = PaymentProofType::Legacy;
		assert!(wrong_type
			.verify_promise_signature(&slate.payment_proof.unwrap().receiver_address)
			.is_err());
		Ok(())
	}

	#[test]
	fn memo_limit() {
		assert!(PaymentMemo::new("a".repeat(PaymentMemo::MAX_LEN)).is_ok());
		assert!(PaymentMemo::new("a".repeat(PaymentMemo::MAX_LEN + 1)).is_err());
		assert!(PaymentMemo::new("ä".repeat(PaymentMemo::MAX_LEN / 2)).is_ok());
		assert!(PaymentMemo::new("ä".repeat(PaymentMemo::MAX_LEN / 2 + 1)).is_err());
	}
}
