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

//! Test the contract command line works as expected
#[macro_use]
extern crate clap;

#[macro_use]
extern crate log;

extern crate grin_wallet;

use grin_wallet_impls::test_framework::{self, LocalWalletClient, WalletProxy};

use clap::App;
use std::thread;
use std::time::Duration;

use grin_keychain::ExtKeychain;
use grin_wallet_impls::DefaultLCProvider;

mod common;
use common::{clean_output_dir, execute_command, initial_setup_wallet, instantiate_wallet, setup};

/// Return the single slatepack file a wallet has written
fn only_slatepack(test_dir: &str, wallet_name: &str) -> String {
	let dir = format!("{}/{}/slatepack", test_dir, wallet_name);
	let mut found: Vec<String> = std::fs::read_dir(&dir)
		.unwrap_or_else(|e| panic!("no slatepack dir {}: {}", dir, e))
		.filter_map(|e| e.ok())
		.map(|e| e.path().to_string_lossy().to_string())
		.filter(|p| p.ends_with(".slatepack"))
		.collect();
	assert_eq!(found.len(), 1, "expected one slatepack in {}", dir);
	found.pop().unwrap()
}

fn contract_command_test_impl(test_dir: &str) -> Result<(), grin_wallet_controller::Error> {
	setup(test_dir);
	let mut wallet_proxy: WalletProxy<
		DefaultLCProvider<LocalWalletClient, ExtKeychain>,
		LocalWalletClient,
		ExtKeychain,
	> = WalletProxy::new(test_dir);
	let chain = wallet_proxy.chain.clone();

	let yml = load_yaml!("../src/bin/grin-wallet.yml");
	let app = App::from_yaml(yml);

	// Two wallets, as a contract has two parties
	let arg_vec = vec!["grin-wallet", "-p", "password1", "init", "-h"];
	let client1 = LocalWalletClient::new("wallet1", wallet_proxy.tx.clone());
	execute_command(&app, test_dir, "wallet1", &client1, arg_vec)?;
	let config1 = initial_setup_wallet(test_dir, "wallet1");
	let (wallet1, mask1_i) = instantiate_wallet(
		config1.clone().members.wallet,
		client1.clone(),
		"password1",
		"default",
	)?;
	wallet_proxy.add_wallet(
		"wallet1",
		client1.get_send_instance(),
		wallet1.clone(),
		mask1_i.clone(),
	);

	let arg_vec = vec!["grin-wallet", "-p", "password2", "init", "-h"];
	let client2 = LocalWalletClient::new("wallet2", wallet_proxy.tx.clone());
	execute_command(&app, test_dir, "wallet2", &client2, arg_vec)?;
	let config2 = initial_setup_wallet(test_dir, "wallet2");
	let (wallet2, mask2_i) = instantiate_wallet(
		config2.clone().members.wallet,
		client2.clone(),
		"password2",
		"default",
	)?;
	wallet_proxy.add_wallet(
		"wallet2",
		client2.get_send_instance(),
		wallet2.clone(),
		mask2_i.clone(),
	);

	thread::spawn(move || {
		if let Err(e) = wallet_proxy.run() {
			error!("Wallet Proxy error: {}", e);
		}
	});

	// Mine into wallet 1 so it has something to send
	let mask1 = (&mask1_i).as_ref();
	let _ = test_framework::award_blocks_to_wallet(&chain, wallet1.clone(), mask1, 5, false);

	// Wallet 1 opens a contract sending 1 grin, leaving an unencrypted slatepack because
	// no --encrypt-for was given
	let arg_vec = vec![
		"grin-wallet",
		"-p",
		"password1",
		"contract",
		"new",
		"--send",
		"1",
	];
	execute_command(&app, test_dir, "wallet1", &client1, arg_vec)?;

	// Wallet 2 views the contract before signing it
	let file_name = only_slatepack(test_dir, "wallet1");
	let arg_vec = vec![
		"grin-wallet",
		"-p",
		"password2",
		"contract",
		"view",
		"-i",
		&file_name,
	];
	execute_command(&app, test_dir, "wallet2", &client2, arg_vec)?;

	// Wallet 1 can view its own contract too
	let arg_vec = vec![
		"grin-wallet",
		"-p",
		"password1",
		"contract",
		"view",
		"-i",
		&file_name,
	];
	execute_command(&app, test_dir, "wallet1", &client1, arg_vec)?;

	// A file that isn't there is reported, not panicked on
	let arg_vec = vec![
		"grin-wallet",
		"-p",
		"password1",
		"contract",
		"view",
		"-i",
		"no/such/file.slatepack",
	];
	assert!(execute_command(&app, test_dir, "wallet1", &client1, arg_vec).is_err());

	// let logging finish
	thread::sleep(Duration::from_millis(200));
	Ok(())
}

#[test]
fn wallet_contract_command_line() {
	let test_dir = "target/test_output/contract_command_line";
	if let Err(e) = contract_command_test_impl(test_dir) {
		panic!("Libwallet Error: {}", e);
	}
	clean_output_dir(test_dir);
}
