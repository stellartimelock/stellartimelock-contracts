#![cfg(test)]
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, Env, String,
};

use crate::{Error, FlexibleVesting, FlexibleVestingClient, DAY_SECS};

const T0: u64 = 1_800_000_000;
const HOUR: u64 = 3_600;

struct Ctx {
    env: Env,
    c: FlexibleVestingClient<'static>,
    tok: token::Client<'static>,
    owner: Address,
}

fn setup() -> Ctx {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(T0);
    let admin = Address::generate(&env);
    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(admin).address();
    token::StellarAssetClient::new(&env, &sac).mint(&owner, &1_000_000_000_000);
    let id = env.register(FlexibleVesting, ());
    let c = FlexibleVestingClient::new(&env, &id);
    let tok = token::Client::new(&env, &sac);
    // Leak env-bound clients for test convenience.
    let c: FlexibleVestingClient<'static> = unsafe { core::mem::transmute(c) };
    let tok: token::Client<'static> = unsafe { core::mem::transmute(tok) };
    Ctx { env, c, tok, owner }
}

impl Ctx {
    fn create(&self, amount: i128, days: u32, wait_hours: u32) -> u64 {
        self.c.create(
            &self.owner,
            &self.tok.address,
            &String::from_str(&self.env, "v"),
            &amount,
            &T0,
            &days,
            &wait_hours,
        )
    }
    fn at(&self, secs: u64) {
        self.env.ledger().set_timestamp(T0 + secs);
    }
    fn day(&self, d: u64) {
        self.at(d * DAY_SECS + 60);
    }
    fn vested(&self, id: u64) -> i128 {
        self.c.status(&id).vested
    }
    fn bal(&self) -> i128 {
        self.tok.balance(&self.owner)
    }
}

const START_BAL: i128 = 1_000_000_000_000;

#[test]
fn linear_daily_from_start() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    assert_eq!(t.bal(), START_BAL - 1_000);
    t.at(0);
    assert_eq!(t.vested(id), 100);
    t.day(3);
    assert_eq!(t.vested(id), 400);
    assert_eq!(t.c.claim(&t.owner, &id), 400);
    assert_eq!(t.c.try_claim(&t.owner, &id), Err(Ok(Error::NothingToClaim)));
    t.day(9);
    assert_eq!(t.vested(id), 1_000);
    assert_eq!(t.c.status(&id).last_unlock, T0 + 9 * DAY_SECS);
    assert_eq!(t.c.claim(&t.owner, &id), 600);
    assert_eq!(t.bal(), START_BAL);
    assert!(t.c.status(&id).complete);
}

#[test]
fn remainder_paid_on_last_day() {
    let t = setup();
    let id = t.create(1_003, 10, 0);
    t.day(8);
    assert_eq!(t.vested(id), 900);
    t.day(9);
    assert_eq!(t.vested(id), 1_003);
}

#[test]
fn nothing_before_start() {
    let t = setup();
    let id = t.c.create(
        &t.owner,
        &t.tok.address,
        &String::from_str(&t.env, "later"),
        &700,
        &(T0 + 5 * DAY_SECS),
        &7,
        &0,
    );
    assert_eq!(t.vested(id), 0);
    assert_eq!(t.c.try_claim(&t.owner, &id), Err(Ok(Error::NothingToClaim)));
    assert_eq!(t.c.try_skip_today(&t.owner, &id), Err(Ok(Error::NotStarted)));
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &true),
        Err(Ok(Error::NotStarted))
    );
    assert!(!t.c.status(&id).started);
}

#[test]
fn skip_moves_today_to_end() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    t.day(2);
    assert_eq!(t.c.skip_today(&t.owner, &id), T0 + 10 * DAY_SECS);
    assert_eq!(t.vested(id), 200);
    let s = t.c.status(&id);
    assert!(s.skipped_today && !s.can_skip && !s.can_double);
    assert_eq!(
        t.c.try_skip_today(&t.owner, &id),
        Err(Ok(Error::NotAvailableToday))
    );
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &false),
        Err(Ok(Error::NotAvailableToday))
    );
    t.day(3);
    assert_eq!(t.vested(id), 300);
    t.day(9);
    assert_eq!(t.vested(id), 900);
    t.day(10);
    assert_eq!(t.vested(id), 1_000);
}

#[test]
fn cannot_skip_after_claiming_today() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    t.day(1);
    t.c.claim(&t.owner, &id);
    assert!(!t.c.status(&id).can_skip);
    assert_eq!(
        t.c.try_skip_today(&t.owner, &id),
        Err(Ok(Error::AlreadyClaimedToday))
    );
}

