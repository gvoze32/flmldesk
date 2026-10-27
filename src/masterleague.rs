//! Master League save editing: club rosters, contracts and club finances.
//!
//! Layout of the decrypted `data` block (PES 2021 1.04+ and Football Life,
//! both run the same Master League code):
//!
//! * Club table at 0x50, 0x690 bytes per club: name +0x04, abbreviation +0x4A,
//!   roster of 40 `(internal_id, player_id)` pairs at +0x14C (empty =
//!   `(0xFFFF, 0)`), roster count in the low 7 bits of the u16 at +0x426,
//!   shirt numbers (u16, same order as the roster, 0 after the last player)
//!   at +0x29A.
//!   Clubs come first, then national teams (Albania/ALB first) up to The Old Boys.
//! * Finance block: transfer budget u32, three zero words, a word that is 0
//!   in the first season and later holds the transfer budget the season
//!   started with (left alone), then the annual salary budget u32 (+0x14),
//!   both budgets in units of 100 EUR. Football Life 2026
//!   keeps it at 0xC7DD00 (confirmed against the in-game value); PES 2021 at
//!   0xC7DE00 (ML Money Tool by Devil Cold52). The game shows the salary budget
//!   minus the club's wage bill, and charges the transfer fee when a deal is
//!   agreed. Manager name: UTF-8 at 0x1942C0, 46 bytes; read and edited
//!   separately from the budget, so one being unreadable doesn't lock the other.
//! * "My Team" list of the managed club at 0xB7F8BC: 16-byte rows
//!   `(internal id, player id, 0, flag)`, flag 0xC0 in the squad, 0xC1 just
//!   arrived, 0xC2 on loan, 0xC7 free; club reference at 0xB7F804.
//! * Player records: `playerdb`. Moves the editor makes itself (sales, moves
//!   between other clubs, releases, signings that join right away) and
//!   renewals update them like the game does.
//! * The save description holds the managed club, its league and the in-game
//!   date (`Chelsea FC / Premier League\n6/8/2025\n…`).
//! * League matches and tables: `league`.
//!
//! Every edit goes through `patch` (data block) or `patch_record` (player
//! records), which journal the previous bytes, so undo is byte-exact and
//! saving can prove that nothing outside the journal changed.

use std::collections::{BTreeMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

use crate::contracts::{Contract, ContractTable, Date, SALARY_UNIT, STATUS_AGREED};
use crate::deals::{AgreedDeal, DealTables, LoanDeal};
use crate::league::{Fixture, League, Standing};
use crate::playerdb::{self, Move, PlayerDb};
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
pub const NUMBERS_OFFSET: usize = 0x29A;
pub const MAX_ROSTER: usize = 40;
pub const EMPTY: Player = Player { internal_id: 0xFFFF, player_id: 0 };
const MAX_SAVE: u64 = 80_000_000;

pub const FL26_FINANCE: usize = 0xC7DD00;
pub const PES21_FINANCE: usize = 0xC7DE00;
pub const SALARY_BUDGET_OFFSET: usize = 0x14;
/// Transfer budget at the start of the season; 0 during the first season.
const SEASON_START_BUDGET_OFFSET: usize = 0x10;
pub const MANAGER_NAME: usize = 0x1942C0;
pub const MANAGER_NAME_SIZE: usize = 46;
pub const MY_TEAM_CLUB: usize = 0xB7_F804;
pub const MY_TEAM: usize = 0xB7_F8BC;
pub const MY_TEAM_ROW: usize = 16;
const MY_TEAM_ROWS: usize = 64;
const MY_TEAM_FREE: u32 = 0xC7;
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

/// Transfer and salary budget; the manager name is separate (`manager_name`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finances {
    pub transfer_eur: i64,
    /// Annual salary budget; the game shows this minus the wage bill.
    pub salary_eur: i64,
}

