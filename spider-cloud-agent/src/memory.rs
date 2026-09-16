//! What this process remembers about the sites it has fetched.
//!
//! The router takes site memory as an input and keeps none of its own, which is
//! what lets per site adaptivity exist without a site name reaching any weights.
//! Somebody still has to hold the records, so the client holds them here.
//!
//! Three properties are the whole design.
//!
//! It is bounded. The table is a fixed number of slots allocated once, and a
//! new site lands in the slot its key maps to whether or not something was
//! already there. A map keyed by domain with no ceiling is a leak with extra
//! steps, and a cache that forgets the least recently used site needs ordering
//! that every reader would have to agree on.
//!
//! It does not lock. Each slot is two atomics: the key, and the whole memory
//! packed into one word. Packing is what makes a read consistent without a
//! lock, because the four fields move together or not at all. See
//! `CONTRIBUTING.md` for why a lock is not an option here.
//!
//! It is approximate on purpose. Two tasks recording against one site can race,
//! and the loser's observation is folded in on the next pass or dropped. A
//! routing hint that is one attempt stale changes which settings are tried
//! first, and the policy engine reads what actually came back.

use std::sync::atomic::{AtomicU64, Ordering};

use spider_route::{registrable_domain, AttemptOutcome, SiteMemory, StatusClass};
use url::Url;

/// How many sites are remembered when nobody says otherwise.
///
/// Two hundred and fifty six slots is about four kilobytes and covers far more
/// distinct sites than one process usually touches in a run. A caller crawling
/// more than that raises it.
pub const DEFAULT_CAPACITY: usize = 256;

/// The fewest slots an enabled store may have.
const MIN_CAPACITY: usize = 16;

/// The most slots a store may have, which is where bounded stops being a word
/// and starts being a number.
const MAX_CAPACITY: usize = 65_536;

/// How much one observation moves the success rate.
///
/// A quarter, so a site that starts failing is believed within about three
/// attempts and a single bad fetch does not rewrite the record.
const DECAY: f32 = 0.25;

/// One site's record, keyed by its registrable name.
///
/// Cheap to share: a store is allocated once and read through a shared
/// reference from any number of tasks.
#[derive(Debug)]
pub struct SiteMemoryStore {
    slots: Box<[Slot]>,
}

/// One slot of the table.
///
/// `key` is zero while the slot has never held a site. `packed` holds the
/// memory as a single word, so a reader sees four fields from one attempt
/// rather than a mixture of two.
#[derive(Debug, Default)]
struct Slot {
    key: AtomicU64,
    packed: AtomicU64,
}

impl Default for SiteMemoryStore {
    fn default() -> SiteMemoryStore {
        SiteMemoryStore::new(DEFAULT_CAPACITY)
    }
}

impl SiteMemoryStore {
    /// A store with room for about this many sites.
    ///
    /// Zero disables memory without allocating a table. Otherwise the figure
    /// is rounded up to a power of two and held between 16 and
    /// 65536, so the table can be indexed with a mask and so no caller can ask
    /// for a store that is not bounded.
    pub fn new(capacity: usize) -> SiteMemoryStore {
        if capacity == 0 {
            return SiteMemoryStore {
                slots: Box::default(),
            };
        }
        let wanted = capacity.clamp(MIN_CAPACITY, MAX_CAPACITY);
        let slots = wanted.next_power_of_two().min(MAX_CAPACITY);
        let mut table = Vec::with_capacity(slots);
        table.resize_with(slots, Slot::default);

        SiteMemoryStore {
            slots: table.into_boxed_slice(),
        }
    }

