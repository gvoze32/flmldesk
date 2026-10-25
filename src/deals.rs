//! Agreed transfers that wait for the player to arrive, written the way
//! Football Life 2026 stores a deal agreed in the game. On the arrival date the
//! game itself moves the player, completes the contract and clears the lists.
//!
//! Decoded from two negotiations saved at every step. Offsets marked
//! "table +" are relative to contract slot 0 (`ContractTable::first`), which
//! the game always precedes with a row whose w8 is `0x74574`:
//!
//! | Data | Location | Row |
//! |---|---|---|
//! | Offers | table + `0xD5C0` (after 1140 contract slots), 60 bytes | id, buying club, state (4 = agreed), arrival date, 3 filler words, selling club, internal id, player id, fee, fee, `0xFFFF`, `0xFFFF`, 0 |
//! | Agreed deals | table + `0x47720`, 6 bytes | u16 offer id, u16 contract index, `0xFFFF` |
//! | Pending arrivals | `0xC81870`, 12 bytes | internal id, player id, salary units |
//! | Season deals | `0xC803E0`, 52 bytes | internal id, player id, six zeros, minus the fee (i32), 1, 0, 0, `0xFFFF` |
//!
//! Offer ids and contract indexes share one numbering: a new negotiation takes
//! the lowest free number as offer id and the next free one as contract index.
//! The fee is in units of 100 EUR everywhere. The game also writes a news item
//! and a negotiation timeline; neither is needed for the arrival, so they are
//! left alone. Every table is checked before writing; anything unexpected
//! fails closed.

use std::collections::BTreeSet;

use crate::Error;
use crate::contracts::{ContractTable, Date, RECORD, words};
use crate::masterleague::Player;

const HEADER_MAGIC: u32 = 0x0007_4574;
/// Offers start after the 1140 contract slots (offset from contract slot 0).
pub const OFFERS_OFFSET: usize = 1140 * RECORD;
pub const OFFER_RECORD: usize = 60;
const MAX_OFFERS: usize = 400;
const OFFER_AGREED: u32 = 4;
/// Blank offer row as written by the game.
pub const BLANK_OFFER: [u32; 15] =
    [0xFFFF, 0xFFFF_FFFF, 0, 0xFFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, 0xFFFF_FFFF, 0xFFFF, 0, 0, 0, 0xFFFF, 0xFFFF, 0];
pub const AGREED_OFFSET: usize = 0x47720;
pub const AGREED_ENTRY: usize = 6;
const MAX_AGREED: usize = 100;
pub const PENDING_ARRIVALS: usize = 0xC8_1870;
pub const PENDING_ENTRY: usize = 12;
const MAX_PENDING: usize = 64;
pub const SEASON_DEALS: usize = 0xC8_03E0;
pub const DEAL_ENTRY: usize = 52;
const MAX_DEALS: usize = 100;
const NONE16: u16 = 0xFFFF;

fn u32s<const N: usize>(data: &[u8], off: usize) -> Option<[u32; N]> {
    let bytes = data.get(off..off.checked_add(4 * N)?)?;
    Some(core::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())))
}

fn u16s<const N: usize>(data: &[u8], off: usize) -> Option<[u16; N]> {
    let bytes = data.get(off..off.checked_add(2 * N)?)?;
    Some(core::array::from_fn(|i| u16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]])))
}