/// The managed club's league as the "League results" tab shows it.
#[derive(Clone, Debug)]
pub struct LeagueView {
    /// From the save description, or "League".
    pub name: String,
    /// The managed club's reference in `fixtures` and `standings`.
    pub club: u32,
    pub fixtures: Vec<Fixture>,
    pub standings: Vec<Standing>,
}

#[derive(Clone, Debug, Default)]
pub struct Operation {
    pub description: String,
    /// (offset, previous bytes) in write order.
    patches: Vec<(usize, Vec<u8>)>,
    /// Same for the player records.
    record_patches: Vec<(usize, Vec<u8>)>,
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

/// A loan to or from the managed club.
#[derive(Clone, Debug)]
pub struct LoanRequest {
    /// The other club: the one the player comes from (loan in) or goes to (loan out).
    pub club: usize,
    /// True when the managed club loans its own player out.
    pub outgoing: bool,
    pub player_id: u32,
    pub fee_eur: i64,
    pub loan_end: Date,
    /// Charge (loan in) or credit (loan out) the fee to the transfer budget.
    pub use_transfer_budget: bool,
    pub name: Option<String>,
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
    /// Why the manager name can't be edited; `None` when it can.
    pub manager_error: Option<String>,
    /// In-game date from the save description.
    pub game_date: Option<Date>,
    /// Managed club named in the save description.
    pub described_club: Option<usize>,
    /// That club's league, from the description.
    pub league_name: Option<String>,
    /// Player records as opened; `records` holds the edited copy.
    players: Option<PlayerDb>,
    pub records: Vec<u8>,
    /// Why the player records can't be read; selling, releasing, renewing and
    /// moving players between other clubs need them.
    pub players_error: Option<String>,
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
            manager_error: None,
            game_date: None,
            described_club: None,
            league_name: None,
            players: None,
            records: Vec::new(),
            players_error: None,
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
        game.manager_error = game.check_manager_name().err();
        game.read_description();
        game.load_players();
        Ok(game)
    }

    fn load_players(&mut self) {
        match PlayerDb::locate(&self.data) {
            Ok(db) => {
                self.records = db.records.clone();
                self.players = Some(db);
                self.players_error = None;
            }
            Err(e) => {
                self.records.clear();
                self.players = None;
                self.players_error = Some(format!(
                    "{e} Selling, releasing, renewing and moving players between other clubs are locked."
                ));
            }
        }
    }

    /// `"Chelsea FC / Premier League\n6/8/2025\nPremier League"` at desc+0x80.
    fn read_description(&mut self) {
        let raw = self.save.desc.get(0x80..).unwrap_or(&[]);
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let text = String::from_utf8_lossy(&raw[..end]).into_owned();
        let mut lines = text.lines();
        let mut first = lines.next().map(|l| l.split(" / "));
        if let Some(club) = first.as_mut().and_then(|parts| parts.next()) {
            let matches: Vec<usize> = self.teams.iter().filter(|t| t.name == club.trim()).map(|t| t.index).collect();
            self.described_club = (matches.len() == 1).then(|| matches[0]);
        }
        self.league_name = first.and_then(|mut parts| parts.next()).map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
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
        // No cross-variant fallback: the other game's offset can hold a look-alike block.
        let off = if self.game.contains("Football Life") { FL26_FINANCE } else { PES21_FINANCE };
        let plausible = self.data.len() >= off + SALARY_BUDGET_OFFSET + 4
            && (4..SEASON_START_BUDGET_OFFSET).step_by(4).all(|gap| u32_at(&self.data, off + gap) == 0)
            && u32_at(&self.data, off) < 0x8000_0000
            && (1..0x8000_0000).contains(&u32_at(&self.data, off + SALARY_BUDGET_OFFSET));
        if plausible { Ok(off) } else { Err("Couldn't find the budget in this save, so it can't be edited.".into()) }
    }