#[test]
fn skip_last_day_extends() {
    let t = setup();
    let id = t.create(1_005, 5, 0);
    t.day(4);
    t.c.skip_today(&t.owner, &id);
    assert_eq!(t.vested(id), 804);
    t.day(5);
    assert_eq!(t.vested(id), 1_005);
    // After the end there is nothing left to skip.
    t.day(6);
    assert_eq!(
        t.c.try_skip_today(&t.owner, &id),
        Err(Ok(Error::NotAvailableToday))
    );
}

#[test]
fn double_from_end_pays_twice_and_ends_sooner() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    t.day(1);
    let before = t.bal();
    assert_eq!(t.c.double_today(&t.owner, &id, &false), 300);
    assert_eq!(t.bal(), before + 300);
    assert_eq!(t.c.status(&id).last_unlock, T0 + 8 * DAY_SECS);
    assert!(t.c.status(&id).doubled_today);
    t.day(2);
    assert_eq!(t.vested(id), 400); // tomorrow normal
    t.day(8);
    assert_eq!(t.vested(id), 1_000);
}

#[test]
fn double_from_tomorrow_zeroes_tomorrow_keeps_end() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    t.day(1);
    assert_eq!(t.c.double_today(&t.owner, &id, &true), 300);
    assert_eq!(t.c.status(&id).last_unlock, T0 + 9 * DAY_SECS);
    t.day(2);
    let s = t.c.status(&id);
    assert!(s.zero_today && !s.can_skip && !s.can_double);
    assert_eq!(s.vested, 300);
    assert_eq!(
        t.c.try_skip_today(&t.owner, &id),
        Err(Ok(Error::NotAvailableToday))
    );
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &false),
        Err(Ok(Error::NotAvailableToday))
    );
    t.day(3);
    assert_eq!(t.vested(id), 400);
    t.day(8);
    assert_eq!(t.vested(id), 900);
    t.day(9);
    assert_eq!(t.vested(id), 1_000);
}

#[test]
fn two_doubles_from_tomorrow_track_zero_days() {
    let t = setup();
    let id = t.create(1_400, 14, 0);
    t.day(1);
    t.c.double_today(&t.owner, &id, &true); // day 2 zero
    t.day(3);
    t.c.double_today(&t.owner, &id, &true); // day 4 zero
    t.day(4);
    assert_eq!(t.vested(id), 500);
    t.day(5);
    assert_eq!(t.vested(id), 600);
    t.day(13);
    assert_eq!(t.vested(id), 1_400);
    assert_eq!(t.c.status(&id).last_unlock, T0 + 13 * DAY_SECS);
}

#[test]
fn weekly_cap_two_per_vest_week() {
    let t = setup();
    let id = t.create(3_000, 30, 0);
    t.day(0);
    t.c.double_today(&t.owner, &id, &false);
    t.day(1);
    t.c.double_today(&t.owner, &id, &false);
    t.day(2);
    assert_eq!(t.c.status(&id).doubles_left_this_week, 0);
    assert!(!t.c.status(&id).can_double);
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &false),
        Err(Ok(Error::WeeklyLimitReached))
    );
    t.day(6);
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &true),
        Err(Ok(Error::WeeklyLimitReached))
    );
    t.day(7); // week 1 from the vest start
    assert_eq!(t.c.status(&id).doubles_left_this_week, 2);
    t.c.double_today(&t.owner, &id, &false);
    assert_eq!(t.c.status(&id).doubles_left_this_week, 1);
}

#[test]
fn double_on_last_day_rejected() {
    let t = setup();
    let id = t.create(500, 5, 0);
    t.day(4);
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &false),
        Err(Ok(Error::NothingLeftToPull))
    );
}

#[test]
fn wait_vest_requires_request_then_claim() {
    let t = setup();
    let id = t.create(1_000, 10, 4);
    t.day(1);
    assert_eq!(
        t.c.try_double_today(&t.owner, &id, &false),
        Err(Ok(Error::WaitRequired))
    );
    assert_eq!(t.c.try_claim_double(&t.owner, &id), Err(Ok(Error::NoPendingRequest)));
    let now = DAY_SECS + 60;
    assert_eq!(t.c.request_double(&t.owner, &id, &false), T0 + now + 4 * HOUR);
    assert_eq!(
        t.c.try_request_double(&t.owner, &id, &true),
        Err(Ok(Error::PendingRequestExists))
    );
    assert!(!t.c.status(&id).can_double);
    t.at(now + 4 * HOUR - 1);
    assert_eq!(t.c.try_claim_double(&t.owner, &id), Err(Ok(Error::WaitNotOver)));
    t.at(now + 4 * HOUR);
    assert!(t.c.status(&id).pending_ready);
    assert_eq!(t.c.claim_double(&t.owner, &id), 300);
    assert!(!t.c.status(&id).has_pending);
    assert_eq!(t.c.status(&id).last_unlock, T0 + 8 * DAY_SECS);
}