    /// How many slots the table has, which is the most sites it can remember
    /// at once.
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// How many slots hold a site right now.
    ///
    /// Never more than [`SiteMemoryStore::capacity`], which is the property
    /// worth checking.
    pub fn len(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.key.load(Ordering::Relaxed) != 0)
            .count()
    }

    /// Whether nothing has been recorded yet.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What is remembered about the site this address belongs to.
    ///
    /// `None` when nothing is, which is what a cold start looks like and what
    /// the router is built to handle.
    pub fn get(&self, url: &Url) -> Option<SiteMemory> {
        if self.slots.is_empty() {
            return None;
        }
        let key = key_of(url)?;
        let slot = self.slot(key)?;

        if slot.key.load(Ordering::Relaxed) != key {
            return None;
        }

        let memory = unpack(slot.packed.load(Ordering::Relaxed));
        if memory.observations == 0 {
            None
        } else {
            Some(memory)
        }
    }

    /// Fold one attempt into what is remembered about this site.
    ///
    /// A site that has not been seen before takes the slot its key maps to,
    /// evicting whatever was there. That is the bound: a slot per site, never a
    /// slot more.
    pub fn observe(&self, url: &Url, outcome: &AttemptOutcome) {
        self.update(url, |memory| fold(memory, outcome));
    }

    /// Write a record directly, replacing whatever was there.
    ///
    /// For a caller that keeps its own records elsewhere and wants this process
    /// to start from them rather than relearn them.
    pub fn remember(&self, url: &Url, memory: SiteMemory) {
        self.update(url, |_| memory);
    }

    /// Forget everything.
    pub fn clear(&self) {
        for slot in &self.slots {
            slot.key.store(0, Ordering::Relaxed);
            slot.packed.store(0, Ordering::Relaxed);
        }
    }

    /// Replace one site's record with what `next` makes of it.
    fn update(&self, url: &Url, next: impl Fn(SiteMemory) -> SiteMemory) {
        if self.slots.is_empty() {
            return;
        }
        let Some(key) = key_of(url) else {
            return;
        };
        let Some(slot) = self.slot(key) else {
            return;
        };

        // Claiming is a plain write rather than a compare and swap. Two tasks
        // claiming the same slot for different sites both write a key, and the
        // one that loses has recorded an attempt against a record that reads
        // as another site's. That is a stale routing hint for one call, which
        // is the same cost as the eviction this slot was going to take anyway.
        if slot.key.load(Ordering::Relaxed) != key {
            slot.key.store(key, Ordering::Relaxed);
            slot.packed.store(0, Ordering::Relaxed);
        }

        let mut current = slot.packed.load(Ordering::Relaxed);
        loop {
            let packed = pack(next(unpack(current)));
            match slot.packed.compare_exchange_weak(
                current,
                packed,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(seen) => current = seen,
            }
        }
    }

    /// The slot a key maps to.
    fn slot(&self, key: u64) -> Option<&Slot> {
        // A power of two table, so the mask is the whole of the indexing.
        let mask = self.slots.len().saturating_sub(1);
        self.slots.get((key as usize) & mask)
    }
}

/// The key for the site an address belongs to.
///
/// The registrable name, so every host under one site shares one record. A
/// literal address and a url with no host get none, because neither names a
/// site to remember.
fn key_of(url: &Url) -> Option<u64> {
    let name = registrable_domain(url)?;
    let hashed = fnv1a(name.as_bytes());

    // Zero means an empty slot, so the one site that hashes to it moves over.
    Some(if hashed == 0 { 1 } else { hashed })
}

/// A 64 bit FNV-1a, case folded on the way in.
///
/// Not a security hash and it does not need to be. Nothing outside this process
/// sees the value, and the cost of a collision is one site reading another
/// site's record until the next attempt overwrites it.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(byte.to_ascii_lowercase());
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// Fold one attempt into a record.
fn fold(memory: SiteMemory, outcome: &AttemptOutcome) -> SiteMemory {
    let scored = if outcome.success { 1.0 } else { 0.0 };
    let success_rate = if memory.observations == 0 {
        scored
    } else {
        memory.success_rate + (scored - memory.success_rate) * DECAY
    };

    // A streak counts one way or the other and never both, so a success after
    // failures starts at one rather than climbing back through zero.
    let streak = match (outcome.success, memory.streak) {
        (true, held) if held > 0 => held.saturating_add(1),
        (true, _) => 1,
        (false, held) if held < 0 => held.saturating_sub(1),
        (false, _) => -1,
    };

    SiteMemory {
        observations: memory.observations.saturating_add(1),
        success_rate: success_rate.clamp(0.0, 1.0),
        streak,
        last_status: outcome.status,
    }
}

/// How many steps the success rate is quantized to on the way into a word.
const RATE_STEPS: f32 = u16::MAX as f32;

