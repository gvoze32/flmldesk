//! League matches and tables: the managed club's league results, and
//! changing one of them into a win.
//!
//! Layout of the decrypted `data` block, matched against the first matchday
//! of a Football Life 2026 save (`research/negow-1` → `negow-2`: all 10
//! Premier League scores and the 20 table rows, plus the 341 table rows of
//! the other leagues that had played):
//!
//! * Match table (`0x329B00` in the samples): one 596-byte record per match of
//!   every competition, in index order. Words: index; state << 28 | round << 16
//!   | competition (state 2 = to be played, 6 = played; round from 0); date
//!   (`year | month << 16 | day << 24`); two unknown words; home club
//!   reference; away club reference; score (byte 0 home goals, byte 3 away
//!   goals); then the line-ups.
//! * League table, one block per competition (`0xCB389C` for the Premier
//!   League): header `(year << 16 | competition, competition, format, 1, …)`,
//!   format 1 for a plain league; from +0x14 48 team rows of 16 bytes (club
//!   reference first, `0xFFFFFFFF` when free); from +0x318 one 20-byte row per
//!   club in table order: club, position, `points | won << 8 | lost << 14 |
//!   drawn << 20 | away wins << 26`, `goals for | goals against << 12 |
//!   played << 24`, away goals for.
//!
//! The game ranks the Premier League by points, goal difference and goals
//! (equal clubs share a position) and Serie A by points only; a table that
//! follows neither, or that doesn't add up to its played matches (a points
//! deduction, say), is never changed.

use std::collections::HashMap;

use crate::Error;
use crate::contracts::Date;

pub const MATCH_RECORD: usize = 596;
const STATE_SHIFT: u32 = 28;
const STATE_SCHEDULED: u32 = 2;
const STATE_PLAYED: u32 = 6;
const DATE: usize = 8;
const HOME: usize = 20;
const AWAY: usize = 24;
const SCORE: usize = 28;
/// Bytes 1 and 2 of the score word; zero in every league match of the samples.
const SCORE_OTHER: u32 = 0x00FF_FF00;
/// Records checked when looking for the start of the match table.
const PROBE: usize = 4;

const FORMAT_LEAGUE: u32 = 1;
const TEAM_ROWS: usize = 0x14;
const TEAM_ROW: usize = 16;
const MAX_TEAMS: usize = 48;
const STANDINGS: usize = 0x318;
const STANDING_ROW: usize = 20;
const NO_CLUB: u32 = u32::MAX;
pub const MAX_GOALS: u8 = 99;

fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// One match of the match table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub index: usize,
    pub competition: u16,
    /// Round number as the game shows it (from 1).
    pub round: u16,
    pub date: Option<Date>,
    pub home: u32,
    pub away: u32,
    pub played: bool,
    pub home_goals: u8,
    pub away_goals: u8,
}

impl Fixture {
    pub fn involves(&self, club: u32) -> bool {
        self.home == club || self.away == club
    }

