//! Master League save editing: club rosters, contracts and club finances.
//!
//! Layout of the decrypted `data` block (PES 2021 1.04+ and Football Life,
//! both run the same Master League code):
//!
//! * Club table at 0x50, 0x690 bytes per club: name +0x04, abbreviation +0x4A,
//!   roster of 40 `(internal_id, player_id)` pairs at +0x14C (empty =
//!   `(0xFFFF, 0)`), roster count in the low 7 bits of the u16 at +0x426.
//!   Clubs come first, then national teams (Albania/ALB first) up to The Old Boys.
//! * Finance block: transfer budget u32, four zero words, then the annual
//!   salary budget u32 (+0x14), both in units of 100 EUR. Football Life 2026
//!   keeps it at 0xC7DD00 (confirmed against the in-game value); PES 2021 at
//!   0xC7DE00 (ML Money Tool by Devil Cold52). The game shows the salary budget
//!   minus the club's wage bill, and charges the transfer fee when a deal is
//!   agreed. Manager name: UTF-8 at 0x1942C0, 46 bytes.
//! * The save description holds the managed club and in-game date
//!   (`Chelsea FC / Premier League\n6/8/2025\n…`).
//!
//! Every edit goes through `patch`, which journals the previous bytes, so undo
//! is byte-exact and saving can prove that nothing outside the journal changed.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::contracts::{Contract, ContractTable, Date, SALARY_UNIT, STATUS_AGREED};
use crate::deals::{AgreedDeal, DealTables};
use crate::{Error, crypto};

pub const TABLE_START: usize = 0x50;
pub const RECORD_SIZE: usize = 0x690;
pub const NAME_OFFSET: usize = 0x04;
pub const NAME_SIZE: usize = 0x46;
pub const ABBR_OFFSET: usize = 0x4A;
pub const ABBR_SIZE: usize = 6;
pub const ROSTER_OFFSET: usize = 0x14C;
pub const COUNT_OFFSET: usize = 0x426;
pub const COUNT_MASK: u16 = 0x7F;
pub const MAX_ROSTER: usize = 40;
pub const EMPTY: Player = Player { internal_id: 0xFFFF, player_id: 0 };
const MAX_SAVE: u64 = 80_000_000;

pub const FL26_FINANCE: usize = 0xC7DD00;
pub const PES21_FINANCE: usize = 0xC7DE00;
pub const SALARY_BUDGET_OFFSET: usize = 0x14;
pub const MANAGER_NAME: usize = 0x1942C0;
pub const MANAGER_NAME_SIZE: usize = 46;
pub const MONEY_UNIT: i64 = 100;

