#![no_std]
//! Flexible Vesting for Stellar TimeLock.
//!
//! One deployed instance holds many independent vests. Each vest pays a fixed
//! daily amount (`total / days`) counted from its start timestamp. Day `i`
//! (0-based) unlocks at `start + i * 86_400`. Any remainder from the division
//! is paid with the final day.
//!
//! Owner controls, all enforced here:
//! - `skip_today`: today pays 0 and its amount moves to the end (+1 day).
//! - Double today: today pays 2x, at most 2 per vest-week (weeks counted
//!   from the vest start). Each double picks its source:
//!   - from tomorrow: tomorrow pays 0, end date unchanged;
//!   - from the end: end date moves 1 day earlier, tomorrow is normal.
//! - Optional wait (set at creation, never changeable): a double is a
//!   request, claimable after the wait, and expires 24h after the request.
//!   An expired request still counts toward the weekly limit.
//!
//! There is no early-withdrawal path, no admin, no upgrade and no pause.
//! Funds leave only through vested claims to the owner.

mod test;

use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env,
    String, Vec,
};

/// `fast-clock` is a TESTNET-ONLY build used to exercise multi-day rules in
/// minutes (1 day = 120 s, 1 hour = 5 s). Never deploy it to mainnet.
#[cfg(not(feature = "fast-clock"))]
pub const HOUR_SECS: u64 = 3_600;
#[cfg(feature = "fast-clock")]
pub const HOUR_SECS: u64 = 5;
pub const DAY_SECS: u64 = 24 * HOUR_SECS;
pub const WEEK_DAYS: u32 = 7;
pub const MAX_DOUBLES_PER_WEEK: u32 = 2;
/// A pending double expires this long after it was requested.
pub const REQUEST_TTL_SECS: u64 = 24 * HOUR_SECS;
/// Wait must end before the request expires.
pub const MAX_WAIT_HOURS: u32 = 23;
pub const MAX_DAYS: u32 = 3_650;
/// Allow a start slightly in the past (client clock vs ledger close time).
pub const START_GRACE_SECS: u64 = DAY_SECS;

const MARK_NONE: u32 = 0;
const MARK_SKIP: u32 = 1;
const MARK_DOUBLE: u32 = 2;