    /// (goals of `club`, goals of the other side).
    pub fn goals_of(&self, club: u32) -> (u8, u8) {
        if self.home == club { (self.home_goals, self.away_goals) } else { (self.away_goals, self.home_goals) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchTable {
    pub start: usize,
    pub count: usize,
}

impl MatchTable {
    /// The only place where `PROBE` consecutive records numbered from 0 with a
    /// valid state and date start the table.
    pub fn locate(data: &[u8]) -> Result<MatchTable, Error> {
        let probe = |o: usize| {
            (0..PROBE).all(|k| {
                let r = o + k * MATCH_RECORD;
                u32_at(data, r) == k as u32
                    && matches!(u32_at(data, r + 4) >> STATE_SHIFT, STATE_SCHEDULED | STATE_PLAYED)
                    && Date::unpack(u32_at(data, r + DATE)).is_some()
            })
        };
        let last = data.len().saturating_sub(PROBE * MATCH_RECORD);
        let mut found = (0..last).step_by(4).filter(|&o| u32_at(data, o) == 0 && u32_at(data, o + MATCH_RECORD) == 1 && probe(o));
        let start = found.next().ok_or_else(|| Error::new("Couldn't find the matches in this save."))?;
        if found.next().is_some() {
            return Err(Error::new("The matches in this save couldn't be told apart from other data."));
        }
        let count = (0..)
            .take_while(|&k| {
                let r = start + k * MATCH_RECORD;
                r + MATCH_RECORD <= data.len() && u32_at(data, r) == k as u32
            })
            .count();
        Ok(MatchTable { start, count })
    }

    fn offset(&self, index: usize) -> usize {
        self.start + index * MATCH_RECORD
    }

    pub fn fixture(&self, data: &[u8], index: usize) -> Fixture {
        let o = self.offset(index);
        let head = u32_at(data, o + 4);
        let score = u32_at(data, o + SCORE);
        Fixture {
            index,
            competition: head as u16,
            round: ((head >> 16) & 0xFF) as u16 + 1,
            date: Date::unpack(u32_at(data, o + DATE)),
            home: u32_at(data, o + HOME),
            away: u32_at(data, o + AWAY),
            played: head >> STATE_SHIFT == STATE_PLAYED,
            home_goals: score as u8,
            away_goals: (score >> 24) as u8,
        }
    }

    pub fn fixtures<'a>(&'a self, data: &'a [u8]) -> impl Iterator<Item = Fixture> + 'a {
        (0..self.count).map(move |k| self.fixture(data, k))
    }
}

/// A club's league record as the table stores it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    pub played: u32,
    pub won: u32,
    pub drawn: u32,
    pub lost: u32,
    pub away_won: u32,
    pub goals_for: u32,
    pub goals_against: u32,
    pub away_goals_for: u32,
    pub points: u32,
}

impl Record {
    fn decode(a: u32, b: u32, c: u32) -> Record {
        Record {
            points: a & 0xFF,
            won: (a >> 8) & 0x3F,
            lost: (a >> 14) & 0x3F,
            drawn: (a >> 20) & 0x3F,
            away_won: a >> 26,
            goals_for: b & 0xFFF,
            goals_against: (b >> 12) & 0xFFF,
            played: b >> 24,
            away_goals_for: c,
        }
    }

    fn encode(&self) -> Result<[u32; 3], Error> {
        let fits = self.points <= 0xFF
            && [self.won, self.lost, self.drawn, self.away_won].iter().all(|v| *v <= 0x3F)
            && self.goals_for <= 0xFFF
            && self.goals_against <= 0xFFF
            && self.played <= 0xFF;
        if !fits {
            return Err(Error::new("The league table can't hold these numbers."));
        }
        Ok([
            self.points | self.won << 8 | self.lost << 14 | self.drawn << 20 | self.away_won << 26,
            self.goals_for | self.goals_against << 12 | self.played << 24,
            self.away_goals_for,
        ])
    }

    fn add(&mut self, scored: u8, conceded: u8, away: bool) {
        let (scored, conceded) = (scored as u32, conceded as u32);
        self.played += 1;
        self.goals_for += scored;
        self.goals_against += conceded;
        if away {
            self.away_goals_for += scored;
        }
        match scored.cmp(&conceded) {
            std::cmp::Ordering::Greater => {
                self.won += 1;
                self.points += 3;
                self.away_won += away as u32;
            }
            std::cmp::Ordering::Equal => {
                self.drawn += 1;
                self.points += 1;
            }
            std::cmp::Ordering::Less => self.lost += 1,
        }
    }

    pub fn goal_difference(&self) -> i64 {
        self.goals_for as i64 - self.goals_against as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Standing {
    pub club: u32,
    pub position: u32,
    pub record: Record,
}

/// How a league breaks ties between clubs on the same points.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    /// Goal difference, then goals scored (Premier League).
    GoalDifference,
    /// Not at all: equal points share a position (Serie A).
    Points,
}

impl Order {
    fn key(self, r: &Record) -> (i64, i64, i64) {
        match self {
            Order::GoalDifference => (r.points as i64, r.goal_difference(), r.goals_for as i64),
            Order::Points => (r.points as i64, 0, 0),
        }
    }

    /// Positions of `rows` (in table order): 1 + the clubs ahead of each one.
    fn positions(self, rows: &[Standing]) -> Vec<u32> {
        rows.iter()
            .map(|r| 1 + rows.iter().filter(|q| self.key(&q.record) > self.key(&r.record)).count() as u32)
            .collect()
    }

    /// The order the stored table follows, if any.
    fn of(rows: &[Standing]) -> Option<Order> {
        [Order::GoalDifference, Order::Points].into_iter().find(|order| {
            rows.windows(2).all(|w| order.key(&w[0].record) >= order.key(&w[1].record))
                && order.positions(rows).iter().zip(rows).all(|(p, r)| *p == r.position)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeagueTable {
    pub header: usize,
    pub competition: u16,
    /// Clubs in the order of the team rows.
    pub clubs: Vec<u32>,
}

impl LeagueTable {
    fn at(data: &[u8], o: usize) -> Option<LeagueTable> {
        let tag = u32_at(data, o);
        let competition = u32_at(data, o + 4);
        if !(1900..=2200).contains(&(tag >> 16))
            || tag & 0xFFFF != competition
            || competition == 0
            || u32_at(data, o + 8) != FORMAT_LEAGUE
            || o + STANDINGS + MAX_TEAMS * STANDING_ROW > data.len()
        {
            return None;
        }
        let rows: Vec<u32> = (0..MAX_TEAMS).map(|j| u32_at(data, o + TEAM_ROWS + j * TEAM_ROW)).collect();
        let n = rows.iter().take_while(|c| **c != NO_CLUB).count();
        if n < 2 || rows[n..].iter().any(|c| *c != NO_CLUB) {
            return None;
        }
        let clubs = rows[..n].to_vec();
        // The table rows hold the same clubs as the team rows, each once.
        let mut listed = clubs.clone();
        let mut ranked: Vec<u32> = (0..n).map(|j| u32_at(data, o + STANDINGS + j * STANDING_ROW)).collect();
        listed.sort_unstable();
        ranked.sort_unstable();
        if listed != ranked || listed.windows(2).any(|w| w[0] == w[1]) {
            return None;
        }
        Some(LeagueTable { header: o, competition: competition as u16, clubs })
    }

    /// Every plain league table in the save.
    pub fn find_all(data: &[u8]) -> Vec<LeagueTable> {
        let last = data.len().saturating_sub(12);
        (0..last).step_by(4).filter_map(|o| LeagueTable::at(data, o)).collect()
    }

    pub fn standings(&self, data: &[u8]) -> Vec<Standing> {
        (0..self.clubs.len())
            .map(|j| {
                let o = self.header + STANDINGS + j * STANDING_ROW;
                Standing {
                    club: u32_at(data, o),
                    position: u32_at(data, o + 4),
                    record: Record::decode(u32_at(data, o + 8), u32_at(data, o + 12), u32_at(data, o + 16)),
                }
            })
            .collect()
    }
}

/// The league a club plays in: the plain league table with the most of its matches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct League {
    pub matches: MatchTable,
    pub table: LeagueTable,
}

impl League {
    pub fn of(data: &[u8], club: u32) -> Result<League, Error> {
        let matches = MatchTable::locate(data)?;
        let mut counts: HashMap<u16, usize> = HashMap::new();
        for f in matches.fixtures(data).filter(|f| f.involves(club)) {
            *counts.entry(f.competition).or_default() += 1;
        }
        LeagueTable::find_all(data)
            .into_iter()
            .filter(|t| t.clubs.contains(&club))
            .max_by_key(|t| counts.get(&t.competition).copied().unwrap_or(0))
            .map(|table| League { matches, table })
            .ok_or_else(|| Error::new("Couldn't find a league table with your club in this save."))
    }

    /// The club's league matches, in match order.
    pub fn fixtures_of(&self, data: &[u8], club: u32) -> Vec<Fixture> {
        self.matches.fixtures(data).filter(|f| f.competition == self.table.competition && f.involves(club)).collect()
    }

    /// Records of the table's clubs from the played matches, with the score of
    /// match `changed` replaced when given.
    fn records(&self, data: &[u8], changed: Option<(usize, u8, u8)>) -> Result<HashMap<u32, Record>, Error> {
        let mut records: HashMap<u32, Record> = self.table.clubs.iter().map(|c| (*c, Record::default())).collect();
        for f in self.matches.fixtures(data).filter(|f| f.played && f.competition == self.table.competition) {
            let (home_goals, away_goals) = match changed {
                Some((index, h, a)) if index == f.index => (h, a),
                _ => (f.home_goals, f.away_goals),
            };
            for (club, scored, conceded, away) in [(f.home, home_goals, away_goals, false), (f.away, away_goals, home_goals, true)] {
                records
                    .get_mut(&club)
                    .ok_or_else(|| Error::new("A league match involves a club that isn't in the league table."))?
                    .add(scored, conceded, away);
            }
        }
        Ok(records)
    }

    /// The bytes to write for a new score of a played league match: the score
    /// word and the whole table, re-sorted the way the league sorts it.
    pub fn new_score(&self, data: &[u8], index: usize, home_goals: u8, away_goals: u8) -> Result<Vec<(usize, Vec<u8>)>, Error> {
        if index >= self.matches.count {
            return Err(Error::new("Match not found."));
        }
        let f = self.matches.fixture(data, index);
        if f.competition != self.table.competition || !f.played {
            return Err(Error::new("Only a league match that has been played can be changed."));
        }
        if home_goals > MAX_GOALS || away_goals > MAX_GOALS {
            return Err(Error::new(format!("Enter at most {MAX_GOALS} goals.")));
        }
        let rows = self.table.standings(data);
        let before = self.records(data, None)?;
        if rows.iter().any(|r| before[&r.club] != r.record) {
            return Err(Error::new(
                "The league table in this save doesn't add up to its results (for example after a points deduction), \
                 so it can't be changed safely.",
            ));
        }
        let order = Order::of(&rows)
            .ok_or_else(|| Error::new("The order of this league's table isn't understood, so it can't be changed safely."))?;
        let after = self.records(data, Some((index, home_goals, away_goals)))?;
        let mut new: Vec<Standing> = rows.iter().map(|r| Standing { record: after[&r.club], ..*r }).collect();
        // Stable: clubs that stay level keep their order.
        new.sort_by_key(|s| std::cmp::Reverse(order.key(&s.record)));
        let positions = order.positions(&new);
        let mut table = Vec::with_capacity(new.len() * STANDING_ROW);
        for (row, position) in new.iter().zip(positions) {
            let [a, b, c] = row.record.encode()?;
            for word in [row.club, position, a, b, c] {
                table.extend(word.to_le_bytes());
            }
        }
        let o = self.matches.offset(index);
        let score = (u32_at(data, o + SCORE) & SCORE_OTHER) | home_goals as u32 | (away_goals as u32) << 24;
        Ok(vec![(o + SCORE, score.to_le_bytes().to_vec()), (self.table.header + STANDINGS, table)])
    }
}