pub const NATIONAL_FIRST: [&str; 4] = ["ALB", "AND", "ARM", "AUT"];
const NATIONAL_FIRST_NAMES: [&str; 4] = ["albania", "andorra", "armenia", "austria"];
pub const BACKUP_DIR: &str = "FLMLDesk_backup";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Team {
    pub index: usize,
    pub name: String,
    pub abbr: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Player {
    pub internal_id: u32,
    pub player_id: u32,
}

/// Where a transferred player comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Club(usize),
    /// National-team players without a club; some may be free agents.
    Others,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finances {
    pub transfer_eur: i64,
    /// Annual salary budget; the game shows this minus the wage bill.
    pub salary_eur: i64,
    pub manager_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct Operation {
    pub description: String,
    /// (offset, previous bytes) in write order.
    patches: Vec<(usize, Vec<u8>)>,
}

#[derive(Clone, Debug)]
pub struct TransferRequest {
    pub source: Source,
    pub dest: usize,
    pub player_id: u32,
    pub fee_eur: i64,
    pub salary_eur: Option<i64>,
    pub contract_end: Option<Date>,
    /// Charge (incoming) or credit (outgoing) the fee to the transfer budget.
    pub use_transfer_budget: bool,
    pub name: Option<String>,
}

impl TransferRequest {
    pub fn new(source: Source, dest: usize, player_id: u32) -> Self {
        TransferRequest {
            source,
            dest,
            player_id,
            fee_eur: 0,
            salary_eur: None,
            contract_end: None,
            use_transfer_budget: true,
            name: None,
        }
    }
}

/// `round(value / divisor)` with ties to even, like Python's `round`.
pub fn div_round_even(value: i64, divisor: i64) -> i64 {
    let q = value.div_euclid(divisor);
    let r = value.rem_euclid(divisor) * 2;
    if r > divisor || (r == divisor && q % 2 != 0) { q + 1 } else { q }
}

pub fn eur_to_units(eur: i64) -> i64 {
    div_round_even(eur, MONEY_UNIT)
}

pub fn fmt_int(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

pub fn fmt_eur(eur: i64) -> String {
    format!("€{}", fmt_int(eur))
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn text_at(data: &[u8], off: usize, size: usize) -> String {
    let raw = &data[off..off + size];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

pub struct MasterLeague {
    pub path: PathBuf,
    save: crypto::SaveFile,
    pub game: String,
    pub data: Vec<u8>,
    pub history: Vec<Operation>,
    current: Option<Operation>,
    pub managed_team: Option<usize>,
    pub contracts: Option<ContractTable>,
    pub teams: Vec<Team>,
    national: Range<usize>,
    finance_offset: Option<usize>,
    pub finance_error: Option<String>,
    /// In-game date from the save description.
    pub game_date: Option<Date>,
    /// Managed club named in the save description.
    pub described_club: Option<usize>,
}

impl MasterLeague {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let path = std::path::absolute(path).map_err(|_| Error::new("File not found."))?;
        let meta = std::fs::metadata(&path).map_err(|_| Error::new("File not found."))?;
        if !meta.is_file() || meta.len() > MAX_SAVE {
            return Err(Error::new("File not found, or its size doesn't look like an ML save."));
        }
        let blob = std::fs::read(&path).map_err(|e| Error::new(e.to_string()))?;
        let save = crypto::unpack(&blob, true)?;
        if save.file_type() != "ML" {
            let t = save.file_type();
            return Err(Error::new(format!(
                "This isn't a Master League save (file type: {}).",
                if t.is_empty() { "?" } else { &t }
            )));
        }
        let mut game = MasterLeague {
            path,
            game: save.game_version(),
            data: save.data.clone(),
            save,
            history: Vec::new(),
            current: None,
            managed_team: None,
            contracts: None,
            teams: Vec::new(),
            national: 0..0,
            finance_offset: None,
            finance_error: None,
            game_date: None,
            described_club: None,
        };
        game.teams = game.discover_clubs()?;
        game.national = game.discover_national_teams()?;
        for i in 0..game.teams.len() {
            game.raw_roster(i)?;
        }
        match game.locate_finances() {
            Ok(offset) => game.finance_offset = Some(offset),
            Err(e) => game.finance_error = Some(e),
        }
        game.read_description();
        Ok(game)
    }

    /// `"Chelsea FC / Premier League\n6/8/2025\nPremier League"` at desc+0x80.
    fn read_description(&mut self) {
        let raw = self.save.desc.get(0x80..).unwrap_or(&[]);
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let text = String::from_utf8_lossy(&raw[..end]).into_owned();
        let mut lines = text.lines();
        if let Some(club) = lines.next().and_then(|l| l.split(" / ").next()) {
            let matches: Vec<usize> = self.teams.iter().filter(|t| t.name == club.trim()).map(|t| t.index).collect();
            self.described_club = (matches.len() == 1).then(|| matches[0]);
        }
        self.game_date = lines.next().and_then(|line| {
            let mut parts = line.trim().split('/').map(|p| p.parse::<u16>().ok());
            let (d, m, y) = (parts.next()??, parts.next()??, parts.next()??);
            Date::new(y, u8::try_from(m).ok()?, u8::try_from(d).ok()?)
        });
    }

    // --- discovery -----------------------------------------------------
    fn base(index: usize) -> usize {
        TABLE_START + index * RECORD_SIZE
    }

    fn table_len(&self) -> usize {
        self.data.len().saturating_sub(TABLE_START) / RECORD_SIZE
    }

    fn team(&self, index: usize) -> Result<Team, Error> {
        let base = Self::base(index);
        if base + RECORD_SIZE > self.data.len() {
            return Err(Error::new("The club list in this save looks damaged."));
        }
        Ok(Team {
            index,
            name: text_at(&self.data, base + NAME_OFFSET, NAME_SIZE),
            abbr: text_at(&self.data, base + ABBR_OFFSET, ABBR_SIZE),
        })
    }

    fn is_national_start(&self, i: usize) -> Result<bool, Error> {
        let group = (i..i + 4).map(|j| self.team(j)).collect::<Result<Vec<_>, _>>()?;
        Ok(group.iter().zip(NATIONAL_FIRST).all(|(t, a)| t.abbr.to_uppercase() == a)
            || group.iter().zip(NATIONAL_FIRST_NAMES).all(|(t, n)| t.name.to_lowercase() == n))
    }

    fn discover_clubs(&self) -> Result<Vec<Team>, Error> {
        let limit = 900.min(self.table_len().saturating_sub(4));
        let mut count = 0;
        for i in 0..limit {
            if self.is_national_start(i)? {
                count = i;
                break;
            }
        }
        if !(400..=750).contains(&count) {
            return Err(Error::new("Couldn't tell clubs and national teams apart; this save was not changed."));
        }
        let teams = (0..count).map(|i| self.team(i)).collect::<Result<Vec<_>, _>>()?;
        if teams.iter().any(|t| t.name.is_empty()) {
            return Err(Error::new("A club in this save has no name."));
        }
        Ok(teams)
    }

    fn discover_national_teams(&self) -> Result<Range<usize>, Error> {
        let first = self.teams.len();
        for i in first..(first + 200).min(self.table_len()) {
            let t = self.team(i)?;
            if t.abbr.to_uppercase() == "TOB" || t.name.to_lowercase() == "the old boys" {
                if (80..=180).contains(&(i - first)) {
                    return Ok(first..i);
                }
                break;
            }
        }
        Ok(0..0)
    }

    /// Finance block offset for this game, verified structurally; error text when untrusted.
    fn locate_finances(&self) -> Result<usize, String> {
        let raw = self.data.get(MANAGER_NAME..MANAGER_NAME + MANAGER_NAME_SIZE).unwrap_or(&[]);
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let name_ok = std::str::from_utf8(&raw[..end])
            .is_ok_and(|n| !n.trim().is_empty() && !n.chars().any(|c| c < ' '))
            && raw[end..].iter().all(|&b| b == 0);
        if raw.is_empty() || !name_ok {
            return Err("Couldn't find the manager name in this save, so the budget can't be edited.".into());
        }
        // No cross-variant fallback: the other game's offset can hold a look-alike block.
        let off = if self.game.contains("Football Life") { FL26_FINANCE } else { PES21_FINANCE };
        let plausible = self.data.len() >= off + SALARY_BUDGET_OFFSET + 4
            && (4..SALARY_BUDGET_OFFSET).step_by(4).all(|gap| u32_at(&self.data, off + gap) == 0)
            && u32_at(&self.data, off) < 0x8000_0000
            && (1..0x8000_0000).contains(&u32_at(&self.data, off + SALARY_BUDGET_OFFSET));
        if plausible { Ok(off) } else { Err("Couldn't find the budget in this save, so it can't be edited.".into()) }
    }

    // --- rosters ---------------------------------------------------------
    fn raw_roster(&self, index: usize) -> Result<(usize, Vec<Player>), Error> {
        if index >= self.teams.len() && !self.national.contains(&index) {
            return Err(Error::new("Invalid club."));
        }
        let base = Self::base(index);
        let n = (u16_at(&self.data, base + COUNT_OFFSET) & COUNT_MASK) as usize;
        let name = self.team(index)?.name;
        if n > MAX_ROSTER {
            return Err(Error::new(format!("{name} has an invalid number of players: {n}.")));
        }
        let slots: Vec<Player> = (0..MAX_ROSTER)
            .map(|j| {
                let o = base + ROSTER_OFFSET + 8 * j;
                Player { internal_id: u32_at(&self.data, o), player_id: u32_at(&self.data, o + 4) }
            })
            .collect();
        if slots[..n].contains(&EMPTY) {
            return Err(Error::new(format!("{name}'s squad has an empty slot in the middle.")));
        }
        if slots[n..].iter().any(|p| *p != EMPTY) {
            return Err(Error::new(format!("{name}'s squad has unknown data in its empty slots.")));
        }
        Ok((n, slots))
    }

    fn club_roster(&self, index: usize) -> Vec<Player> {
        // Validated at open and after every write, so this cannot fail for known teams.
        let (n, mut slots) = self.raw_roster(index).expect("validated roster");
        slots.truncate(n);
        slots
    }

    pub fn roster(&self, source: Source) -> Vec<Player> {
        match source {
            Source::Club(index) => self.club_roster(index),
            Source::Others => self.other_candidates(),
        }
    }

    /// National-team players without a club. Some may be free agents.
    pub fn other_candidates(&self) -> Vec<Player> {
        let in_clubs: HashSet<u32> =
            (0..self.teams.len()).flat_map(|i| self.club_roster(i)).map(|p| p.player_id).collect();
        let mut found: BTreeMap<u32, Player> = BTreeMap::new();
        let mut ambiguous = HashSet::new();
        for i in self.national.clone() {
            for p in self.club_roster(i) {
                if p.player_id == 0 || p.internal_id == 0xFFFF || in_clubs.contains(&p.player_id) {
                    continue;
                }
                match found.get(&p.player_id) {
                    Some(prev) if *prev != p => {
                        ambiguous.insert(p.player_id);
                    }
                    Some(_) => {}
                    None => {
                        found.insert(p.player_id, p);
                    }
                }
            }
        }
        found.into_iter().filter(|(pid, _)| !ambiguous.contains(pid)).map(|(_, p)| p).collect()
    }

    pub fn club_of(&self, player_id: u32) -> Vec<usize> {
        (0..self.teams.len()).filter(|&i| self.club_roster(i).iter().any(|p| p.player_id == player_id)).collect()
    }

    pub fn source_name(&self, source: Source) -> String {
        match source {
            Source::Club(i) => self.teams[i].name.clone(),
            Source::Others => "Others (national team)".into(),
        }
    }

    // --- journaled writes -------------------------------------------------
    fn patch(&mut self, offset: usize, new: &[u8]) {
        let op = self.current.as_mut().expect("write outside an operation");
        op.patches.push((offset, self.data[offset..offset + new.len()].to_vec()));
        self.data[offset..offset + new.len()].copy_from_slice(new);
    }

    fn begin(&mut self) {
        self.current = Some(Operation::default());
    }

    fn commit(&mut self, description: String) -> &Operation {
        let mut op = self.current.take().expect("no operation");
        op.description = description;
        self.history.push(op);
        self.history.last().unwrap()
    }

    fn rollback(&mut self) {
        if let Some(op) = self.current.take() {
            for (offset, old) in op.patches.iter().rev() {
                self.data[*offset..offset + old.len()].copy_from_slice(old);
            }
        }
    }

    /// Run `body` as one journaled operation; any error restores the previous bytes.
    fn atomic<F>(&mut self, body: F) -> Result<&Operation, Error>
    where
        F: FnOnce(&mut Self) -> Result<String, Error>,
    {
        self.begin();
        match body(self) {
            Ok(description) => Ok(self.commit(description)),
            Err(e) => {
                self.rollback();
                Err(e)
            }
        }
    }

    pub fn undo(&mut self) -> Result<String, Error> {
        let op = self.history.pop().ok_or_else(|| Error::new("Nothing to undo."))?;
        for (offset, old) in op.patches.iter().rev() {
            self.data[*offset..offset + old.len()].copy_from_slice(old);
        }
        Ok(op.description)
    }

    pub fn operations(&self) -> Vec<&str> {
        self.history.iter().map(|o| o.description.as_str()).collect()
    }

    fn write_roster(&mut self, index: usize, players: &[Player]) -> Result<(), Error> {
        if players.len() > MAX_ROSTER {
            return Err(Error::new("The destination squad would go over 40 players."));
        }
        let base = Self::base(index);
        let mut bytes = Vec::with_capacity(MAX_ROSTER * 8);
        for p in players.iter().chain(std::iter::repeat_n(&EMPTY, MAX_ROSTER - players.len())) {
            bytes.extend(p.internal_id.to_le_bytes());
            bytes.extend(p.player_id.to_le_bytes());
        }
        self.patch(base + ROSTER_OFFSET, &bytes);
        let word = u16_at(&self.data, base + COUNT_OFFSET);
        self.patch(base + COUNT_OFFSET, &((word & !COUNT_MASK) | players.len() as u16).to_le_bytes());
        Ok(())
    }

    // --- finances -----------------------------------------------------------
    fn finance_at(&self) -> Result<usize, Error> {
        self.finance_offset.ok_or_else(|| Error::new(self.finance_error.clone().unwrap_or_default()))
    }

    pub fn finances(&self) -> Result<Finances, Error> {
        let off = self.finance_at()?;
        Ok(Finances {
            transfer_eur: u32_at(&self.data, off) as i64 * MONEY_UNIT,
            salary_eur: u32_at(&self.data, off + SALARY_BUDGET_OFFSET) as i64 * MONEY_UNIT,
            manager_name: text_at(&self.data, MANAGER_NAME, MANAGER_NAME_SIZE),
        })
    }

    fn set_u32(&mut self, offset: usize, value: i64, label: &str) -> Result<(), Error> {
        let value = u32::try_from(value)
            .ok()
            .filter(|v| *v < 0x8000_0000)
            .ok_or_else(|| Error::new(format!("{label} is too high or too low for the game.")))?;
        self.patch(offset, &value.to_le_bytes());
        Ok(())
    }

    fn adjust_transfer_budget(&mut self, delta_eur: i64) -> Result<(), Error> {
        let off = self.finance_at()?;
        let units = u32_at(&self.data, off) as i64 + div_round_even(delta_eur, MONEY_UNIT);
        if units < 0 {
            return Err(Error::new(format!("Not enough transfer budget: {} short.", fmt_eur(-units * MONEY_UNIT))));
        }
        self.set_u32(off, units.min(0x7FFF_FFFF), "Transfer budget")
    }

    pub fn set_finances(&mut self, transfer_eur: i64, salary_eur: i64, manager_name: &str) -> Result<&Operation, Error> {
        let before = self.finances()?;
        let off = self.finance_at()?;
        let name = manager_name.trim();
        if name.is_empty() || name.len() >= MANAGER_NAME_SIZE {
            return Err(Error::new(format!(
                "Enter a manager name of at most {} letters (fewer with accents or special characters).",
                MANAGER_NAME_SIZE - 1
            )));
        }
        if manager_name.chars().any(|c| c < ' ') {
            return Err(Error::new("The manager name contains characters that aren't allowed."));
        }
        if transfer_eur == before.transfer_eur && salary_eur == before.salary_eur && name == before.manager_name {
            return Err(Error::new("Nothing was changed."));
        }
        let name = name.to_string();
        self.atomic(move |g| {
            let mut changes = Vec::new();
            if transfer_eur != before.transfer_eur {
                g.set_u32(off, eur_to_units(transfer_eur), "Transfer budget")?;
                changes.push(format!("transfer budget {} → {}", fmt_eur(before.transfer_eur), fmt_eur(transfer_eur)));
            }
            if salary_eur != before.salary_eur {
                g.set_u32(off + SALARY_BUDGET_OFFSET, eur_to_units(salary_eur), "Salary budget")?;
                changes.push(format!("salary budget {} → {}", fmt_eur(before.salary_eur), fmt_eur(salary_eur)));
            }
            if name != before.manager_name {
                let mut bytes = name.as_bytes().to_vec();
                bytes.resize(MANAGER_NAME_SIZE, 0);
                g.patch(MANAGER_NAME, &bytes);
                changes.push(format!("manager \"{}\" → \"{name}\"", before.manager_name));
            }
            Ok(format!("Finances: {}", changes.join("; ")))
        })
    }

    // --- managed club -----------------------------------------------------
    /// Select the club the user manages; returns the number of players under contract.
    pub fn set_managed_team(&mut self, index: usize) -> Result<usize, Error> {
        if !self.history.is_empty() {
            return Err(Error::new("Save or undo your changes before switching clubs."));
        }
        if index >= self.teams.len() {
            return Err(Error::new("Choose the club you manage."));
        }
        let roster = self.club_roster(index);
        let table = ContractTable::detect(&self.data, index, &roster)
            .map_err(|e| Error::new(format!("Couldn't read the contracts of {}: {e}", self.teams[index].name)))?;
        let count = table.contracts(&self.data, index).len();
        self.managed_team = Some(index);
        self.contracts = Some(table);
        Ok(count)
    }

    /// The player's contract with the managed club.
    pub fn contract_of(&self, player: &Player) -> Option<Contract> {
        let (table, club) = (self.contracts.as_ref()?, self.managed_team?);
        let found: Vec<Contract> = table
            .find(&self.data, player)
            .into_iter()
            .filter(|c| c.status == crate::contracts::STATUS_CONTRACTED && c.club_index() == club)
            .collect();
        (found.len() == 1).then(|| found[0])
    }

    pub fn contract_count(&self) -> usize {
        match (&self.contracts, self.managed_team) {
            (Some(t), Some(club)) => t.contracts(&self.data, club).len(),
            _ => 0,
        }
    }

    /// Annual wages of the managed club's contracts, in EUR.
    pub fn wage_bill_eur(&self) -> i64 {
        match (&self.contracts, self.managed_team) {
            (Some(t), Some(club)) => t.wage_bill_units(&self.data, club) as i64 * SALARY_UNIT as i64,
            _ => 0,
        }
    }

    /// Same month/day as the club's existing contracts, `seasons` years after the game date.
    pub fn contract_end_in(&self, seasons: u16) -> Date {
        let year = self.game_date.map(|d| d.year).unwrap_or_else(|| chrono::Datelike::year(&chrono::Local::now()) as u16);
        let (month, day) = match (&self.contracts, self.managed_team) {
            (Some(t), Some(club)) => t.contracts(&self.data, club).first().map_or((6, 30), |c| (c.end.month, c.end.day)),
            _ => (6, 30),
        };
        let year = (year + seasons).min(2150);
        Date::new(year, month, day).unwrap_or(Date { year, month: 6, day: 30 })
    }

    // --- transfers --------------------------------------------------------
    pub fn transfer(&mut self, req: TransferRequest) -> Result<&Operation, Error> {
        if let Source::Club(i) = req.source
            && i >= self.teams.len()
        {
            return Err(Error::new("The player's current club isn't valid."));
        }
        if req.dest >= self.teams.len() {
            return Err(Error::new("The destination must be a real club, not a national team."));
        }
        if req.source == Source::Club(req.dest) {
            return Err(Error::new("From and to are the same club."));
        }
        if req.fee_eur < 0 {
            return Err(Error::new("The fee can't be negative."));
        }
        let mine = self.managed_team;
        let incoming = Some(req.dest) == mine;
        let outgoing = matches!(req.source, Source::Club(i) if Some(i) == mine);
        if (incoming || outgoing) && self.contracts.is_none() {
            return Err(Error::new("First choose the club you manage."));
        }
        if (incoming || outgoing)
            && req.use_transfer_budget
            && req.fee_eur != 0
            && let Some(e) = &self.finance_error
        {
            return Err(Error::new(format!("{e} Untick \"Pay fee from transfer budget\" to continue.")));
        }
        let src_roster = self.roster(req.source);
        let dst_roster = self.club_roster(req.dest);
        let chosen: Vec<Player> = src_roster.iter().copied().filter(|p| p.player_id == req.player_id).collect();
        if chosen.len() != 1 {
            return Err(Error::new("Player not found (or listed twice) at their current club."));
        }
        let player = chosen[0];
        if dst_roster.len() >= MAX_ROSTER {
            return Err(Error::new(format!("{} already has 40 players.", self.teams[req.dest].name)));
        }
        if dst_roster.iter().any(|p| p.player_id == req.player_id) {
            return Err(Error::new("The player is already at the destination club."));
        }
        let elsewhere: Vec<String> = self
            .club_of(req.player_id)
            .into_iter()
            .filter(|&i| Source::Club(i) != req.source)
            .take(4)
            .map(|i| self.teams[i].name.clone())
            .collect();
        if !elsewhere.is_empty() {
            return Err(Error::new(format!("The player is also listed at another club: {}", elsewhere.join(", "))));
        }
        if incoming && (req.salary_eur.is_none() || req.contract_end.is_none()) {
            return Err(Error::new("Enter a salary and contract end date for a player joining your club."));
        }
        if incoming && self.game_date.is_none() {
            return Err(Error::new("Couldn't read the in-game date from the save, so no contract was created."));
        }
        let label = req.name.clone().unwrap_or_else(|| format!("ID {}", req.player_id));
        let header = format!("{label}: {} → {}", self.source_name(req.source), self.teams[req.dest].name);
        self.atomic(move |g| {
            let mut parts = vec![header];
            if outgoing {
                let table = g.contracts.clone().unwrap();
                let (offset, bytes, contract) = table.removal(&g.data, &player, mine.unwrap())?;
                g.patch(offset, &bytes);
                parts.push(format!("contract of {}/yr removed", fmt_eur(contract.annual_eur() as i64)));
                if req.use_transfer_budget && req.fee_eur != 0 {
                    g.adjust_transfer_budget(req.fee_eur)?;
                    parts.push(format!("fee received {}", fmt_eur(req.fee_eur)));
                }
            }
            if incoming {
                let (salary_eur, end) = (req.salary_eur.unwrap(), req.contract_end.unwrap());
                if req.use_transfer_budget && req.fee_eur != 0 {
                    g.adjust_transfer_budget(-req.fee_eur)?;
                    parts.push(format!("fee {}", fmt_eur(req.fee_eur)));
                }
                let units = u32::try_from(div_round_even(salary_eur, SALARY_UNIT as i64)).unwrap_or(0);
                let value = u32::try_from(eur_to_units(req.fee_eur)).unwrap_or(0);
                let salary = fmt_eur(units as i64 * SALARY_UNIT as i64);
                let table = g.contracts.clone().unwrap();
                // From a club: an agreed deal, and the game moves the player on the
                // next day. Saves whose transfer lists aren't recognised, and
                // players without a club (no sample of their deal yet), join right away.
                let tables = match req.source {
                    Source::Club(seller) => Some((seller, DealTables::locate(&g.data, &table))),
                    Source::Others => None,
                };
                if let Some((seller, Ok(tables))) = &tables {
                    let arrival = g.game_date.unwrap().next_day();
                    let deal = AgreedDeal {
                        player,
                        buyer_ref: table.team_ref,
                        seller_ref: u32_at(&g.data, Self::base(*seller)),
                        fee_units: value,
                        salary_units: units,
                        value_units: value,
                        end,
                        arrival,
                    };
                    for (offset, bytes) in tables.agree(&g.data, &table, &deal)? {
                        g.patch(offset, &bytes);
                    }
                    if !table.find(&g.data, &player).iter().any(|c| c.status == STATUS_AGREED && c.team == table.team_ref) {
                        return Err(Error::new("Final check failed: the agreed deal can't be read."));
                    }
                    parts.push(format!("deal agreed, arrives {arrival}; salary {salary}/yr until {end}"));
                    return Ok(parts.join("; "));
                }
                let (offset, record) = table.signing(&g.data, &player, units, value, end, g.game_date.unwrap())?;
                g.patch(offset, &record);
                parts.push(format!("salary {salary}/yr until {end}"));
                if matches!(tables, Some((_, Err(_)))) {
                    parts.push("joined right away (this save's transfer lists weren't recognised)".into());
                }
            }
            if let Source::Club(i) = req.source {
                let rest: Vec<Player> = src_roster.iter().copied().filter(|p| p.player_id != req.player_id).collect();
                g.write_roster(i, &rest)?;
            }
            let mut dest = dst_roster.clone();
            dest.push(player);
            g.write_roster(req.dest, &dest)?;
            if let Source::Club(i) = req.source
                && g.club_roster(i).contains(&player)
            {
                return Err(Error::new("Final check failed: the player is still at the old club."));
            }
            if !g.club_roster(req.dest).contains(&player) {
                return Err(Error::new("Final check failed: the player didn't join the new club."));
            }
            if incoming && g.contract_of(&player).is_none() {
                return Err(Error::new("Final check failed: the new contract can't be read."));
            }
            Ok(parts.join("; "))
        })
    }

    // --- saving -----------------------------------------------------------
    /// True when every byte that differs from the opened file lies in a journaled patch.
    fn changes_are_journaled(&self) -> bool {
        let original = &self.save.data;
        let mut masked = self.data.clone();
        for op in &self.history {
            for (offset, old) in &op.patches {
                masked[*offset..offset + old.len()].copy_from_slice(&original[*offset..offset + old.len()]);
            }
        }
        masked == *original
    }

    pub fn build(&self) -> Result<Vec<u8>, Error> {
        if !self.changes_are_journaled() {
            return Err(Error::new("Unexpected changes found in the save; nothing was saved."));
        }
        let mut save = self.save.clone();
        save.data = self.data.clone();
        let out = crypto::pack(&save);
        let check = crypto::unpack(&out, true)?;
        if check.data != save.data
            || check.desc != save.desc
            || check.logo != save.logo
            || check.serial != save.serial
            || check.file_header != save.file_header
        {
            return Err(Error::new("Couldn't confirm the new file is correct; nothing was written."));
        }
        Ok(out)
    }

    /// Write the edited save. Overwriting backs up the current file first.
    pub fn write(&mut self, target: Option<&Path>) -> Result<(PathBuf, Option<PathBuf>), Error> {
        if self.history.is_empty() {
            return Err(Error::new("There are no changes yet."));
        }
        let out = self.build()?;
        let target = match target {
            Some(t) => std::path::absolute(t).map_err(|e| Error::new(e.to_string()))?,
            None => self.path.clone(),
        };
        let io = |e: std::io::Error| Error::new(format!("Couldn't write the file: {e}"));
        let mut backup = None;
        if target.exists() {
            let folder = target.parent().unwrap_or(Path::new(".")).join(BACKUP_DIR);
            std::fs::create_dir_all(&folder).map_err(io)?;
            let name = target.file_name().unwrap_or_default().to_string_lossy();
            let file = folder.join(format!("{name}.{}.bak", chrono::Local::now().format("%Y%m%d-%H%M%S")));
            std::fs::copy(&target, &file).map_err(io)?;
            backup = Some(file);
        }
        let mut tmp = target.clone().into_os_string();
        tmp.push(".tmp");
        std::fs::write(&tmp, &out).map_err(io)?;
        std::fs::rename(&tmp, &target).map_err(io)?;
        let same = target == self.path;
        if same {
            self.save.data = self.data.clone();
            self.history.clear();
        }
        Ok((target, backup))
    }
}