/// Pack a record into one word.
///
/// Observations saturate at `u16::MAX` and the success rate is quantized to
/// about one part in sixty five thousand. Both are far finer than the buckets
/// the featurizer sorts them into.
fn pack(memory: SiteMemory) -> u64 {
    let observations = u64::from(memory.observations.min(u32::from(u16::MAX)) as u16);
    let rate = u64::from((memory.success_rate.clamp(0.0, 1.0) * RATE_STEPS).round() as u16);
    let streak = u64::from(memory.streak as u16);
    let status = u64::from(status_code(memory.last_status));

    observations | (rate << 16) | (streak << 32) | (status << 48)
}

/// Read a record back out of one word.
fn unpack(packed: u64) -> SiteMemory {
    let observations = (packed & 0xffff) as u32;
    let rate = ((packed >> 16) & 0xffff) as u16;
    let streak = ((packed >> 32) & 0xffff) as u16 as i16;
    let status = ((packed >> 48) & 0xff) as u8;

    SiteMemory {
        observations,
        success_rate: f32::from(rate) / RATE_STEPS,
        streak,
        last_status: status_from_code(status),
    }
}

/// The number a status class is stored as.
///
/// Written out here rather than taken from the router crate, because a stored
/// number has to keep meaning the same thing and a crate that owns an open enum
/// is free to renumber it.
fn status_code(status: StatusClass) -> u8 {
    match status {
        StatusClass::Ok => 1,
        StatusClass::Empty => 2,
        StatusClass::BadRequest => 3,
        StatusClass::NeedsLogin => 4,
        StatusClass::Blocked => 5,
        StatusClass::NotFound => 6,
        StatusClass::RateLimited => 7,
        StatusClass::ServerError => 8,
        // Including `Unknown`, and including a class added after this was
        // written. Nothing was recorded is the honest reading of both.
        _ => 0,
    }
}

/// The status class a stored number means.
fn status_from_code(code: u8) -> StatusClass {
    match code {
        1 => StatusClass::Ok,
        2 => StatusClass::Empty,
        3 => StatusClass::BadRequest,
        4 => StatusClass::NeedsLogin,
        5 => StatusClass::Blocked,
        6 => StatusClass::NotFound,
        7 => StatusClass::RateLimited,
        8 => StatusClass::ServerError,
        _ => StatusClass::Unknown,
    }
}

