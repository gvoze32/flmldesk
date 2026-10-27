//! Agreed transfers that wait for the player to arrive, written the way
//! Football Life 2026 stores a deal agreed in the game. On the arrival date the
//! game itself moves the player, completes the contract and clears the lists.
//!
//! Decoded from negotiations saved at every step (two club signings, one free
//! agent). Offsets marked "table +" are relative to contract slot 0
//! (`ContractTable::first`), which the game always precedes with a row whose
//! w8 is `0x74574`:
//!
//! | Data | Location | Row |
//! |---|---|---|
//! | Offers | table + `0xD5C0` (after 1140 contract slots), 60 bytes | id, buying club, state (4 = agreed), arrival date, 3 filler words, selling club, internal id, player id, fee, fee, `0xFFFF`, `0xFFFF`, 0 |
//! | Agreed deals | table + `0x47720`, 6 bytes | u16 offer id, u16 contract index, `0xFFFF`; a free agent: contract index, `0xFFFF`, `0xFFFF` |
//! | Pending arrivals | `0xC81870`, 12 bytes | internal id, player id, salary units |
//! | Season deals | `0xC803E0`, 52 bytes | internal id, player id, six zeros, minus the fee (i32), 1, 0, 0, `0xFFFF` |
//! | News | `0xBE3360`, 56 bytes | type `0x6E`, internal id, player id, filler, agreement date |
//! | Negotiation timeline | table + `0x587A4`, 100-byte blocks | see `TIMELINE_OFFSET` |
//!
//! A free agent's deal has no offer, no season-deal row and no timeline.
//! Offer ids and contract indexes share one numbering: the lowest free number
//! is the offer id and the next free one the contract index (a free agent's
//! contract takes the lowest). The fee is in units of 100 EUR everywhere. The
//! timeline and news are what the game keeps for an agreed deal until the
//! player arrives (the negotiations screen likely reads them). Every table is
//! checked before writing; anything unexpected fails closed.
//!
//! An agreed loan (`agree_loan`, decoded from a loan-in and a loan-out saved at
//! every step) uses the same tables with these differences: no contract row
//! and no contract index (the player keeps his contract with his club), offer
//! word 12 holds the loan end date, the agreed-deals entry is `(offer id,
//! 0xFFFF, 0xFFFF)`, no pending arrival, season-deal type 2, only the offer
//! block of the timeline, and for a loan-out the fee is income (positive
//! season-deal fee) with news type `0x6F`. The game moves the player on the
//! arrival date and writes the parent club and loan end into his record then.

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
/// Negotiation timeline (offset from contract slot 0): 100-byte blocks, three
/// per negotiation slot (offer id, contract index, unused). Block header: u16
/// owner (0 for your club, 0x100… for AI clubs, 0xFF00 = free), u16 0xFFFF;
/// then 8 rounds of 6 × u16: number, round (low byte), from (year, day << 8 |
/// month), to (same). The game's negotiations screen lists the slots in order.
pub const TIMELINE_OFFSET: usize = 0x58740;
pub const TIMELINE_BLOCK: usize = 100;
pub const TIMELINE_SLOT: usize = 3 * TIMELINE_BLOCK;
pub const TIMELINE_ROUND: usize = 12;
const TIMELINE_ROUNDS: usize = 8;
const TIMELINE_FREE: u16 = 0xFF00;
const TIMELINE_MINE: u16 = 0;
const MAX_TIMELINE_SLOTS: usize = 64;
/// News items, 56 bytes: type, internal id, player id, filler, then the date in the last word.
pub const NEWS: usize = 0xBE_3360;
pub const NEWS_ROW: usize = 56;
const MAX_NEWS: usize = 256;
const NEWS_DEAL_AGREED: u32 = 0x6E;
const NEWS_LOAN_OUT_AGREED: u32 = 0x6F;
const SEASON_PERMANENT: u32 = 1;
const SEASON_LOAN: u32 = 2;
/// Free news row as written by the game.
pub const BLANK_NEWS: [u32; 14] =
    [0xFF, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF, 0, 0xFFFF_FFFF, 0xFFFF, 0xFFFF_FFFF, 0, 0xFFFF, 0xFFFF, 0xFFFF];

