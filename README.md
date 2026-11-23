# Stellar TimeLock smart contracts

Soroban smart contracts used by the [Stellar TimeLock](https://stellartimelock.com)
wallet for time locks and vesting on the Stellar network.

The code is public so anyone can read what the contracts do with locked funds,
build them, and run the tests. It is licensed under GPL-3.0 (see `LICENSE`).
The wallet app itself is not in this repo and stays closed source.

## Contracts in this repo

| Folder | What it does |
|---|---|
| [`flexible_vesting/`](flexible_vesting/) | Daily vesting with owner controls enforced on-chain: skip today (that day moves to the end), double today (at most 2 per vest-week, taken from tomorrow or from the end), and an optional wait of 1 to 23 hours before a double can be claimed. No early withdrawal, no admin, no upgrade. |
| [`htlc/`](htlc/) | Hash time-locked contract for native XLM. The recipient claims with the preimage before the deadline; the sender gets a refund after it. One instance holds many locks. No admin after `initialize`. |

## Deployments

| Contract | Network | Contract id | Notes |
|---|---|---|---|
| `flexible_vesting` | Testnet | [`CA7F6IZ57GRNPJZO235Z6PG4EHXN7YV72FA6TLF4WKR2RM4QO5BKF6CM`](https://stellar.expert/explorer/testnet/contract/CA7F6IZ57GRNPJZO235Z6PG4EHXN7YV72FA6TLF4WKR2RM4QO5BKF6CM) | Testnet only. Not on mainnet yet. WASM sha256 `955270360c639db8bb3aa348372a285a28217a3223040321162fb9a5a8041c21`, which a release build of this repo reproduces exactly. |
| `htlc` | Mainnet | [`CDPWT5VY4YFBUKUTZ7LKQNQ3FVJE5VSKJJD4LFBLUY4VSYYSHKAUTRCH`](https://stellar.expert/explorer/public/contract/CDPWT5VY4YFBUKUTZ7LKQNQ3FVJE5VSKJJD4LFBLUY4VSYYSHKAUTRCH) | Deployed Aug 11, 2026 (WASM sha256 `0fb3760c222c362975089dc1d0fd09b47d114793f22b1aa70c02e9da7671e284`). The source here is newer: the 10 XLM minimum for new locks and its Elite exemption were added in Oct 2026 and are not in the deployed instance. |

## Deployed contracts whose source is not here

The wallet also uses these mainnet contracts. Their source could not be found
when this repo was published, so it is not included and nothing here should be
read as their code.

| Contract | Network | Contract id |
|---|---|---|
| XLM Vault (`xlm_vault`) | Mainnet | [`CCWDMIPD4ZTTIV5LR53PD325MS6VRGF3WJEJRKNCIKK3G7H6AXJ3UE4F`](https://stellar.expert/explorer/public/contract/CCWDMIPD4ZTTIV5LR53PD325MS6VRGF3WJEJRKNCIKK3G7H6AXJ3UE4F) |
| Advanced Vaults (`multi_asset_vault_v2`) | Mainnet | [`CAPXQVGAD2TZNEDKZZDX3YKUBHQ2UI2XDI5AND6Z35D2YY2NZ7AJV6LM`](https://stellar.expert/explorer/public/contract/CAPXQVGAD2TZNEDKZZDX3YKUBHQ2UI2XDI5AND6Z35D2YY2NZ7AJV6LM) |

Native XLM SAC: mainnet `CAS3J7GYLGXMF6TDJBBYYSE3HQ6BBSMLNUQ34T6TZMYMW2EVH34XOWMA`,
testnet `CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC`.

## Build and test

Needs Rust with the `wasm32v1-none` target.

```bash
rustup target add wasm32v1-none
cargo test                                         # all contracts, native host
cargo build --release --target wasm32v1-none       # WASM in target/wasm32v1-none/release/
```

`flexible_vesting` has a `fast-clock` feature (1 day = 120 s) for exercising
multi-day rules on testnet. It is for testing only and is never deployed as a
production contract.

## Security

These contracts have not had a third-party audit. If you find a problem,
please open an issue or email stellartimelock@gmail.com.

## License

GPL-3.0. See [`LICENSE`](LICENSE).
