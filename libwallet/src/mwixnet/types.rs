// Copyright 2024 The Grin Developers
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

//! Types related to mwixnet requests required by rest of lib crate apis
//! Should rexport all needed types here

use super::onion::comsig_serde;
use grin_core::libtx::secp_ser::string_or_u64;
use serde::{Deserialize, Serialize};
use x25519_dalek::PublicKey as xPublicKey;

pub use super::onion::{onion::Onion, ComSignature, Hop};

/// A Swap request
#[derive(Serialize, Deserialize, Debug)]
pub struct SwapReq {
	/// Com signature
	#[serde(with = "comsig_serde")]
	pub comsig: ComSignature,
	/// Onion
	pub onion: Onion,
}

/// mwixnetRequest Creation Params
#[derive(Serialize, Deserialize, Debug)]
pub struct MixnetReqCreationParams {
	/// x25519 onion public keys of the mix servers, in route order
	#[serde(with = "vec_xpubkey_hex")]
	pub server_pubkeys: Vec<xPublicKey>,
	/// Fees per hop
	#[serde(with = "string_or_u64")]
	pub fee_per_hop: u64,
}

/// Serializes a Vec<x25519 PublicKey> as a list of hex strings.
pub mod vec_xpubkey_hex {
	use grin_util::{from_hex, ToHex};
	use serde::de::Error;
	use serde::{Deserialize, Deserializer, Serializer};
	use x25519_dalek::PublicKey as xPublicKey;

	///
	pub fn serialize<S: Serializer>(keys: &Vec<xPublicKey>, s: S) -> Result<S::Ok, S::Error> {
		let hexes: Vec<String> = keys.iter().map(|k| k.as_bytes().to_hex()).collect();
		serde::Serialize::serialize(&hexes, s)
	}

	///
	pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<xPublicKey>, D::Error> {
		let hexes: Vec<String> = Deserialize::deserialize(d)?;
		let mut keys = Vec::with_capacity(hexes.len());
		for h in hexes {
			let bytes = from_hex(&h).map_err(Error::custom)?;
			let arr: [u8; 32] = bytes
				.as_slice()
				.try_into()
				.map_err(|_| Error::custom("x25519 pubkey must be 32 bytes"))?;
			keys.push(xPublicKey::from(arr));
		}
		Ok(keys)
	}
}