    /// The manager name field holds one NUL-padded line of UTF-8; error text otherwise.
    fn check_manager_name(&self) -> Result<(), String> {
        let raw = self.data.get(MANAGER_NAME..MANAGER_NAME + MANAGER_NAME_SIZE).unwrap_or(&[]);
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let name_ok = std::str::from_utf8(&raw[..end])
            .is_ok_and(|n| !n.trim().is_empty() && !n.chars().any(|c| c < ' '))
            && raw[end..].iter().all(|&b| b == 0);
        if !raw.is_empty() && name_ok {
            Ok(())
        } else {
            Err("Couldn't find the manager name in this save, so it can't be edited.".into())
        }
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

    fn patch_record(&mut self, offset: usize, new: &[u8]) {
        let op = self.current.as_mut().expect("write outside an operation");
        op.record_patches.push((offset, self.records[offset..offset + new.len()].to_vec()));
        self.records[offset..offset + new.len()].copy_from_slice(new);
    }

    fn revert(&mut self, op: &Operation) {
        for (offset, old) in op.patches.iter().rev() {
            self.data[*offset..offset + old.len()].copy_from_slice(old);
        }
        for (offset, old) in op.record_patches.iter().rev() {
            self.records[*offset..offset + old.len()].copy_from_slice(old);
        }
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
            self.revert(&op);
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
        self.revert(&op);
        Ok(op.description)
    }

    pub fn operations(&self) -> Vec<&str> {
        self.history.iter().map(|o| o.description.as_str()).collect()
    }

    fn shirt_numbers(&self, index: usize) -> Vec<u16> {
        let base = Self::base(index) + NUMBERS_OFFSET;
        (0..MAX_ROSTER).map(|j| u16_at(&self.data, base + 2 * j)).collect()
    }

    /// Squad and shirt numbers (same order; zeros after the last player), as the game keeps them.
    fn write_squad(&mut self, index: usize, players: &[Player], numbers: &[u16]) -> Result<(), Error> {
        if players.len() > MAX_ROSTER || numbers.len() != players.len() {
            return Err(Error::new("The destination squad would go over 40 players."));
        }
        let base = Self::base(index);
        let mut bytes = Vec::with_capacity(MAX_ROSTER * 8);
        for p in players.iter().chain(std::iter::repeat_n(&EMPTY, MAX_ROSTER - players.len())) {
            bytes.extend(p.internal_id.to_le_bytes());
            bytes.extend(p.player_id.to_le_bytes());
        }
        self.patch(base + ROSTER_OFFSET, &bytes);
        let numbers: Vec<u8> =
            numbers.iter().chain(std::iter::repeat_n(&0, MAX_ROSTER - numbers.len())).flat_map(|n| n.to_le_bytes()).collect();
        self.patch(base + NUMBERS_OFFSET, &numbers);
        let word = u16_at(&self.data, base + COUNT_OFFSET);
        self.patch(base + COUNT_OFFSET, &((word & !COUNT_MASK) | players.len() as u16).to_le_bytes());
        Ok(())
    }

    /// Take the player out of a club's squad; later players and their shirt
    /// numbers move up. Returns his shirt number.
    fn remove_from_squad(&mut self, index: usize, player: &Player) -> Result<u16, Error> {
        let mut squad = self.club_roster(index);
        let mut numbers = self.shirt_numbers(index);
        numbers.truncate(squad.len());
        let k = squad.iter().position(|p| p == player).ok_or_else(|| Error::new("Player not found at his club."))?;
        squad.remove(k);
        let number = numbers.remove(k);
        self.write_squad(index, &squad, &numbers)?;
        Ok(number)
    }

    /// Add the player at the end of a club's squad with his old shirt number
    /// when it is free there (as the game does), otherwise the lowest free one.
    fn add_to_squad(&mut self, index: usize, player: Player, number: Option<u16>) -> Result<u16, Error> {
        let mut squad = self.club_roster(index);
        let mut numbers = self.shirt_numbers(index);
        numbers.truncate(squad.len());
        let taken: HashSet<u16> = numbers.iter().copied().collect();
        let number = number
            .filter(|n| (1..=99).contains(n) && !taken.contains(n))
            .or_else(|| (1..=99).find(|n| !taken.contains(n)))
            .ok_or_else(|| Error::new(format!("{} has no free shirt number.", self.teams[index].name)))?;
        squad.push(player);
        numbers.push(number);
        self.write_squad(index, &squad, &numbers)?;
        Ok(number)
    }

    /// Remove the player's row from the managed club's "My Team" list (later
    /// rows move up), as the list holds exactly the squad plus loans. Lists
    /// of another club, or without the player, are left alone.
    fn drop_from_my_team(&mut self, club: usize, player: &Player) {
        let Some(end) = MY_TEAM.checked_add(MY_TEAM_ROWS * MY_TEAM_ROW).filter(|e| *e <= self.data.len()) else { return };
        if u32_at(&self.data, MY_TEAM_CLUB) != u32_at(&self.data, Self::base(club)) {
            return;
        }
        let row = |k: usize| MY_TEAM + k * MY_TEAM_ROW;
        let used = (0..MY_TEAM_ROWS).take_while(|&k| u32_at(&self.data, row(k) + 12) & 0xFF != MY_TEAM_FREE).count();
        let found = (0..used).find(|&k| {
            u32_at(&self.data, row(k)) == player.internal_id && u32_at(&self.data, row(k) + 4) == player.player_id
        });
        let Some(k) = found else { return };
        let mut bytes = self.data[row(k + 1)..row(used)].to_vec();
        bytes.extend([0xFFFFu32, 0, 0, MY_TEAM_FREE].iter().flat_map(|w| w.to_le_bytes()));
        debug_assert!(row(k) + bytes.len() <= end);
        self.patch(row(k), &bytes);
    }

    /// Rewrite the player's record with `change`.
    fn update_record(&mut self, player: &Player, change: impl FnOnce(&[u8]) -> Result<Vec<u8>, Error>) -> Result<(), Error> {
        let db = self.players.as_ref().ok_or_else(|| Error::new(self.players_error.clone().unwrap_or_default()))?;
        let off = db.record(&self.records, player)?;
        let new = change(&self.records[off..off + playerdb::RECORD])?;
        self.patch_record(off, &new);
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

    pub fn set_finances(&mut self, transfer_eur: i64, salary_eur: i64) -> Result<&Operation, Error> {
        let before = self.finances()?;
        let off = self.finance_at()?;
        if transfer_eur == before.transfer_eur && salary_eur == before.salary_eur {
            return Err(Error::new("Nothing was changed."));
        }
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
            Ok(format!("Finances: {}", changes.join("; ")))
        })
    }