const LEDGERS_PER_DAY: u32 = 17_280;
const TTL_THRESHOLD: u32 = 30 * LEDGERS_PER_DAY;
const TTL_EXTEND: u32 = 180 * LEDGERS_PER_DAY;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    InvalidAmount = 1,
    InvalidDays = 2,
    InvalidStart = 3,
    InvalidWait = 4,
    VestNotFound = 5,
    NotOwner = 6,
    NotStarted = 7,
    /// Today is already skipped, doubled, zeroed by yesterday's double, or
    /// the vest has no day left to act on.
    NotAvailableToday = 8,
    /// Skipping would take back an amount that was already claimed.
    AlreadyClaimedToday = 9,
    /// Doubling needs at least one more unpaid day after today.
    NothingLeftToPull = 10,
    WeeklyLimitReached = 11,
    /// This vest has a wait: use `request_double` then `claim_double`.
    WaitRequired = 12,
    /// This vest has no wait: use `double_today`.
    NoWaitConfigured = 13,
    PendingRequestExists = 14,
    NoPendingRequest = 15,
    WaitNotOver = 16,
    RequestExpired = 17,
    NothingToClaim = 18,
    DailyAmountTooSmall = 19,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DoubleRequest {
    pub requested_at: u64,
    pub ready_at: u64,
    pub expires_at: u64,
    pub from_tomorrow: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Vest {
    pub id: u64,
    pub owner: Address,
    pub token: Address,
    pub name: String,
    pub total: i128,
    pub days: u32,
    pub daily: i128,
    pub start: u64,
    /// 0 = doubles are immediate. Immutable after creation.
    pub wait_secs: u64,
    pub withdrawn: i128,
    pub skips: u32,
    pub doubles: u32,
    /// Zeroed days (from "double from tomorrow") that are already in the past.
    pub zeros_done: u32,
    /// The most recent zeroed day; may be tomorrow.
    pub zero_day: Option<u32>,
    /// Day index of the last skip/double and which one it was.
    pub mark_day: u32,
    pub mark: u32,
    pub week: u32,
    pub week_doubles: u32,
    /// `pending` is meaningful only when `has_pending` is true.
    pub has_pending: bool,
    pub pending: DoubleRequest,
    pub created_at: u64,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestStatus {
    pub now: u64,
    pub started: bool,
    pub day: u32,
    pub vested: i128,
    pub withdrawn: i128,
    pub claimable: i128,
    /// Timestamp the final day unlocks, if no further skips/doubles happen.
    pub last_unlock: u64,
    pub skipped_today: bool,
    pub doubled_today: bool,
    pub zero_today: bool,
    pub can_skip: bool,
    pub can_double: bool,
    pub doubles_left_this_week: u32,
    pub has_pending: bool,
    pub pending: DoubleRequest,
    pub pending_ready: bool,
    pub pending_expired: bool,
    pub complete: bool,
}

#[contracttype]
#[derive(Clone)]
enum DataKey {
    NextId,
    Vest(u64),
    Owner(Address),
}

fn bump_instance(env: &Env) {
    env.storage().instance().extend_ttl(TTL_THRESHOLD, TTL_EXTEND);
}

fn load(env: &Env, id: u64) -> Result<Vest, Error> {
    let key = DataKey::Vest(id);
    let v: Vest = env
        .storage()
        .persistent()
        .get(&key)
        .ok_or(Error::VestNotFound)?;
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
    Ok(v)
}

fn save(env: &Env, v: &Vest) {
    let key = DataKey::Vest(v.id);
    env.storage().persistent().set(&key, v);
    env.storage()
        .persistent()
        .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND);
    bump_instance(env);
}

fn load_owned(env: &Env, id: u64, owner: &Address) -> Result<Vest, Error> {
    owner.require_auth();
    let v = load(env, id)?;
    if &v.owner != owner {
        return Err(Error::NotOwner);
    }
    Ok(v)
}

/// Current day index, or None before start.
fn day_of(v: &Vest, now: u64) -> Option<u32> {
    if now < v.start {
        None
    } else {
        Some(((now - v.start) / DAY_SECS) as u32)
    }
}

fn zeros_through(v: &Vest, d: u32) -> i64 {
    let pending = match v.zero_day {
        Some(z) if z <= d => 1,
        _ => 0,
    };
    v.zeros_done as i64 + pending
}

/// Units unlocked through day `d` inclusive, before capping at `days`.
fn units_uncapped(v: &Vest, d: u32) -> i64 {
    (d as i64 + 1) - v.skips as i64 + v.doubles as i64 - zeros_through(v, d)
}

fn amount_for_units(v: &Vest, units: i64) -> i128 {
    if units <= 0 {
        0
    } else if units >= v.days as i64 {
        v.total
    } else {
        v.daily * units as i128
    }
}

fn vested_at(v: &Vest, now: u64) -> i128 {
    match day_of(v, now) {
        None => 0,
        Some(d) => amount_for_units(v, units_uncapped(v, d)),
    }
}

fn marked_today(v: &Vest, d: u32) -> bool {
    v.mark != MARK_NONE && v.mark_day == d
}

/// Today pays exactly its normal share and nothing has been done to it.
fn today_is_normal(v: &Vest, d: u32) -> bool {
    !marked_today(v, d) && v.zero_day != Some(d) && units_uncapped(v, d) <= v.days as i64
}

fn doubles_left(v: &Vest, d: u32) -> u32 {
    let used = if v.week == d / WEEK_DAYS { v.week_doubles } else { 0 };
    MAX_DOUBLES_PER_WEEK.saturating_sub(used)
}

fn count_double(v: &mut Vest, d: u32) -> Result<(), Error> {
    let wk = d / WEEK_DAYS;
    if v.week != wk {
        v.week = wk;
        v.week_doubles = 0;
    }
    if v.week_doubles >= MAX_DOUBLES_PER_WEEK {
        return Err(Error::WeeklyLimitReached);
    }
    v.week_doubles += 1;
    Ok(())
}

fn check_double_eligible(v: &Vest, d: u32) -> Result<(), Error> {
    if !today_is_normal(v, d) {
        return Err(Error::NotAvailableToday);
    }
    if units_uncapped(v, d) >= v.days as i64 {
        return Err(Error::NothingLeftToPull);
    }
    Ok(())
}

fn apply_double(v: &mut Vest, d: u32, from_tomorrow: bool) -> Result<(), Error> {
    check_double_eligible(v, d)?;
    v.doubles += 1;
    v.mark_day = d;
    v.mark = MARK_DOUBLE;
    if from_tomorrow {
        if v.zero_day.is_some() {
            // Previous zero day is strictly in the past (today is not zero).
            v.zeros_done += 1;
        }
        v.zero_day = Some(d + 1);
    }
    Ok(())
}

fn pay_out(env: &Env, v: &mut Vest, now: u64) -> i128 {
    let amount = vested_at(v, now) - v.withdrawn;
    if amount > 0 {
        token::Client::new(env, &v.token).transfer(
            &env.current_contract_address(),
            &v.owner,
            &amount,
        );
        v.withdrawn += amount;
        amount
    } else {
        0
    }
}

fn empty_request() -> DoubleRequest {
    DoubleRequest {
        requested_at: 0,
        ready_at: 0,
        expires_at: 0,
        from_tomorrow: false,
    }
}

fn started_day(v: &Vest, now: u64) -> Result<u32, Error> {
    day_of(v, now).ok_or(Error::NotStarted)
}

#[contract]
pub struct FlexibleVesting;

#[contractimpl]
impl FlexibleVesting {
    /// Lock `amount` of `token` from `owner`, paying `amount / days` per day
    /// from `start`. `wait_hours` = 0 for instant doubles, else 1..=23.
    pub fn create(
        env: Env,
        owner: Address,
        token: Address,
        name: String,
        amount: i128,
        start: u64,
        days: u32,
        wait_hours: u32,
    ) -> Result<u64, Error> {
        owner.require_auth();
        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }
        if days == 0 || days > MAX_DAYS {
            return Err(Error::InvalidDays);
        }
        if wait_hours > MAX_WAIT_HOURS {
            return Err(Error::InvalidWait);
        }
        let now = env.ledger().timestamp();
        if start.saturating_add(START_GRACE_SECS) < now {
            return Err(Error::InvalidStart);
        }
        let daily = amount / days as i128;
        if daily <= 0 {
            return Err(Error::DailyAmountTooSmall);
        }

        token::Client::new(&env, &token).transfer(
            &owner,
            &env.current_contract_address(),
            &amount,
        );

        let id: u64 = env.storage().instance().get(&DataKey::NextId).unwrap_or(0);
        env.storage().instance().set(&DataKey::NextId, &(id + 1));

        let v = Vest {
            id,
            owner: owner.clone(),
            token,
            name,
            total: amount,
            days,
            daily,
            start,
            wait_secs: wait_hours as u64 * HOUR_SECS,
            withdrawn: 0,
            skips: 0,
            doubles: 0,
            zeros_done: 0,
            zero_day: None,
            mark_day: 0,
            mark: MARK_NONE,
            week: 0,
            week_doubles: 0,
            has_pending: false,
            pending: empty_request(),
            created_at: now,
        };
        save(&env, &v);

        let okey = DataKey::Owner(owner.clone());
        let mut ids: Vec<u64> = env
            .storage()
            .persistent()
            .get(&okey)
            .unwrap_or(Vec::new(&env));
        ids.push_back(id);
        env.storage().persistent().set(&okey, &ids);
        env.storage()
            .persistent()
            .extend_ttl(&okey, TTL_THRESHOLD, TTL_EXTEND);

        env.events()
            .publish((symbol_short!("create"), owner), (id, amount, days));
        Ok(id)
    }

    /// Transfer everything vested and not yet claimed to the owner.
    pub fn claim(env: Env, owner: Address, id: u64) -> Result<i128, Error> {
        let mut v = load_owned(&env, id, &owner)?;
        let now = env.ledger().timestamp();
        let paid = pay_out(&env, &mut v, now);
        if paid == 0 {
            return Err(Error::NothingToClaim);
        }
        save(&env, &v);
        env.events().publish((symbol_short!("claim"), owner), (id, paid));
        Ok(paid)
    }

    /// Today pays 0; its share moves to the end (+1 day).
    pub fn skip_today(env: Env, owner: Address, id: u64) -> Result<u64, Error> {
        let mut v = load_owned(&env, id, &owner)?;
        let now = env.ledger().timestamp();
        let d = started_day(&v, now)?;
        if !today_is_normal(&v, d) {
            return Err(Error::NotAvailableToday);
        }
        let after = amount_for_units(&v, units_uncapped(&v, d) - 1);
        if after < v.withdrawn {
            return Err(Error::AlreadyClaimedToday);
        }
        v.skips += 1;
        v.mark_day = d;
        v.mark = MARK_SKIP;
        save(&env, &v);
        env.events().publish((symbol_short!("skip"), owner), (id, d));
        Ok(last_unlock(&v))
    }

    /// Instant double (vests with no wait). Pays out everything claimable.
    pub fn double_today(
        env: Env,
        owner: Address,
        id: u64,
        from_tomorrow: bool,
    ) -> Result<i128, Error> {
        let mut v = load_owned(&env, id, &owner)?;
        if v.wait_secs != 0 {
            return Err(Error::WaitRequired);
        }
        let now = env.ledger().timestamp();
        let d = started_day(&v, now)?;
        check_double_eligible(&v, d)?;
        count_double(&mut v, d)?;
        apply_double(&mut v, d, from_tomorrow)?;
        let paid = pay_out(&env, &mut v, now);
        save(&env, &v);
        env.events()
            .publish((symbol_short!("double"), owner), (id, d, from_tomorrow, paid));
        Ok(paid)
    }

    /// Start a double on a vest with a wait. Counts toward the weekly limit
    /// immediately, even if never claimed. Returns the ready timestamp.
    pub fn request_double(
        env: Env,
        owner: Address,
        id: u64,
        from_tomorrow: bool,
    ) -> Result<u64, Error> {
        let mut v = load_owned(&env, id, &owner)?;
        if v.wait_secs == 0 {
            return Err(Error::NoWaitConfigured);
        }
        let now = env.ledger().timestamp();
        let d = started_day(&v, now)?;
        if v.has_pending && now < v.pending.expires_at {
            return Err(Error::PendingRequestExists);
        }
        check_double_eligible(&v, d)?;
        count_double(&mut v, d)?;
        let req = DoubleRequest {
            requested_at: now,
            ready_at: now + v.wait_secs,
            expires_at: now + REQUEST_TTL_SECS,
            from_tomorrow,
        };
        let ready = req.ready_at;
        v.pending = req;
        v.has_pending = true;
        save(&env, &v);
        env.events()
            .publish((symbol_short!("dbl_req"), owner), (id, d, from_tomorrow, ready));
        Ok(ready)
    }

    /// Finish a requested double after its wait. The double applies to the
    /// day it is claimed on. Pays out everything claimable.
    pub fn claim_double(env: Env, owner: Address, id: u64) -> Result<i128, Error> {
        let mut v = load_owned(&env, id, &owner)?;
        let now = env.ledger().timestamp();
        if !v.has_pending {
            return Err(Error::NoPendingRequest);
        }
        let req = v.pending.clone();
        if now >= req.expires_at {
            return Err(Error::RequestExpired);
        }
        if now < req.ready_at {
            return Err(Error::WaitNotOver);
        }
        let d = started_day(&v, now)?;
        apply_double(&mut v, d, req.from_tomorrow)?;
        v.has_pending = false;
        v.pending = empty_request();
        let paid = pay_out(&env, &mut v, now);
        save(&env, &v);
        env.events()
            .publish((symbol_short!("dbl_clm"), owner), (id, d, req.from_tomorrow, paid));
        Ok(paid)
    }

    pub fn get_vest(env: Env, id: u64) -> Result<Vest, Error> {
        load(&env, id)
    }

    pub fn list_owner(env: Env, owner: Address) -> Vec<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::Owner(owner))
            .unwrap_or(Vec::new(&env))
    }

    pub fn status(env: Env, id: u64) -> Result<VestStatus, Error> {
        let v = load(&env, id)?;
        Ok(status_of(&v, env.ledger().timestamp()))
    }
}