#[test]
fn request_expires_after_24h_and_still_counts() {
    let t = setup();
    let id = t.create(3_000, 30, 4);
    t.day(0);
    t.c.request_double(&t.owner, &id, &true);
    t.at(24 * HOUR + 60);
    let s = t.c.status(&id);
    assert!(s.pending_expired && !s.pending_ready);
    assert_eq!(t.c.try_claim_double(&t.owner, &id), Err(Ok(Error::RequestExpired)));
    assert_eq!(s.doubles_left_this_week, 1);
    // A new request replaces the expired one.
    t.c.request_double(&t.owner, &id, &false);
    t.at(24 * HOUR + 60 + 4 * HOUR);
    t.c.claim_double(&t.owner, &id);
    t.day(3);
    assert_eq!(
        t.c.try_request_double(&t.owner, &id, &false),
        Err(Ok(Error::WeeklyLimitReached))
    );
    // Only the claimed double changed the schedule.
    assert_eq!(t.vested(id), 500);
}

#[test]
fn claimed_double_applies_to_claim_day_across_midnight() {
    let t = setup();
    let id = t.create(1_000, 10, 4);
    t.at(DAY_SECS + 22 * HOUR); // day 1, 22:00
    t.c.request_double(&t.owner, &id, &true);
    t.at(2 * DAY_SECS + 2 * HOUR); // day 2, 02:00
    assert_eq!(t.c.claim_double(&t.owner, &id), 400);
    t.day(3);
    assert_eq!(t.vested(id), 400); // day 3 zeroed
}

#[test]
fn no_wait_vest_rejects_request() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    t.day(1);
    assert_eq!(
        t.c.try_request_double(&t.owner, &id, &false),
        Err(Ok(Error::NoWaitConfigured))
    );
}

#[test]
fn only_owner_can_act() {
    let t = setup();
    let id = t.create(1_000, 10, 0);
    let other = Address::generate(&t.env);
    t.day(1);
    assert_eq!(t.c.try_claim(&other, &id), Err(Ok(Error::NotOwner)));
    assert_eq!(t.c.try_skip_today(&other, &id), Err(Ok(Error::NotOwner)));
    assert_eq!(t.c.try_double_today(&other, &id, &true), Err(Ok(Error::NotOwner)));
    assert_eq!(t.c.try_claim(&t.owner, &99), Err(Ok(Error::VestNotFound)));
}

#[test]
fn create_validation() {
    let t = setup();
    let name = String::from_str(&t.env, "x");
    let tk = t.tok.address.clone();
    let o = t.owner.clone();
    assert_eq!(t.c.try_create(&o, &tk, &name, &0, &T0, &10, &0), Err(Ok(Error::InvalidAmount)));
    assert_eq!(t.c.try_create(&o, &tk, &name, &100, &T0, &0, &0), Err(Ok(Error::InvalidDays)));
    assert_eq!(t.c.try_create(&o, &tk, &name, &100, &T0, &3_651, &0), Err(Ok(Error::InvalidDays)));
    assert_eq!(t.c.try_create(&o, &tk, &name, &100, &T0, &10, &24), Err(Ok(Error::InvalidWait)));
    assert_eq!(t.c.try_create(&o, &tk, &name, &5, &T0, &10, &0), Err(Ok(Error::DailyAmountTooSmall)));
    t.at(2 * DAY_SECS);
    assert_eq!(t.c.try_create(&o, &tk, &name, &100, &T0, &10, &0), Err(Ok(Error::InvalidStart)));
    let id = t.c.create(&o, &tk, &name, &100, &(T0 + 2 * DAY_SECS), &10, &23);
    assert_eq!(t.c.get_vest(&id).wait_secs, 23 * HOUR);
    assert_eq!(t.c.list_owner(&o).len(), 1);
}

/// Whatever mix of actions happens, the owner never gets more than the total,
/// never gets anything ahead of the schedule, and always ends with the total.
#[test]
fn mixed_actions_conserve_total() {
    let t = setup();
    let id = t.create(10_007, 20, 0);
    let mut d = 0u64;
    let pattern = [0u8, 1, 2, 3, 0, 1, 0, 2, 3, 1, 0, 0, 2, 1, 3, 0];
    for p in pattern.iter() {
        t.day(d);
        match p {
            1 => {
                let _ = t.c.try_skip_today(&t.owner, &id);
            }
            2 => {
                let _ = t.c.try_double_today(&t.owner, &id, &false);
            }
            3 => {
                let _ = t.c.try_double_today(&t.owner, &id, &true);
            }
            _ => {
                let _ = t.c.try_claim(&t.owner, &id);
            }
        }
        let s = t.c.status(&id);
        assert!(s.withdrawn <= s.vested && s.vested <= 10_007);
        d += 1;
    }
    t.day(60);
    let _ = t.c.try_claim(&t.owner, &id);
    assert_eq!(t.bal(), START_BAL);
    assert_eq!(t.tok.balance(&t.c.address), 0);
}