fn timeline_date(d: Date) -> [u16; 2] {
    [d.year, (d.day as u16) << 8 | d.month as u16]
}

/// Last day of the transfer window the game's negotiations run to (31 August
/// in both samples; 31 January in winter).
fn window_end(today: Date) -> Date {
    match today.month {
        1 => Date { year: today.year, month: 1, day: 31 },
        2..=8 => Date { year: today.year, month: 8, day: 31 },
        _ => Date { year: today.year + 1, month: 1, day: 31 },
    }
}

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
    /// Club the player leaves; `None` for a player without a club (free agent),
    /// whose deal has no offer and no fee.
    pub seller_ref: Option<u32>,
    pub fee_units: u32,
    pub salary_units: u32,
    pub value_units: u32,
    pub end: Date,
    /// In-game date the deal is agreed on.
    pub today: Date,
    pub arrival: Date,
}

/// One agreed loan, either to the managed club or from it.
pub struct LoanDeal {
    pub player: Player,
    /// Club the player joins for the loan.
    pub buyer_ref: u32,
    /// Club the player is loaned from (his contract stays there).
    pub seller_ref: u32,
    /// True when the managed club loans the player out (the fee is income).
    pub outgoing: bool,
    pub fee_units: u32,
    pub loan_end: Date,
    pub today: Date,
    pub arrival: Date,
}