/// Timestamp the final day unlocks if nothing else changes.
fn last_unlock(v: &Vest) -> u64 {
    let zeros = v.zeros_done as i64 + if v.zero_day.is_some() { 1 } else { 0 };
    let last_day = v.days as i64 - 1 + v.skips as i64 - v.doubles as i64 + zeros;
    let last_day = if last_day < 0 { 0 } else { last_day as u64 };
    v.start + last_day * DAY_SECS
}

fn status_of(v: &Vest, now: u64) -> VestStatus {
    let vested = vested_at(v, now);
    let claimable = vested - v.withdrawn;
    let started = now >= v.start;
    let d = day_of(v, now).unwrap_or(0);
    let complete = v.withdrawn >= v.total;
    let (skipped, doubled, zero, can_skip, can_double, left) = if started {
        let skipped = marked_today(v, d) && v.mark == MARK_SKIP;
        let doubled = marked_today(v, d) && v.mark == MARK_DOUBLE;
        let zero = v.zero_day == Some(d);
        let normal = today_is_normal(v, d);
        let can_skip =
            normal && amount_for_units(v, units_uncapped(v, d) - 1) >= v.withdrawn;
        let left = doubles_left(v, d);
        let pending_live = v.has_pending && now < v.pending.expires_at;
        let can_double = check_double_eligible(v, d).is_ok()
            && left > 0
            && (v.wait_secs == 0 || !pending_live);
        (skipped, doubled, zero, can_skip, can_double, left)
    } else {
        (false, false, false, false, false, MAX_DOUBLES_PER_WEEK)
    };
    let p = &v.pending;
    let (pending_ready, pending_expired) = if v.has_pending {
        (now >= p.ready_at && now < p.expires_at, now >= p.expires_at)
    } else {
        (false, false)
    };
    VestStatus {
        now,
        started,
        day: d,
        vested,
        withdrawn: v.withdrawn,
        claimable,
        last_unlock: last_unlock(v),
        skipped_today: skipped,
        doubled_today: doubled,
        zero_today: zero,
        can_skip,
        can_double,
        doubles_left_this_week: left,
        has_pending: v.has_pending,
        pending: v.pending.clone(),
        pending_ready,
        pending_expired,
        complete,
    }
}
