// Copyright 2022 The Grin Developers
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

//! Test a wallet doing contract SRS flow
// #[macro_use]
extern crate grin_wallet_controller as wallet;
extern crate grin_wallet_impls as impls;
extern crate log;

use grin_wallet_libwallet as libwallet;

use grin_core::core::transaction::{CommitWrapper, Input, Inputs, OutputFeatures, Transaction};
use impls::test_framework::{self};
use libwallet::contract::my_fee_contribution;
use libwallet::contract::types::{ContractNewArgsAPI, ContractSetupArgsAPI, OwnCommitmentStatus};
use libwallet::{Slate, SlateState, TxLogEntryType};
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

#[macro_use]
mod common;
use common::{clean_output_dir, create_wallets, setup};
use std::path::PathBuf;

/// contract SRS flow
fn contract_srs_tx_impl(test_dir: &'static str) -> Result<(), libwallet::Error> {
	// create two wallets and mine 4 blocks in each (we want both to have balance to get a payjoin)
	let (wallets, chain, stopper, mut bh) =
		create_wallets(vec![vec![("default", 4)], vec![("default", 4)]], test_dir).unwrap();
	let send_wallet = wallets[0].0.clone();
	let send_mask = wallets[0].1.as_ref();
	let recv_wallet = wallets[1].0.clone();
	let recv_mask = wallets[1].1.as_ref();

	let mut slate = Slate::blank(0, true); // this gets overriden below

	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			// Send wallet inititates a standard transaction with --send=5
			let args = &mut ContractNewArgsAPI {
				setup_args: ContractSetupArgsAPI {
					selection_args: common::contract_selection_args(),
					net_change: Some(-5_000_000_000),
					..Default::default()
				},
				..Default::default()
			};
			slate = api.contract_new(m, args)?;
			Ok(())
		},
	)?;
	assert_eq!(slate.state, SlateState::Standard1);

	wallet::controller::owner_single_use(
		recv_wallet.clone(),
		recv_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let wrong_args = ContractSetupArgsAPI {
				selection_args: common::contract_selection_args(),
				net_change: Some(-5_000_000_000),
				..Default::default()
			};
			let err = api.contract_sign(m, &slate, &wrong_args).unwrap_err();
			assert!(matches!(
				err,
				libwallet::Error::GenericError(ref msg)
					if msg == "Expected net change 5000000000, got -5000000000 (did you mean --receive instead of --send?)"
			));

			// Receive wallet calls --receive=5
			let args = &mut ContractSetupArgsAPI {
				selection_args: common::contract_selection_args(),
				net_change: Some(5_000_000_000),
				..Default::default()
			};
			slate = api.contract_sign(m, &slate, args)?;
			Ok(())
		},
	)?;
	assert_eq!(slate.state, SlateState::Standard2);
	wallet::controller::owner_single_use(
		recv_wallet.clone(),
		recv_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let view = api.contract_view(m, &slate)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Clean);
			Ok(())
		},
	)?;
	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let view = api.contract_view(m, &slate)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Clean);
			let mut no_tx = slate.clone();
			no_tx.tx = None;
			let view = api.contract_view(m, &no_tx)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Unknown);
			Ok(())
		},
	)?;

	// The sender must reject an unused commitment from its wallet if it was inserted by the receiver.
	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let (_, outputs) = api.retrieve_outputs(m, false, false, None)?;
			let tx = slate.tx_or_err()?;
			let input_commits = Vec::<CommitWrapper>::from(&tx.inputs());
			let child_index = {
				let mut wallet = api.wallet_inst.lock();
				let wallet = wallet.lc_provider()?.wallet_inst()?;
				let parent = wallet.parent_key_id();
				wallet.current_child_index(&parent)?
			};
			let mut changed = slate.clone();
			changed.tx = Some(
				Transaction::new(
					Inputs::CommitOnly(input_commits.clone()),
					tx.outputs(),
					tx.kernels(),
				)
				.with_offset(tx.offset.clone()),
			);
			let err = api
				.contract_sign(m, &changed, &ContractSetupArgsAPI::default())
				.unwrap_err();
			match err {
				libwallet::Error::GenericError(message) => {
					assert_eq!(message, "Contract slate input features are missing")
				}
				err => panic!("unexpected error: {}", err),
			}
			let view = api.contract_view(m, &changed)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Unknown);
			let current_child_index = {
				let mut wallet = api.wallet_inst.lock();
				let wallet = wallet.lc_provider()?.wallet_inst()?;
				let parent = wallet.parent_key_id();
				wallet.current_child_index(&parent)?
			};
			assert_eq!(current_child_index, child_index);

			let (output, pos) = outputs
				.iter()
				.find_map(|output| {
					let unused = !input_commits
						.iter()
						.any(|input| input.commitment() == output.commit)
						&& !tx
							.outputs()
							.iter()
							.any(|tx_output| tx_output.commitment() == output.commit);
					if unused {
						chain
							.get_output_pos(&output.commit)
							.ok()
							.map(|pos| (output, pos))
					} else {
						None
					}
				})
				.expect("sender has an unused confirmed output");
			let features = if output.output.is_coinbase {
				OutputFeatures::Coinbase
			} else {
				OutputFeatures::Plain
			};
			let mut changed = slate.clone();
			changed.tx = Some(
				changed
					.tx_or_err()?
					.clone()
					.with_input(Input::new(features, output.commit)),
			);
			let view = api.contract_view(m, &changed)?;
			assert_eq!(
				view.own_commitment_status,
				OwnCommitmentStatus::UnexpectedInput
			);
			let err = api
				.contract_sign(m, &changed, &ContractSetupArgsAPI::default())
				.unwrap_err();
			match err {
				libwallet::Error::GenericError(message) => assert_eq!(
					message,
					"Contract slate contains an unexpected input commitment from this wallet"
				),
				err => panic!("unexpected error: {}", err),
			}

			let mut changed = slate.clone();
			changed.tx = Some(
				changed
					.tx_or_err()?
					.clone()
					.with_output(chain.get_unspent_output_at(pos).unwrap()),
			);
			let view = api.contract_view(m, &changed)?;
			assert_eq!(
				view.own_commitment_status,
				OwnCommitmentStatus::UnexpectedOutput
			);
			let err = api
				.contract_sign(m, &changed, &ContractSetupArgsAPI::default())
				.unwrap_err();
			match err {
				libwallet::Error::GenericError(message) => assert_eq!(
					message,
					"Contract slate contains an unexpected output commitment from this wallet"
				),
				err => panic!("unexpected error: {}", err),
			}

			let mut changed = slate.clone();
			changed.tx = Some(
				changed
					.tx_or_err()?
					.clone()
					.with_input(Input::new(features, output.commit))
					.with_output(chain.get_unspent_output_at(pos).unwrap()),
			);
			let view = api.contract_view(m, &changed)?;
			assert_eq!(
				view.own_commitment_status,
				OwnCommitmentStatus::UnexpectedInputAndOutput
			);
			let err = api
				.contract_sign(m, &changed, &ContractSetupArgsAPI::default())
				.unwrap_err();
			match err {
				libwallet::Error::GenericError(message) => assert_eq!(
					message,
					"Contract slate contains unexpected input and output commitments from this wallet"
				),
				err => panic!("unexpected error: {}", err),
			}
			Ok(())
		},
	)?;

	// Send wallet finalizes and posts
	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let args = &mut ContractSetupArgsAPI {
				selection_args: common::contract_selection_args(),
				..Default::default()
			};
			slate = api.contract_sign(m, &slate, args)?;
			Ok(())
		},
	)?;
	assert_eq!(slate.state, SlateState::Standard3);
	wallet::controller::owner_single_use(
		recv_wallet.clone(),
		recv_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let view = api.contract_view(m, &slate)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Clean);
			Ok(())
		},
	)?;
	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let view = api.contract_view(m, &slate)?;
			assert_eq!(view.own_commitment_status, OwnCommitmentStatus::Unknown);
			Ok(())
		},
	)?;

	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			api.post_tx(m, &slate, false)?;
			Ok(())
		},
	)?;
	bh += 1;

	let _ =
		test_framework::award_blocks_to_wallet(&chain, send_wallet.clone(), send_mask, 3, false);
	bh += 3;

	// Assert changes in receive wallet
	wallet::controller::owner_single_use(
		recv_wallet.clone(),
		recv_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let (_, wallet_info) = api.retrieve_summary_info(m, true, 1)?;
			let (refreshed, txs) = api.retrieve_txs(m, true, None, None, None)?;
			assert_eq!(wallet_info.last_confirmed_height, bh);
			assert!(refreshed);
			assert_eq!(txs.len(), 5); // 4 mined and 1 received
			let tx_log = txs[4].clone();
			assert_eq!(tx_log.tx_type, TxLogEntryType::TxReceived);
			assert_eq!(tx_log.amount_credited, 5_000_000_000);
			assert_eq!(tx_log.amount_debited, 0);
			assert_eq!(tx_log.num_inputs, 1);
			assert_eq!(tx_log.num_outputs, 1);
			let expected_fees_paid = Some(my_fee_contribution(1, 1, 1, 2)?);
			assert_eq!(tx_log.fee, expected_fees_paid);
			assert_eq!(
				wallet_info.amount_currently_spendable,
				4 * 60_000_000_000 + 5_000_000_000 - expected_fees_paid.unwrap().fee() // we expect the balance of 4 mined blocks + 5 Grin - fees paid
			);
			Ok(())
		},
	)?;

	// Assert changes in send wallet
	wallet::controller::owner_single_use(
		send_wallet.clone(),
		send_mask,
		PathBuf::from(test_dir),
		|api, m| {
			let (_, wallet_info) = api.retrieve_summary_info(m, true, 1)?;
			let (refreshed, txs) = api.retrieve_txs(m, true, None, None, None)?;
			assert_eq!(wallet_info.last_confirmed_height, bh);
			assert!(refreshed);
			assert_eq!(txs.len() as u64, bh - 4 + 1); // send wallet didn't mine 4 blocks and made 1 tx
			let tx_log = txs[txs.len() - 5].clone(); // TODO: why -5 and not -4?
			assert_eq!(tx_log.tx_type, TxLogEntryType::TxSent);
			assert_eq!(tx_log.amount_credited, 0);
			assert_eq!(tx_log.amount_debited, 5_000_000_000);
			assert_eq!(tx_log.num_inputs, 1);
			assert_eq!(tx_log.num_outputs, 1);
			assert_eq!(tx_log.fee, Some(my_fee_contribution(1, 1, 1, 2)?));
			Ok(())
		},
	)?;

	// let logging finish
	stopper.store(false, Ordering::Relaxed);
	thread::sleep(Duration::from_millis(200));

	Ok(())
}

#[test]
fn wallet_contract_srs_tx() -> Result<(), libwallet::Error> {
	let test_dir = "test_output/contract_srs_tx";
	setup(test_dir);
	contract_srs_tx_impl(test_dir)?;
	clean_output_dir(test_dir);
	Ok(())
}
