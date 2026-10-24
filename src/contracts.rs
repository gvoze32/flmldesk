//! Contract table of the club the user manages.
//!
//! Layout decoded from real Football Life 2026 saves (before / deal agreed /
//! after arrival of three signings). Each row is 48 bytes, twelve u32 words:
//!
//! | word | meaning |
//! |------|---------|
//! | w0   | internal id (same as in the club roster pair) |
//! | w1   | player id |
//! | w2   | annual salary, units of 100 EUR |
//! | w3   | player value at signing, units of 100 EUR (equals the fee for some signings) |
//! | w4   | contract end, packed `year \| month << 16 \| day << 24` |
//! | w5   | low 16 bits: row index (0xFFFF = not assigned yet); high bits unused |
//! | w6   | club reference; low 14 bits = club index |
//! | w7   | low byte: status. 5 = under contract, 4 = deal agreed (arrives on the start date), 1/3 = negotiation |
//! | w8   | contract start date, or 0xFFFF |
//! | w9, w10 | 0xFFFFFFFF |
//! | w11  | low 16 bits 0xFFFF |
//!
//! Rows are contiguous; the first blank row ends the table. The game leaves
//! stale bytes in the unused high halves, so only the documented bits are checked.
//! Detection fails closed: weak or ambiguous evidence returns an error and
//! nothing is edited. Rows are always re-read from the data buffer, so undo
//! through the engine journal keeps this view consistent.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use crate::Error;
use crate::masterleague::Player;

pub const RECORD: usize = 48;
pub const SALARY_UNIT: u64 = 100;
pub const STATUS_CONTRACTED: u8 = 5;
const MAX_WALK: usize = 400;
const MAX_TRAILING_BLANKS: usize = 12;
const NO_INDEX: u32 = 0xFFFF;
const TEAM_MASK: u32 = 0x3FFF;
/// Blank row as written by the game.
pub const BLANK: [u32; 12] =
    [0xFFFF, 0, 0, 0, 0xFFFF, 0xFFFF, 0xFFFF_FFFF, 0, 0xFFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
}

impl Date {
    pub fn new(year: u16, month: u8, day: u8) -> Option<Date> {
        let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return None,
        };
        (year >= 1 && (1..=days).contains(&day)).then_some(Date { year, month, day })
    }

    pub fn pack(self) -> u32 {
        self.year as u32 | (self.month as u32) << 16 | (self.day as u32) << 24
    }

    pub fn unpack(word: u32) -> Option<Date> {
        Date::new((word & 0xFFFF) as u16, ((word >> 16) & 0xFF) as u8, ((word >> 24) & 0xFF) as u8)
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}-{:02}-{:04}", self.day, self.month, self.year)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contract {
    pub offset: usize,
    pub internal_id: u32,
    pub player_id: u32,
    pub salary_units: u32,
    pub value_units: u32,
    pub end: Date,
    pub index: u32,
    pub team: u32,
    pub status: u8,
    pub start: Option<Date>,
}

impl Contract {
    pub fn annual_eur(&self) -> u64 {
        self.salary_units as u64 * SALARY_UNIT
    }

    pub fn club_index(&self) -> usize {
        (self.team & TEAM_MASK) as usize
    }
}

type Words = [u32; 12];

fn words(data: &[u8], off: usize) -> Option<Words> {
    let bytes = data.get(off..off.checked_add(RECORD)?)?;
    Some(core::array::from_fn(|i| u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())))
}

fn words_before(data: &[u8], off: usize) -> Option<Words> {
    off.checked_sub(RECORD).and_then(|o| words(data, o))
}

fn encode(words: &Words) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

pub fn is_blank(t: Option<Words>) -> bool {
    t.is_some_and(|t| {
        t[0] & 0xFFFF == 0xFFFF && t[1] == 0 && t[2] == 0 && t[5] & 0xFFFF == NO_INDEX && t[11] & 0xFFFF == 0xFFFF
    })
}

pub fn is_record(t: Option<Words>) -> bool {
    let Some(t) = t else { return false };
    let end_ok = Date::unpack(t[4]).is_some_and(|d| (2000..=2150).contains(&d.year));
    let index = t[5] & 0xFFFF;
    t[0] & 0xFFFF != 0xFFFF
        && t[1] != 0
        && 0 < t[2]
        && t[2] < 10_000_000
        && end_ok
        && t[11] & 0xFFFF == 0xFFFF
        && ((1..=2000).contains(&index) || index == NO_INDEX)
}