#[cfg(test)]
mod tests {
    // A test may unwrap and may panic: a test that cannot set itself up should
    // fail loudly rather than quietly measure nothing.
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::string_slice
    )]
    use super::*;

    fn url(raw: &str) -> Url {
        Url::parse(raw).expect("a url")
    }

    #[test]
    fn a_site_is_remembered_and_an_unseen_one_is_not() {
        let store = SiteMemoryStore::default();
        let seen = url("https://example.com/a");

        assert!(store.get(&seen).is_none());
        assert!(store.is_empty());

        store.observe(&seen, &AttemptOutcome::failed(StatusClass::Blocked, 900));
        let memory = store.get(&seen).expect("a record");

        assert_eq!(memory.observations, 1);
        assert_eq!(memory.streak, -1);
        assert_eq!(memory.last_status, StatusClass::Blocked);
        assert_eq!(memory.success_rate, 0.0);
        assert!(store.get(&url("https://other.example.org/a")).is_none());
    }

    #[test]
    fn every_host_under_one_name_shares_a_record() {
        let store = SiteMemoryStore::default();
        store.observe(
            &url("https://www.example.com/a"),
            &AttemptOutcome::failed(StatusClass::Blocked, 100),
        );
        store.observe(
            &url("https://api.example.com/b"),
            &AttemptOutcome::failed(StatusClass::Blocked, 100),
        );

        let memory = store.get(&url("https://example.com/c")).expect("a record");
        assert_eq!(memory.observations, 2);
        assert_eq!(memory.streak, -2);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn a_streak_turns_around_rather_than_counting_through_zero() {
        let store = SiteMemoryStore::default();
        let site = url("https://example.com/a");

        for _ in 0..3 {
            store.observe(&site, &AttemptOutcome::failed(StatusClass::Blocked, 10));
        }
        assert_eq!(store.get(&site).expect("a record").streak, -3);

        store.observe(&site, &AttemptOutcome::ok(2_000, 10));
        let memory = store.get(&site).expect("a record");
        assert_eq!(memory.streak, 1);
        assert_eq!(memory.observations, 4);
        assert!(memory.success_rate > 0.0);
    }

    #[test]
    fn the_table_never_grows_past_its_capacity() {
        let store = SiteMemoryStore::new(64);
        assert_eq!(store.capacity(), 64);

        for n in 0..5_000 {
            let site = url(&format!("https://site-{n}.example{n}.com/a"));
            store.observe(&site, &AttemptOutcome::ok(1_000, 20));
        }

        assert!(
            store.len() <= store.capacity(),
            "{} records in {} slots",
            store.len(),
            store.capacity()
        );
        // A table that stayed empty would satisfy the bound and prove nothing.
        assert!(store.len() > 16, "only {} slots filled", store.len());
    }

    #[test]
    fn a_capacity_is_rounded_up_and_held_inside_its_limits() {
        assert_eq!(SiteMemoryStore::new(1).capacity(), MIN_CAPACITY);
        assert_eq!(SiteMemoryStore::new(100).capacity(), 128);
        assert_eq!(SiteMemoryStore::new(usize::MAX).capacity(), MAX_CAPACITY);
    }

    #[test]
    fn zero_capacity_disables_site_memory() {
        let store = SiteMemoryStore::new(0);
        let site = url("https://example.com/a");
        assert_eq!(store.capacity(), 0);
        store.observe(&site, &AttemptOutcome::failed(StatusClass::Blocked, 10));
        store.remember(
            &site,
            SiteMemory {
                observations: 9,
                success_rate: 0.0,
                streak: -9,
                last_status: StatusClass::Blocked,
            },
        );
        assert!(store.get(&site).is_none());
        assert!(store.is_empty());
        store.clear();
        assert_eq!(store.capacity(), 0);
    }

    #[test]
    fn a_record_survives_the_trip_through_one_word() {
        let cases = [
            SiteMemory::cold(),
            SiteMemory {
                observations: 12,
                success_rate: 0.5,
                streak: -3,
                last_status: StatusClass::Blocked,
            },
            SiteMemory {
                observations: u32::MAX,
                success_rate: 1.0,
                streak: i16::MAX,
                last_status: StatusClass::RateLimited,
            },
            SiteMemory {
                observations: 1,
                success_rate: 0.0,
                streak: i16::MIN,
                last_status: StatusClass::Empty,
            },
        ];

        for memory in cases {
            let read = unpack(pack(memory));
            assert_eq!(read.streak, memory.streak, "{memory:?}");
            assert_eq!(read.last_status, memory.last_status, "{memory:?}");
            assert_eq!(
                read.observations,
                memory.observations.min(u32::from(u16::MAX)),
                "{memory:?}"
            );
            assert!(
                (read.success_rate - memory.success_rate).abs() < 0.001,
                "{memory:?} came back at {}",
                read.success_rate
            );
        }
    }

    #[test]
    fn a_literal_address_is_not_a_site_to_remember() {
        let store = SiteMemoryStore::default();
        let literal = url("http://192.0.2.1/status");

        store.observe(&literal, &AttemptOutcome::ok(10, 10));
        assert!(store.get(&literal).is_none());
        assert!(store.is_empty());
    }

    #[test]
    fn a_record_can_be_written_directly_and_cleared() {
        let store = SiteMemoryStore::default();
        let site = url("https://example.com/a");

        store.remember(
            &site,
            SiteMemory {
                observations: 9,
                success_rate: 0.1,
                streak: -4,
                last_status: StatusClass::Blocked,
            },
        );
        assert_eq!(store.get(&site).expect("a record").observations, 9);

        store.clear();
        assert!(store.get(&site).is_none());
    }

    #[test]
    fn records_from_many_threads_all_land() {
        let store = std::sync::Arc::new(SiteMemoryStore::default());
        let site = url("https://example.com/a");

        std::thread::scope(|scope| {
            for _ in 0..4 {
                let store = std::sync::Arc::clone(&store);
                let site = site.clone();
                scope.spawn(move || {
                    for _ in 0..50 {
                        store.observe(&site, &AttemptOutcome::ok(100, 10));
                    }
                });
            }
        });

        assert_eq!(store.get(&site).expect("a record").observations, 200);
    }
}