    // --- manager name -------------------------------------------------------
    pub fn manager_name(&self) -> Result<String, Error> {
        match &self.manager_error {
            Some(e) => Err(Error::new(e.clone())),
            None => Ok(text_at(&self.data, MANAGER_NAME, MANAGER_NAME_SIZE)),
        }
    }

    pub fn set_manager_name(&mut self, manager_name: &str) -> Result<&Operation, Error> {
        let before = self.manager_name()?;
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
        if name == before {
            return Err(Error::new("Nothing was changed."));
        }
        let name = name.to_string();
        self.atomic(move |g| {
            let mut bytes = name.as_bytes().to_vec();
            bytes.resize(MANAGER_NAME_SIZE, 0);
            g.patch(MANAGER_NAME, &bytes);
            Ok(format!("Manager: \"{before}\" → \"{name}\""))
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
        let Some(today) = self.game_date else {
            return Err(Error::new("Couldn't read the in-game date from the save, so nothing was changed."));
        };
        if outgoing {
            self.refuse_agreed_deal(self.contracts.as_ref().unwrap(), &player)?;
        }
        let label = req.name.clone().unwrap_or_else(|| format!("ID {}", req.player_id));
        let header = format!("{label}: {} → {}", self.source_name(req.source), self.teams[req.dest].name);
        self.atomic(move |g| {
            let mut parts = vec![header];
            let mut new_contract = None;
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
                // An agreed deal: the game moves the player on the next day.
                // Saves whose transfer lists aren't recognised join right away.
                match DealTables::locate(&g.data, &table) {
                    Ok(tables) => {
                        let arrival = today.next_day();
                        let deal = AgreedDeal {
                            player,
                            buyer_ref: table.team_ref,
                            seller_ref: match req.source {
                                Source::Club(seller) => Some(u32_at(&g.data, Self::base(seller))),
                                Source::Others => None,
                            },
                            fee_units: value,
                            salary_units: units,
                            value_units: value,
                            end,
                            today,
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
                    Err(_) => {
                        let (offset, record) = table.signing(&g.data, &player, units, value, end, today)?;
                        g.patch(offset, &record);
                        new_contract = Some((units, end));
                        parts.push(format!("salary {salary}/yr until {end}"));
                        parts.push("joined right away (this save's transfer lists weren't recognised)".into());
                    }
                }
            }
            // The editor moves the player itself, so it also does what the game
            // does on a move: his record, the shirt numbers and "My Team".
            let from = match req.source {
                Source::Club(i) => Some(u32_at(&g.data, Self::base(i))),
                Source::Others => None,
            };
            g.update_record(&player, |r| playerdb::moved(r, &player, &Move { from, joined: today, contract: new_contract }))?;
            let number = match req.source {
                Source::Club(i) => Some(g.remove_from_squad(i, &player)?),
                Source::Others => None,
            };
            let number = g.add_to_squad(req.dest, player, number)?;
            if outgoing {
                g.drop_from_my_team(mine.unwrap(), &player);
            }
            parts.push(format!("shirt number {number}"));
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

    /// Managed club, its contract table and the in-game date, needed to edit contracts.
    fn contract_context(&self) -> Result<(usize, ContractTable, Date), Error> {
        let (Some(club), Some(table)) = (self.managed_team, self.contracts.clone()) else {
            return Err(Error::new("First choose the club you manage."));
        };
        let today = self.game_date.ok_or_else(|| Error::new("Couldn't read the in-game date from the save."))?;
        Ok((club, table, today))
    }

    /// Refuses a player the game already has an agreed deal for (a sale or loan waiting for the next day).
    fn refuse_agreed_deal(&self, table: &ContractTable, player: &Player) -> Result<(), Error> {
        match DealTables::locate(&self.data, table) {
            Ok(tables) if tables.has_agreed_offer(&self.data, player) => Err(Error::new(format!(
                "Player ID {} already has an agreed deal (for example a loan) that the game completes on the next day.",
                player.player_id
            ))),
            _ => Ok(()),
        }
    }

    /// Agree a loan to or from the managed club. As in the game, the fee is
    /// paid (or received) now and the player moves on the next in-game day;
    /// his contract stays with the club that loans him out.
    pub fn loan(&mut self, req: LoanRequest) -> Result<&Operation, Error> {
        let (mine, table, today) = self.contract_context()?;
        if req.club >= self.teams.len() || req.club == mine {
            return Err(Error::new("Choose another club (not a national team) for the loan."));
        }
        if req.fee_eur < 0 {
            return Err(Error::new("The fee can't be negative."));
        }
        let pay = req.use_transfer_budget && req.fee_eur != 0;
        if pay && let Some(e) = &self.finance_error {
            return Err(Error::new(format!("{e} Untick \"Pay fee from transfer budget\" to continue.")));
        }
        let fee_units = u32::try_from(eur_to_units(req.fee_eur))
            .ok()
            .filter(|u| *u < 0x8000_0000)
            .ok_or_else(|| Error::new("The fee is too high for the game."))?;
        let (from, to) = if req.outgoing { (mine, req.club) } else { (req.club, mine) };
        let chosen: Vec<Player> = self.club_roster(from).into_iter().filter(|p| p.player_id == req.player_id).collect();
        if chosen.len() != 1 {
            return Err(Error::new("Player not found (or listed twice) at their current club."));
        }
        let player = chosen[0];
        if self.club_of(req.player_id) != [from] {
            return Err(Error::new("The player is listed at more than one club."));
        }
        if self.club_roster(to).len() >= MAX_ROSTER {
            return Err(Error::new(format!("{} already has 40 players.", self.teams[to].name)));
        }
        if req.outgoing {
            if self.contract_of(&player).is_none() {
                return Err(Error::new("Only players under contract with your club can be loaned out (not players on loan)."));
            }
            self.refuse_agreed_deal(&table, &player)?;
        } else if !table.find(&self.data, &player).is_empty() {
            return Err(Error::new(format!(
                "Player ID {} is already in a negotiation or under contract in the game; finish or cancel that there first.",
                player.player_id
            )));
        }
        let tables = DealTables::locate(&self.data, &table)?;
        let club_ref = |i: usize| if i == mine { table.team_ref } else { u32_at(&self.data, Self::base(i)) };
        let arrival = today.next_day();
        let deal = LoanDeal {
            player,
            buyer_ref: club_ref(to),
            seller_ref: club_ref(from),
            outgoing: req.outgoing,
            fee_units,
            loan_end: req.loan_end,
            today,
            arrival,
        };
        let label = req.name.clone().unwrap_or_else(|| format!("ID {}", req.player_id));
        let header = format!(
            "{label}: loan {} → {} until {}, moves {arrival}",
            self.teams[from].name, self.teams[to].name, req.loan_end
        );
        self.atomic(move |g| {
            for (offset, bytes) in tables.agree_loan(&g.data, &table, &deal)? {
                g.patch(offset, &bytes);
            }
            let mut parts = vec![header];
            if pay {
                g.adjust_transfer_budget(if req.outgoing { req.fee_eur } else { -req.fee_eur })?;
                parts.push(format!("fee {} {}", if req.outgoing { "received" } else { "paid" }, fmt_eur(req.fee_eur)));
            }
            if !DealTables::locate(&g.data, &table)?.has_agreed_offer(&g.data, &player) {
                return Err(Error::new("Final check failed: the agreed loan can't be read."));
            }
            Ok(parts.join("; "))
        })
    }

    /// The managed club's squad with each player's contract (`None` when it can't be read).
    pub fn squad_contracts(&self) -> Vec<(Player, Option<Contract>)> {
        let Some(club) = self.managed_team else { return Vec::new() };
        self.club_roster(club).into_iter().map(|p| (p, self.contract_of(&p))).collect()
    }

    /// New yearly salary and end date for a player under contract with the managed club.
    pub fn renew_contract(&mut self, player: Player, salary_eur: i64, end: Date, name: Option<&str>) -> Result<&Operation, Error> {
        let (club, table, today) = self.contract_context()?;
        if salary_eur < 0 {
            return Err(Error::new("The salary can't be negative."));
        }
        let units = u32::try_from(div_round_even(salary_eur, SALARY_UNIT as i64)).unwrap_or(u32::MAX);
        let old = self.contract_of(&player).ok_or_else(|| Error::new("This player's contract couldn't be found; nothing was changed."))?;
        if old.salary_units == units && old.end == end {
            return Err(Error::new("Nothing was changed."));
        }
        let label = name.map_or_else(|| format!("ID {}", player.player_id), str::to_string);
        self.atomic(move |g| {
            let (offset, bytes, _) = table.renewal(&g.data, &player, club, units, end, today)?;
            g.patch(offset, &bytes);
            // The player's record mirrors the contract's salary and end date.
            g.update_record(&player, |r| Ok(playerdb::renewed(r, units, end)))?;
            let new = g.contract_of(&player).filter(|c| c.salary_units == units && c.end == end);
            if new.is_none() {
                return Err(Error::new("Final check failed: the new contract can't be read."));
            }
            Ok(format!(
                "{label}: contract {}/yr until {} → {}/yr until {end}",
                fmt_eur(old.annual_eur() as i64),
                old.end,
                fmt_eur(units as i64 * SALARY_UNIT as i64)
            ))
        })
    }

    /// End a player's contract with the managed club and take him out of the
    /// squad, without a new club. `compensation_eur` is paid from the transfer
    /// budget when `use_transfer_budget` is set.
    pub fn release(
        &mut self,
        player: Player,
        compensation_eur: i64,
        use_transfer_budget: bool,
        name: Option<&str>,
    ) -> Result<&Operation, Error> {
        let (club, table, _) = self.contract_context()?;
        if compensation_eur < 0 {
            return Err(Error::new("The compensation can't be negative."));
        }
        let pay = use_transfer_budget && compensation_eur != 0;
        if pay && let Some(e) = &self.finance_error {
            return Err(Error::new(format!("{e} Untick \"Pay from transfer budget\" to continue.")));
        }
        let roster = self.club_roster(club);
        if roster.iter().filter(|p| **p == player).count() != 1 {
            return Err(Error::new("Player not found (or listed twice) in your squad."));
        }
        self.refuse_agreed_deal(&table, &player)?;
        let label = name.map_or_else(|| format!("ID {}", player.player_id), str::to_string);
        self.atomic(move |g| {
            let (offset, bytes, contract) = table.removal(&g.data, &player, club)?;
            g.patch(offset, &bytes);
            let mut parts = vec![
                format!("{label}: released from {}", g.teams[club].name),
                format!("contract of {}/yr until {} removed", fmt_eur(contract.annual_eur() as i64), contract.end),
            ];
            if pay {
                g.adjust_transfer_budget(-compensation_eur)?;
                parts.push(format!("compensation {}", fmt_eur(compensation_eur)));
            }
            g.update_record(&player, |r| playerdb::released(r, &player))?;
            g.remove_from_squad(club, &player)?;
            g.drop_from_my_team(club, &player);
            if g.club_roster(club).contains(&player) || g.contract_of(&player).is_some() {
                return Err(Error::new("Final check failed: the player is still at your club."));
            }
            Ok(parts.join("; "))
        })
    }

    // --- league results ---------------------------------------------------
    /// A club's reference, as the match and league tables store it.
    fn club_ref(&self, index: usize) -> u32 {
        u32_at(&self.data, Self::base(index))
    }

    /// Name of the club with this reference (low 14 bits = club index).
    pub fn club_name(&self, reference: u32) -> String {
        let index = (reference & 0x3FFF) as usize;
        match self.teams.get(index) {
            Some(t) if self.club_ref(index) == reference => t.name.clone(),
            _ => format!("Club {reference:#x}"),
        }
    }

    fn managed_league(&self) -> Result<(usize, u32, League), Error> {
        let club = self.managed_team.ok_or_else(|| Error::new("First choose the club you manage."))?;
        let me = self.club_ref(club);
        Ok((club, me, League::of(&self.data, me)?))
    }

    fn league_label(&self, club: usize) -> String {
        self.league_name.clone().filter(|_| self.described_club == Some(club)).unwrap_or_else(|| "League".into())
    }

    /// The managed club's league matches and its table.
    pub fn league(&self) -> Result<LeagueView, Error> {
        let (club, me, league) = self.managed_league()?;
        Ok(LeagueView {
            name: self.league_label(club),
            club: me,
            fixtures: league.fixtures_of(&self.data, me),
            standings: league.table.standings(&self.data),
        })
    }

    /// Turn a played league match of the managed club into a win with this
    /// score; the league table is recalculated. Scorers, player stats and
    /// everything else the game did after the match stay as they are.
    pub fn set_league_win(&mut self, index: usize, our_goals: u8, their_goals: u8) -> Result<&Operation, Error> {
        let (club, me, league) = self.managed_league()?;
        let f = league
            .fixtures_of(&self.data, me)
            .into_iter()
            .find(|f| f.index == index)
            .ok_or_else(|| Error::new("This isn't one of your club's league matches."))?;
        if !f.played {
            return Err(Error::new("This match hasn't been played yet: play it in the game first, then make it a win here."));
        }
        if our_goals <= their_goals {
            return Err(Error::new("Choose a winning score: your goals must be more than the opponent's."));
        }
        let (home, away) = if f.home == me { (our_goals, their_goals) } else { (their_goals, our_goals) };
        if (home, away) == (f.home_goals, f.away_goals) {
            return Err(Error::new("Nothing was changed."));
        }
        let patches = league.new_score(&self.data, index, home, away)?;
        let label = self.league_label(club);
        self.atomic(move |g| {
            for (offset, bytes) in &patches {
                g.patch(*offset, bytes);
            }
            let after = league.matches.fixture(&g.data, index);
            if (after.home_goals, after.away_goals) != (home, away) || !after.played {
                return Err(Error::new("Final check failed: the new score can't be read."));
            }
            Ok(format!(
                "{label} round {}: {} {}–{} {} → {home}–{away}",
                f.round,
                g.club_name(f.home),
                f.home_goals,
                f.away_goals,
                g.club_name(f.away)
            ))
        })
    }

    // --- saving -----------------------------------------------------------
    /// True when every byte that differs from the opened file lies in a journaled patch.
    fn changes_are_journaled(&self) -> bool {
        let mask = |current: &[u8], original: &[u8], patches: &mut dyn Iterator<Item = &(usize, Vec<u8>)>| {
            let mut masked = current.to_vec();
            for (offset, old) in patches {
                masked[*offset..offset + old.len()].copy_from_slice(&original[*offset..offset + old.len()]);
            }
            masked == original
        };
        let data_ok = mask(&self.data, &self.save.data, &mut self.history.iter().flat_map(|op| &op.patches));
        let records_ok = match &self.players {
            Some(db) => mask(&self.records, &db.records, &mut self.history.iter().flat_map(|op| &op.record_patches)),
            None => self.records.is_empty(),
        };
        data_ok && records_ok
    }

    pub fn build(&self) -> Result<Vec<u8>, Error> {
        self.build_save().map(|(out, _)| out)
    }

    /// The encrypted file and the save it decrypts to. Changed player records
    /// are compressed back into their container, which can change its size.
    fn build_save(&self) -> Result<(Vec<u8>, crypto::SaveFile), Error> {
        if !self.changes_are_journaled() {
            return Err(Error::new("Unexpected changes found in the save; nothing was saved."));
        }
        let mut save = self.save.clone();
        let edited = self.players.as_ref().filter(|db| db.records != self.records);
        save.data = match edited {
            Some(db) => db.splice(&self.data, &self.records)?,
            None => self.data.clone(),
        };
        let size = u32::try_from(save.data.len()).map_err(|_| Error::new("The save would be too large."))?;
        save.file_header[64..68].copy_from_slice(&size.to_le_bytes());
        let out = crypto::pack(&save);
        let check = crypto::unpack(&out, true)?;
        let spliced_ok = edited.is_none_or(|db| db.check_splice(&self.data, &check.data, &self.records));
        if check.data != save.data
            || !spliced_ok
            || check.desc != save.desc
            || check.logo != save.logo
            || check.serial != save.serial
            || check.file_header != save.file_header
        {
            return Err(Error::new("Couldn't confirm the new file is correct; nothing was written."));
        }
        Ok((out, check))
    }

    /// Write the edited save. Overwriting backs up the current file first.
    pub fn write(&mut self, target: Option<&Path>) -> Result<(PathBuf, Option<PathBuf>), Error> {
        if self.history.is_empty() {
            return Err(Error::new("There are no changes yet."));
        }
        let (out, written) = self.build_save()?;
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
            // The file on disk is now the base: tables after the player records
            // may have moved, so they are found again.
            self.data = written.data.clone();
            self.save = written;
            self.history.clear();
            self.load_players();
            if let Some(club) = self.managed_team {
                self.contracts = ContractTable::detect(&self.data, club, &self.club_roster(club)).ok();
                if self.contracts.is_none() {
                    self.managed_team = None;
                }
            }
        }
        Ok((target, backup))
    }
}