pub struct DealTables {
    offers: usize,
    offer_count: usize,
    agreed_free: usize,
    pending_free: usize,
    deal_free: usize,
    /// Offset of the first free timeline slot.
    timeline_free: usize,
    news_free: usize,
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
                // A free agent's deal has no offer: (contract index, 0xFFFF).
                (id, index) if id < 2000 && (index < 2000 || index == NONE16) => Entry::Used,
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
        let timeline = table.first + TIMELINE_OFFSET;
        let block_state = |block: usize| -> Option<Entry> {
            let [owner, filler] = u16s::<2>(data, block)?;
            // A cleared round keeps its dates; only the number is reset.
            let rounds_ok = (0..TIMELINE_ROUNDS).all(|r| {
                u16s::<6>(data, block + 4 + r * TIMELINE_ROUND).is_some_and(|[number, _, y1, _, y2, _]| {
                    number == NONE16 || (number < 2000 && (2000..=2150).contains(&y1) && (2000..=2150).contains(&y2))
                })
            });
            Some(match (owner, filler, rounds_ok) {
                (TIMELINE_FREE, 0xFFFF, true) => Entry::Free,
                (owner, 0xFFFF, true) if owner & 0xFF == 0 && owner >> 8 < 0x10 => Entry::Used,
                _ => Entry::Unknown,
            })
        };
        // A slot is free when its offer and contract blocks are.
        let timeline_slot = first_free(MAX_TIMELINE_SLOTS, |i| {
            let slot = timeline + i * TIMELINE_SLOT;
            match (block_state(slot)?, block_state(slot + TIMELINE_BLOCK)?, block_state(slot + 2 * TIMELINE_BLOCK)?) {
                (Entry::Unknown, ..) | (_, Entry::Unknown, _) | (.., Entry::Unknown) => Some(Entry::Unknown),
                (Entry::Free, Entry::Free, _) => Some(Entry::Free),
                _ => Some(Entry::Used),
            }
        })
        .ok_or_else(unknown)?;
        let timeline_free = timeline + timeline_slot * TIMELINE_SLOT;
        let news_free = first_free(MAX_NEWS, |i| {
            let w = u32s::<14>(data, NEWS + i * NEWS_ROW)?;
            Some(if w == BLANK_NEWS {
                Entry::Free
            } else if w[0] & 0xFF != 0xFF && w[13] != 0xFFFF && Date::unpack(w[13]).is_some() {
                Entry::Used
            } else {
                Entry::Unknown
            })
        })
        .ok_or_else(unknown)?;
        Ok(DealTables { offers, offer_count, agreed_free, pending_free, deal_free, timeline_free, news_free })
    }

    fn offer_rows(&self, data: &[u8]) -> Vec<[u32; 15]> {
        (0..self.offer_count).filter_map(|i| u32s::<15>(data, self.offers + i * OFFER_RECORD)).collect()
    }

    /// Whether the game lists an agreed deal (offer state 4) for the player.
    pub fn has_agreed_offer(&self, data: &[u8], p: &Player) -> bool {
        self.offer_rows(data).iter().any(|w| w[2] & 0xFF == OFFER_AGREED && w[8] == p.internal_id && w[9] == p.player_id)
    }

    fn refuse_existing_offer(&self, data: &[u8], p: &Player) -> Result<Vec<[u32; 15]>, Error> {
        let offers = self.offer_rows(data);
        if offers.iter().any(|w| w[8] == p.internal_id && w[9] == p.player_id) {
            return Err(Error::new(format!(
                "Player ID {} already has an offer in the game; finish or cancel that negotiation there first.",
                p.player_id
            )));
        }
        Ok(offers)
    }

    fn timeline_rounds(number: u32, rounds: &[(u16, Date, Date)]) -> Vec<u8> {
        let mut bytes: Vec<u8> = [TIMELINE_MINE, NONE16].iter().flat_map(|v| v.to_le_bytes()).collect();
        for &(round, from, to) in rounds {
            let [y1, md1] = timeline_date(from);
            let [y2, md2] = timeline_date(to);
            bytes.extend([number as u16, round, y1, md1, y2, md2].iter().flat_map(|v| v.to_le_bytes()));
        }
        bytes
    }

    /// Patches that turn the player into an agreed loan starting on `loan.arrival`.
    pub fn agree_loan(&self, data: &[u8], table: &ContractTable, loan: &LoanDeal) -> Result<Vec<(usize, Vec<u8>)>, Error> {
        let p = loan.player;
        let offers = self.refuse_existing_offer(data, &p)?;
        if loan.loan_end <= loan.arrival || loan.loan_end.year > 2150 {
            return Err(Error::new("The loan must end after the player arrives and before 2150."));
        }
        // A loan takes the lowest free number for its offer and no contract index.
        let mut used: BTreeSet<u32> = table.rows(data).iter().map(|c| c.index).collect();
        used.extend(offers.iter().map(|w| w[0] & 0xFFFF));
        let offer_id = (0..).find(|n| !used.contains(n)).unwrap();
        if offer_id >= 2000 {
            return Err(Error::new("No free offer number is left."));
        }
        let offer = [
            offer_id,
            loan.buyer_ref,
            OFFER_AGREED,
            loan.arrival.pack(),
            0xFFFF_FFFF,
            0xFFFF_FFFF,
            0xFFFF,
            loan.seller_ref,
            p.internal_id,
            p.player_id,
            loan.fee_units,
            loan.fee_units,
            loan.loan_end.pack(),
            0xFFFF,
            0,
        ];
        let fee = if loan.outgoing { loan.fee_units } else { (-(loan.fee_units as i64)) as i32 as u32 };
        let season = [p.internal_id, p.player_id, 0, 0, 0, 0, 0, 0, fee, SEASON_LOAN, 0, 0, 0xFFFF];
        // Only the offer block: the game's loan-in had the club accepting
        // today and talks until the window closes, its loan-out one round.
        let rounds: &[(u16, Date, Date)] = if loan.outgoing {
            &[(1, loan.today, window_end(loan.today))]
        } else {
            &[(1, loan.today, loan.today), (2, loan.today, window_end(loan.today))]
        };
        let mut news = BLANK_NEWS;
        news[0] = if loan.outgoing { NEWS_LOAN_OUT_AGREED } else { NEWS_DEAL_AGREED };
        news[1] = p.internal_id;
        news[2] = p.player_id;
        news[13] = loan.today.pack();
        let agreed: Vec<u8> = [offer_id as u16, NONE16, NONE16].iter().flat_map(|v| v.to_le_bytes()).collect();
        Ok(vec![
            (self.offers + self.offer_count * OFFER_RECORD, encode(offer)),
            (SEASON_DEALS + self.deal_free * DEAL_ENTRY, encode(season)),
            (self.timeline_free, Self::timeline_rounds(offer_id, rounds)),
            (table.first + AGREED_OFFSET + self.agreed_free * AGREED_ENTRY, agreed),
            (NEWS + self.news_free * NEWS_ROW, encode(news)),
        ])
    }

    /// Patches that turn the player into an agreed signing arriving on `deal.arrival`.
    pub fn agree(&self, data: &[u8], table: &ContractTable, deal: &AgreedDeal) -> Result<Vec<(usize, Vec<u8>)>, Error> {
        let p = deal.player;
        let offers = self.refuse_existing_offer(data, &p)?;
        // Offer ids and contract indexes share one numbering: the lowest free
        // number for the offer, the next free one for the contract. A free
        // agent has no offer, so his contract takes the lowest free number.
        let mut used: BTreeSet<u32> = table.rows(data).iter().map(|c| c.index).collect();
        used.extend(offers.iter().map(|w| w[0] & 0xFFFF));
        let free_from = |start: u32| (start..).find(|n| !used.contains(n)).unwrap();
        let offer_id = deal.seller_ref.map(|_| free_from(0));
        let index = free_from(offer_id.map_or(0, |id| id + 1));
        if index >= 2000 {
            return Err(Error::new("No free contract number is left."));
        }

        let mut patches = vec![table.agreed(data, &p, index, deal.salary_units, deal.value_units, deal.end, deal.arrival)?];
        let pair = match (offer_id, deal.seller_ref) {
            (Some(offer_id), Some(seller_ref)) => {
                let offer = [
                    offer_id,
                    deal.buyer_ref,
                    OFFER_AGREED,
                    deal.arrival.pack(),
                    0xFFFF_FFFF,
                    0xFFFF_FFFF,
                    0xFFFF,
                    seller_ref,
                    p.internal_id,
                    p.player_id,
                    deal.fee_units,
                    deal.fee_units,
                    0xFFFF,
                    0xFFFF,
                    0,
                ];
                patches.push((self.offers + self.offer_count * OFFER_RECORD, encode(offer)));
                let fee = (-(deal.fee_units as i64)) as i32 as u32;
                let season = [p.internal_id, p.player_id, 0, 0, 0, 0, 0, 0, fee, SEASON_PERMANENT, 0, 0, 0xFFFF];
                patches.push((SEASON_DEALS + self.deal_free * DEAL_ENTRY, encode(season)));
                // A timeline slot as in the game: an offer block and a contract
                // block, each with the club accepting and the player's contract
                // talks until the window closes.
                for (block, number) in [(0, offer_id), (1, index)] {
                    let rounds = [(1, deal.today, deal.today), (2, deal.today, window_end(deal.today))];
                    patches.push((self.timeline_free + block * TIMELINE_BLOCK, Self::timeline_rounds(number, &rounds)));
                }
                [offer_id as u16, index as u16, NONE16]
            }
            _ => [index as u16, NONE16, NONE16],
        };
        let pair: Vec<u8> = pair.iter().flat_map(|v| v.to_le_bytes()).collect();
        patches.push((table.first + AGREED_OFFSET + self.agreed_free * AGREED_ENTRY, pair));
        patches.push((PENDING_ARRIVALS + self.pending_free * PENDING_ENTRY, encode([p.internal_id, p.player_id, deal.salary_units])));
        let mut news = BLANK_NEWS;
        news[0] = NEWS_DEAL_AGREED;
        news[1] = p.internal_id;
        news[2] = p.player_id;
        news[13] = deal.today.pack();
        patches.push((NEWS + self.news_free * NEWS_ROW, encode(news)));
        Ok(patches)
    }
}