fn contract_at(data: &[u8], off: usize) -> Option<Contract> {
    let t = words(data, off);
    is_record(t).then(|| {
        let t = t.unwrap();
        Contract {
            offset: off,
            internal_id: t[0],
            player_id: t[1],
            salary_units: t[2],
            value_units: t[3],
            end: Date::unpack(t[4]).unwrap(),
            index: t[5] & 0xFFFF,
            team: t[6],
            status: (t[7] & 0xFF) as u8,
            start: Date::unpack(t[8]),
        }
    })
}

#[derive(Clone, Debug)]
pub struct ContractTable {
    pub first: usize,
    pub slots: usize,
    /// Club reference word (w6) of the managed club's contracts.
    pub team_ref: u32,
}

impl ContractTable {
    pub fn detect(data: &[u8], club: usize, roster: &[Player]) -> Result<ContractTable, Error> {
        if roster.len() < 12 {
            return Err(Error::new("Skuad terlalu sedikit untuk mengenali tabel kontrak dengan aman."));
        }
        // Group candidate rows by offset modulo the record size.
        let mut groups: BTreeMap<usize, Vec<(usize, Player)>> = BTreeMap::new();
        for p in roster {
            let mut pattern = p.internal_id.to_le_bytes().to_vec();
            pattern.extend(p.player_id.to_le_bytes());
            for off in memchr::memmem::find_iter(data, &pattern) {
                if contract_at(data, off).is_some_and(|c| c.club_index() == club) {
                    groups.entry(off % RECORD).or_default().push((off, *p));
                }
            }
        }
        if groups.is_empty() {
            return Err(Error::new("Tabel kontrak klubmu tidak ditemukan dari pemain di skuad."));
        }
        let distinct = |v: &Vec<(usize, Player)>| v.iter().map(|(_, p)| *p).collect::<HashSet<_>>().len();
        let mut ranked: Vec<_> = groups.into_values().collect();
        ranked.sort_by_key(|v| std::cmp::Reverse(distinct(v)));
        let anchors = &ranked[0];
        let seen: HashSet<Player> = anchors.iter().map(|(_, p)| *p).collect();
        if seen.len() < 10.max(roster.len().div_ceil(2)) {
            return Err(Error::new(format!(
                "Hanya {} dari {} pemain punya kontrak yang cocok; tabel tidak diedit.",
                seen.len(),
                roster.len()
            )));
        }
        if ranked.len() > 1 && distinct(&ranked[1]) as f64 >= seen.len() as f64 * 0.6 {
            return Err(Error::new("Ada dua tabel kontrak yang sama-sama mungkin; tabel tidak diedit."));
        }
        let positions: Vec<usize> = anchors.iter().map(|(o, _)| *o).collect();
        let (lo, hi) = (*positions.iter().min().unwrap(), *positions.iter().max().unwrap());
        if hi - lo > RECORD * 200 {
            return Err(Error::new("Kontrak pemain tidak berada dalam satu blok yang rapat."));
        }
        let mut first = lo;
        while is_record(words_before(data, first)) {
            first -= RECORD;
        }
        let mut slots = 0;
        let mut trailing = 0;
        let mut blanks = 0;
        for i in 0..MAX_WALK {
            let t = words(data, first + i * RECORD);
            if is_record(t) {
                if trailing > 0 {
                    return Err(Error::new("Tabel kontrak punya baris kosong di tengah; tabel tidak diedit."));
                }
            } else if is_blank(t) {
                trailing += 1;
                blanks += 1;
            } else {
                break;
            }
            slots = i + 1;
            if trailing >= MAX_TRAILING_BLANKS {
                break;
            }
        }
        if blanks == 0 {
            return Err(Error::new("Tidak ada baris kontrak kosong; tabel penuh atau tidak dikenali."));
        }
        let mut refs: HashMap<u32, usize> = HashMap::new();
        for (off, _) in anchors {
            *refs.entry(words(data, *off).unwrap()[6]).or_default() += 1;
        }
        let team_ref = refs.into_iter().max_by_key(|(r, n)| (*n, std::cmp::Reverse(*r))).unwrap().0;
        let table = ContractTable { first, slots, team_ref };
        let live: HashSet<(u32, u32)> = table.rows(data).iter().map(|c| (c.internal_id, c.player_id)).collect();
        if !seen.iter().all(|p| live.contains(&(p.internal_id, p.player_id))) {
            return Err(Error::new("Kontrak acuan tidak berada dalam satu tabel."));
        }
        Ok(table)
    }