fn encode<const N: usize>(words: [u32; N]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

fn is_offer_blank(w: &[u32; 15]) -> bool {
    w[0] & 0xFFFF == 0xFFFF && w[1] == 0xFFFF_FFFF && w[7] == 0xFFFF_FFFF && w[9] == 0 && w[10] == 0 && w[12] & 0xFFFF == 0xFFFF
}

fn is_offer(w: &[u32; 15]) -> bool {
    w[0] & 0xFFFF < 2000 && w[1] != 0xFFFF_FFFF && w[7] != 0xFFFF_FFFF && w[8] & 0xFFFF != 0xFFFF && w[9] != 0
}

/// Index of the first free entry among `max` entries classified by `entry`;
/// `None` if an entry is neither free nor a valid used one, or none is free.
fn first_free(max: usize, entry: impl Fn(usize) -> Option<Entry>) -> Option<usize> {
    for i in 0..max {
        match entry(i)? {
            Entry::Free => return Some(i),
            Entry::Used => {}
            Entry::Unknown => return None,
        }
    }
    None
}

enum Entry {
    Free,
    Used,
    Unknown,
}

/// One agreed incoming transfer.
pub struct AgreedDeal {
    pub player: Player,
    pub buyer_ref: u32,
    pub seller_ref: u32,
    pub fee_units: u32,
    pub salary_units: u32,
    pub value_units: u32,
    pub end: Date,
    pub arrival: Date,
}

pub struct DealTables {
    offers: usize,
    offer_count: usize,
    agreed_free: usize,
    pending_free: usize,
    deal_free: usize,
}

impl DealTables {
    pub fn locate(data: &[u8], table: &ContractTable) -> Result<DealTables, Error> {
        let unknown = || Error::new("This save's transfer lists don't match the known layout, so an agreed deal can't be written.");
        let header = table.first.checked_sub(RECORD).and_then(|o| words(data, o)).ok_or_else(unknown)?;
        if header[8] != HEADER_MAGIC || header[9..12] != [0, 0, 0] {
            return Err(unknown());
        }
        let offers = table.first + OFFERS_OFFSET;
        let offer_count = first_free(MAX_OFFERS, |i| {
            let w = u32s::<15>(data, offers + i * OFFER_RECORD)?;
            Some(if is_offer_blank(&w) {
                Entry::Free
            } else if is_offer(&w) {
                Entry::Used
            } else {
                Entry::Unknown
            })
        })
        .ok_or_else(unknown)?;
        let agreed_free = first_free(MAX_AGREED, |i| {
            let [id, index, _] = u16s::<3>(data, table.first + AGREED_OFFSET + i * AGREED_ENTRY)?;
            Some(match (id, index) {
                (NONE16, NONE16) => Entry::Free,
                (NONE16, _) | (_, NONE16) => Entry::Unknown,
                (id, index) if id < 2000 && index < 2000 => Entry::Used,
                _ => Entry::Unknown,
            })
        })
        .ok_or_else(unknown)?;
        let pending_free = first_free(MAX_PENDING, |i| {
            let [iid, pid, salary] = u32s::<3>(data, PENDING_ARRIVALS + i * PENDING_ENTRY)?;
            Some(match (iid, pid, salary) {
                (0xFFFF, 0, 0) => Entry::Free,
                (iid, pid, salary) if iid & 0xFFFF != 0xFFFF && pid != 0 && salary != 0 => Entry::Used,
                _ => Entry::Unknown,
            })
        })
        .ok_or_else(unknown)?;
        let deal_free = first_free(MAX_DEALS, |i| {
            let w = u32s::<13>(data, SEASON_DEALS + i * DEAL_ENTRY)?;
            Some(if w[0] == 0xFFFF && w[1..12].iter().all(|&x| x == 0) && w[12] == 0xFFFF {
                Entry::Free
            } else if w[0] & 0xFFFF != 0xFFFF && w[1] != 0 && w[2..8].iter().all(|&x| x == 0) && w[12] == 0xFFFF {
                Entry::Used
            } else {
                Entry::Unknown
            })
        })
        .ok_or_else(unknown)?;
        Ok(DealTables { offers, offer_count, agreed_free, pending_free, deal_free })
    }

    fn offer_rows(&self, data: &[u8]) -> Vec<[u32; 15]> {
        (0..self.offer_count).filter_map(|i| u32s::<15>(data, self.offers + i * OFFER_RECORD)).collect()
    }

    /// Patches that turn the player into an agreed signing arriving on `deal.arrival`.
    pub fn agree(&self, data: &[u8], table: &ContractTable, deal: &AgreedDeal) -> Result<Vec<(usize, Vec<u8>)>, Error> {
        let p = deal.player;
        let offers = self.offer_rows(data);
        if offers.iter().any(|w| w[8] == p.internal_id && w[9] == p.player_id) {
            return Err(Error::new(format!(
                "Player ID {} already has an offer in the game; finish or cancel that negotiation there first.",
                p.player_id
            )));
        }
        // Offer ids and contract indexes share one numbering.
        let mut used: BTreeSet<u32> = table.rows(data).iter().map(|c| c.index).collect();
        used.extend(offers.iter().map(|w| w[0] & 0xFFFF));
        let offer_id = (0..).find(|n| !used.contains(n)).unwrap();
        let index = (offer_id + 1..).find(|n| !used.contains(n)).unwrap();
        if index >= 2000 {
            return Err(Error::new("No free contract number is left."));
        }

        let row = table.agreed(data, &p, index, deal.salary_units, deal.value_units, deal.end, deal.arrival)?;
        let arrival = deal.arrival.pack();
        let offer = [
            offer_id,
            deal.buyer_ref,
            OFFER_AGREED,
            arrival,
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF,
            deal.seller_ref,
            p.internal_id,
            p.player_id,
            deal.fee_units,
            deal.fee_units,
            0xFFFF,
            0xFFFF,
            0,
        ];
        let agreed: Vec<u8> = [offer_id as u16, index as u16, NONE16].iter().flat_map(|v| v.to_le_bytes()).collect();
        let pending = [p.internal_id, p.player_id, deal.salary_units];
        let fee = (-(deal.fee_units as i64)) as i32 as u32;
        let season = [p.internal_id, p.player_id, 0, 0, 0, 0, 0, 0, fee, 1, 0, 0, 0xFFFF];
        Ok(vec![
            row,
            (self.offers + self.offer_count * OFFER_RECORD, encode(offer)),
            (table.first + AGREED_OFFSET + self.agreed_free * AGREED_ENTRY, agreed),
            (PENDING_ARRIVALS + self.pending_free * PENDING_ENTRY, encode(pending)),
            (SEASON_DEALS + self.deal_free * DEAL_ENTRY, encode(season)),
        ])
    }
}
