# flexible_vesting

Daily vesting with owner controls, all enforced on-chain. Separate from
`multi_asset_vault_v2`; only new vests created with the app's "Flexible"
switch use it.

- Day `i` (0-based) unlocks `total / days` at `start + i * 86400`. The
  division remainder is paid on the last day.
- `skip_today`: today pays 0; its share moves to the end (+1 day). Refused
  once today's share was claimed, or on a day that was already changed.
- `double_today(from_tomorrow)` / `request_double` + `claim_double`: today
  pays 2x. At most 2 per vest-week (weeks counted from `start`).
  - `from_tomorrow = true`: tomorrow pays 0, end unchanged.
  - `from_tomorrow = false`: end moves 1 day earlier, tomorrow normal.
- Wait: `wait_hours` set at `create` (0 = off, else 1..23), immutable.
  With a wait, a double is requested, claimable after the wait, and expires
  24h after the request. The request counts toward the week at request time,
  so an expired one still counts. The double applies to the day it is claimed.
- No early withdrawal of the balance, no admin, no upgrade, no pause.

Entry points: `create`, `claim`, `skip_today`, `double_today`,
`request_double`, `claim_double`, `get_vest`, `list_owner`, `status`.

## Build / test

```
cargo test -p flexible_vesting
cargo build -p flexible_vesting --release --target wasm32v1-none
```

The `fast-clock` feature (1 day = 120 s, 1 hour = 5 s) is a TESTNET-ONLY
build for exercising multi-day rules live. Never deploy it to mainnet.

## Deployments

| Network | Build | Contract id | WASM sha256 |
|---|---|---|---|
| Testnet | release | `CA7F6IZ57GRNPJZO235Z6PG4EHXN7YV72FA6TLF4WKR2RM4QO5BKF6CM` | `955270360c639db8bb3aa348372a285a28217a3223040321162fb9a5a8041c21` |
| Mainnet | — | not deployed | — |