    fn offsets(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.slots).map(|i| self.first + i * RECORD)
    }

    /// Every row (players under contract, agreed deals and negotiations).
    pub fn rows(&self, data: &[u8]) -> Vec<Contract> {
        self.offsets().map_while(|off| contract_at(data, off)).collect()
    }

    /// Players under contract with the managed club.
    pub fn contracts(&self, data: &[u8], club: usize) -> Vec<Contract> {
        self.rows(data).into_iter().filter(|c| c.status == STATUS_CONTRACTED && c.club_index() == club).collect()
    }

    pub fn wage_bill_units(&self, data: &[u8], club: usize) -> u64 {
        self.contracts(data, club).iter().map(|c| c.salary_units as u64).sum()
    }

    pub fn find(&self, data: &[u8], player: &Player) -> Vec<Contract> {
        self.rows(data)
            .into_iter()
            .filter(|c| c.internal_id == player.internal_id && c.player_id == player.player_id)
            .collect()
    }

    fn end_of_rows(&self, data: &[u8]) -> usize {
        self.first + self.rows(data).len() * RECORD
    }

    /// Patch (offset, bytes) removing the player's contract; later rows move up.
    pub fn removal(&self, data: &[u8], player: &Player, club: usize) -> Result<(usize, Vec<u8>, Contract), Error> {
        let found: Vec<Contract> =
            self.find(data, player).into_iter().filter(|c| c.status == STATUS_CONTRACTED && c.club_index() == club).collect();
        if found.len() != 1 {
            return Err(Error::new(format!(
                "Kontrak ID {} di klubmu tidak tunggal ({} baris); dibatalkan.",
                player.player_id,
                found.len()
            )));
        }
        let row = found[0];
        let end = self.end_of_rows(data);
        let mut bytes = data[row.offset + RECORD..end].to_vec();
        bytes.extend(encode(&BLANK));
        Ok((row.offset, bytes, row))
    }

    /// Patch (offset, bytes) that puts the player under contract with the managed club.
    ///
    /// An existing row for the player (shortlist, negotiation, agreed deal) is
    /// completed in place, as the game does when a deal goes through; otherwise
    /// a new row is appended after the last one.
    pub fn signing(
        &self,
        data: &[u8],
        player: &Player,
        salary_units: u32,
        value_units: u32,
        end: Date,
        start: Date,
    ) -> Result<(usize, Vec<u8>), Error> {
        if !(1..10_000_000).contains(&salary_units) {
            return Err(Error::new("Gaji harus antara €100 dan €999.999.900 per tahun."));
        }
        if !(2000..=2150).contains(&end.year) || end <= start {
            return Err(Error::new("Akhir kontrak harus setelah tanggal game dan sebelum 2150."));
        }
        let rows = self.rows(data);
        let next_index = rows.iter().map(|c| c.index).filter(|i| *i != NO_INDEX).max().unwrap_or(0) + 1;
        let existing: Vec<&Contract> =
            rows.iter().filter(|c| c.internal_id == player.internal_id && c.player_id == player.player_id).collect();
        let (offset, mut w) = match existing.as_slice() {
            [] => {
                let offset = self.first + rows.len() * RECORD;
                if offset >= self.first + self.slots * RECORD || !is_blank(words(data, offset)) {
                    return Err(Error::new("Tabel kontrak penuh; jual/lepas pemain dulu."));
                }
                let mut w = BLANK;
                w[3] = value_units;
                w[5] = next_index;
                (offset, w)
            }
            [row] => {
                if row.status == STATUS_CONTRACTED && row.team == self.team_ref {
                    return Err(Error::new(format!("ID {} sudah punya kontrak di klubmu.", player.player_id)));
                }
                let mut w = words(data, row.offset).unwrap();
                if w[5] & 0xFFFF == NO_INDEX {
                    w[5] = (w[5] & !0xFFFF) | next_index;
                }
                (row.offset, w)
            }
            _ => return Err(Error::new(format!("ID {} punya lebih dari satu baris kontrak; dibatalkan.", player.player_id))),
        };
        w[0] = player.internal_id;
        w[1] = player.player_id;
        w[2] = salary_units;
        w[4] = end.pack();
        w[6] = self.team_ref;
        w[7] = (w[7] & !0xFF) | STATUS_CONTRACTED as u32;
        w[8] = start.pack();
        if !is_record(Some(w)) {
            return Err(Error::new("Kontrak baru tidak lolos validasi format."));
        }
        Ok((offset, encode(&w)))
    }
}
