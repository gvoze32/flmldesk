//! Build a synthetic Master League save with the layout observed in real
//! Football Life 2026 saves.
#![allow(dead_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

use flate2::Compression;
use flate2::write::ZlibEncoder;
use fl_ml_desk::contracts::{BLANK, Date};
use fl_ml_desk::crypto::{self, FILE_HEADER_SIZE, SaveFile};
use fl_ml_desk::deals;
use fl_ml_desk::masterleague as ml;
use fl_ml_desk::playerdb as pdb;

pub const CLUBS: usize = 450;
pub const NATIONAL: usize = 100;
pub const MANAGED: usize = 0;
pub const SQUAD: usize = 20;
pub const TABLE_AT: usize = 0x500000;
/// Offset of the table's first contract (after a non-contract row).
pub const FIRST_ROW: usize = TABLE_AT + 48;
pub const TEAM_FLAGS: u32 = 0x0019_8000;
pub const STATUS_CONTRACTED: u32 = 5;
/// Club 5 player you are negotiating for (row with your club, status 1).
pub const NEGOTIATING: (usize, usize) = (5, 3);
/// Row index of the negotiation; the squad uses 0..SQUAD.
pub const NEGOTIATION_INDEX: u32 = SQUAD as u32 + 1;
pub const GAME_DATE: (u16, u8, u8) = (2025, 8, 6);
pub const START: (u16, u8, u8) = (2025, 7, 1);
/// Player records container (its magic), after every fixed-offset table.
pub const PLAYERS_AT: usize = 0xCA_0000;
/// Clubs whose players have records (internal ids below 0x10000).
pub const RECORDED_CLUBS: usize = 64;
/// Bytes after the container, which must survive a resize.
pub const TAIL: [u8; 8] = *b"TAILDATA";
/// Competition registrations of every club player.
pub const COMPETITIONS: [u32; 4] = [0x0340_0011, 0x0280_0017, 0xFFFF, 0xFFFF];

/// Shirt number of squad slot `j`: clubs with an even index use 1..=20, odd ones 21..=40.
pub fn shirt(club: usize, j: usize) -> u16 {
    (j + 1 + (club % 2) * 20) as u16
}

/// Managed squad's contract salary (units) and end, mirrored by the records.
pub fn managed_terms(j: usize) -> (u32, Date) {
    (10_000 + j as u32, date(2026 + (j % 3) as u16, 8, 31))
}

/// Offset of the player's record in `MasterLeague::records`.
pub fn record_at(p: ml::Player) -> usize {
    (p.internal_id & 0xFFFF) as usize * pdb::RECORD
}

pub fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// Records as the game keeps them: clubs' players registered for two
/// competitions, the "Others" as free agents.
pub fn player_records() -> Vec<u8> {
    let mut r = vec![0u8; 0x10000 * pdb::RECORD];
    let mut fill = |p: ml::Player, club: u32, salary: u32, end: Date, competitions: [u32; 4]| {
        let o = record_at(p);
        put(&mut r, o + pdb::R_INTERNAL, &(p.internal_id as u16).to_le_bytes());
        put_words(&mut r, o + pdb::R_PLAYER, &[p.player_id]);
        put_words(&mut r, o + pdb::R_COMPETITIONS, &competitions);
        put_words(&mut r, o + pdb::R_SALARY, &[salary]);
        put_words(&mut r, o + pdb::R_FROM_CLUB, &[pdb::NO_CLUB, club, end.pack(), date(START.0, START.1, START.2).pack(), pdb::NO_LOAN]);
        put(&mut r, o + 152, &[0x64, 0x01, 0x07, 0x00]);
    };
    for c in 0..RECORDED_CLUBS {
        for j in 0..SQUAD {
            let (salary, end) = if c == MANAGED { managed_terms(j) } else { (5_000, date(2027, 6, 30)) };
            fill(club_player(c, j), team_ref(c), salary, end, COMPETITIONS);
        }
    }
    for n in 0..NATIONAL {
        for k in 0..2 {
            let p = ml::Player { internal_id: (500_000 + n * 2 + k) as u32, player_id: (900_000 + n * 2 + k) as u32 };
            fill(p, pdb::NO_CLUB, 2_000, date(2027, 1, 31), [0xFFFF; 4]);
        }
    }
    r
}

/// Container in the game's format: 256 KiB zlib chunks behind a big-endian header.
fn player_container(records: &[u8]) -> Vec<u8> {
    let chunks: Vec<(usize, Vec<u8>)> = records
        .chunks(0x40000)
        .map(|c| {
            let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
            z.write_all(c).unwrap();
            (c.len(), z.finish().unwrap())
        })
        .collect();
    let mut body: Vec<u8> = [0x2011_0912u32, 0, records.len() as u32, chunks.iter().map(|c| c.1.len() as u32).sum()]
        .iter()
        .flat_map(|v| v.to_be_bytes())
        .collect();
    for (i, (size, stream)) in chunks.iter().enumerate() {
        let next = if i + 1 == chunks.len() { 0 } else { body.len() + 12 + stream.len() };
        for v in [*size, stream.len(), next] {
            body.extend((v as u32).to_be_bytes());
        }
        body.extend(stream);
    }
    body
}

pub fn club_player(club: usize, j: usize) -> ml::Player {
    ml::Player { internal_id: (1000 * club + j + 1) as u32, player_id: (100_000 + club * 100 + j) as u32 }
}

pub fn team_ref(club: usize) -> u32 {
    TEAM_FLAGS | club as u32
}

pub fn date(y: u16, m: u8, d: u8) -> Date {
    Date::new(y, m, d).unwrap()
}

fn put(data: &mut [u8], off: usize, bytes: &[u8]) {
    data[off..off + bytes.len()].copy_from_slice(bytes);
}

pub fn put_words(data: &mut [u8], off: usize, words: &[u32]) {
    for (i, w) in words.iter().enumerate() {
        put(data, off + 4 * i, &w.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
pub fn row(p: ml::Player, salary: u32, value: u32, end: Date, index: u32, team: u32, status: u32, start: Option<Date>) -> [u32; 12] {
    [
        index,
        team,
        status,
        start.map_or(0xFFFF, Date::pack),
        0xFFFF_FFFF,
        0xFFFF_FFFF,
        0xFFFF,
        p.internal_id,
        p.player_id,
        salary,
        value,
        end.pack(),
    ]
}

pub fn build_data(localized: bool) -> Vec<u8> {
    build_data_with_records(localized, &player_records())
}

pub fn build_data_with_records(localized: bool, records: &[u8]) -> Vec<u8> {
    let mut data = vec![0u8; ml::FL26_FINANCE + 0x10000];
    let national_names = ["Albania", "Andorra", "Armenia", "Austria"];
    let team = |data: &mut Vec<u8>, index: usize, name: &str, abbr: &str, players: &[ml::Player]| {
        let base = ml::TABLE_START + index * ml::RECORD_SIZE;
        put_words(data, base, &[team_ref(index)]);
        put(data, base + ml::NAME_OFFSET, name.as_bytes());
        put(data, base + ml::ABBR_OFFSET, abbr.as_bytes());
        for j in 0..ml::MAX_ROSTER {
            let p = players.get(j).copied().unwrap_or(ml::EMPTY);
            put_words(data, base + ml::ROSTER_OFFSET + 8 * j, &[p.internal_id, p.player_id]);
        }
        put(data, base + ml::COUNT_OFFSET, &(0x8000u16 | players.len() as u16).to_le_bytes());
        let numbers: Vec<u8> = (0..players.len()).flat_map(|j| shirt(index, j).to_le_bytes()).collect();
        put(data, base + ml::NUMBERS_OFFSET, &numbers);
    };
    for c in 0..CLUBS {
        let players: Vec<_> = (0..SQUAD).map(|j| club_player(c, j)).collect();
        team(&mut data, c, &format!("Club {c}"), &format!("C{c:03}"), &players);
    }
    for n in 0..NATIONAL {
        let (name, abbr) = if n < 4 {
            let name = if localized { format!("国家{n}") } else { national_names[n].to_string() };
            (name, ml::NATIONAL_FIRST[n].to_string())
        } else {
            (format!("Nation {n}"), format!("N{n:02}"))
        };
        // One club player plus two players without a club ("Others").
        let players = [
            club_player(n, 0),
            ml::Player { internal_id: (500_000 + n * 2) as u32, player_id: (900_000 + n * 2) as u32 },
            ml::Player { internal_id: (500_001 + n * 2) as u32, player_id: (900_001 + n * 2) as u32 },
        ];
        team(&mut data, CLUBS + n, &name, &abbr, &players);
    }
    team(&mut data, CLUBS + NATIONAL, "The Old Boys", "TOB", &[]);

    // Contract table: a non-contract row, the managed squad, a negotiation, blanks.
    let start = Date::new(START.0, START.1, START.2);
    put_words(&mut data, TABLE_AT, &[0x0D81_B036, 0x1B03_606C, 0x3606_C0D8, 0x6C0D_81B0, 0xD81B_0360, 0xB036_06C0, 0x7FFB_6AD1, 0xCCCB_0C69, 476_532, 0, 0, 0]);
    let mut off = FIRST_ROW;
    for j in 0..SQUAD {
        let end = date(2026 + (j % 3) as u16, 8, 31);
        let mut w = row(club_player(MANAGED, j), 10_000 + j as u32, 500_000 + j as u32, end, j as u32, team_ref(MANAGED), STATUS_CONTRACTED, start);
        if j % 4 == 1 {
            // The game leaves stale bytes in unused high halves.
            w[0] |= 0x459C_0000;
            w[2] |= 0xA44F_4D00;
            w[6] |= 0x0005_0000;
        }
        put_words(&mut data, off, &w);
        off += 48;
    }
    let (nc, nj) = NEGOTIATING;
    put_words(&mut data, off, &row(club_player(nc, nj), 30_000, 777_000, date(2027, 8, 31), NEGOTIATION_INDEX, team_ref(MANAGED), 1, None));
    off += 48;
    for _ in 0..20 {
        put_words(&mut data, off, &BLANK);
        off += 48;
    }

    // Transfer lists behind the contract table: the negotiation's offer
    // (id = its index - 1, as the game numbers them), then free slots.
    let offers = FIRST_ROW + deals::OFFERS_OFFSET;
    let p = club_player(nc, nj);
    put_words(&mut data, offers, &negotiation_offer(p, nc));
    for k in 1..8 {
        put_words(&mut data, offers + k * deals::OFFER_RECORD, &deals::BLANK_OFFER);
    }
    for k in 0..8 {
        put(&mut data, FIRST_ROW + deals::AGREED_OFFSET + k * deals::AGREED_ENTRY, &[0xFF; 6]);
        put_words(&mut data, deals::PENDING_ARRIVALS + k * deals::PENDING_ENTRY, &[0xFFFF, 0, 0]);
        put_words(&mut data, deals::SEASON_DEALS + k * deals::DEAL_ENTRY, &[0xFFFF, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFFFF]);
    }
    // Negotiation timeline: slot 0 (blocks 0..3) is an AI club's negotiation, then free slots.
    let blank_round = [0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0];
    let timeline = FIRST_ROW + deals::TIMELINE_OFFSET;
    for k in 0..48 {
        let block = timeline + k * deals::TIMELINE_BLOCK;
        let owner: u16 = if k < 2 { 0x0100 } else { 0xFF00 };
        put(&mut data, block, &[owner.to_le_bytes(), [0xFF, 0xFF]].concat());
        for r in 0..8 {
            put(&mut data, block + 4 + r * deals::TIMELINE_ROUND, &blank_round);
        }
    }
    for (k, number) in [(0usize, 39u16), (1, 40)] {
        let round: Vec<u8> = [number, 1, 2025, 1 << 8 | 8, 2025, 4 << 8 | 8].iter().flat_map(|v| v.to_le_bytes()).collect();
        put(&mut data, timeline + k * deals::TIMELINE_BLOCK + 4, &round);
    }
    // One news item already in the list, then free rows.
    let mut item = deals::BLANK_NEWS;
    (item[0], item[13]) = (0x1B, date(2025, 8, 1).pack());
    put_words(&mut data, deals::NEWS, &item);
    for k in 1..8 {
        put_words(&mut data, deals::NEWS + k * deals::NEWS_ROW, &deals::BLANK_NEWS);
    }

    put_words(&mut data, ml::FL26_FINANCE, &[500_000, 0, 0, 0, 0, 200_000]); // €50M, €20M
    put(&mut data, ml::MANAGER_NAME, b"Test Manager");
    data[ml::MANAGER_NAME + ml::MANAGER_NAME_SIZE] = b'V';

    // "My Team": the managed squad in its own order (reversed), then free rows.
    put_words(&mut data, ml::MY_TEAM_CLUB, &[team_ref(MANAGED)]);
    for k in 0..64 {
        let row = match SQUAD.checked_sub(k + 1) {
            Some(j) => [club_player(MANAGED, j).internal_id, club_player(MANAGED, j).player_id, 0, 0xC0],
            None => [0xFFFF, 0, 0, 0xC7],
        };
        put_words(&mut data, ml::MY_TEAM + k * ml::MY_TEAM_ROW, &row);
    }

    // Player records, then other data the container must not disturb.
    let container = player_container(records);
    data.resize(PLAYERS_AT, 0);
    put_words(&mut data, PLAYERS_AT - 16, &[container.len() as u32, TAIL.len() as u32, 0, 0]);
    data.extend(container);
    data.extend(TAIL);
    data
}

pub fn negotiation_offer(p: ml::Player, seller: usize) -> [u32; 15] {
    let fee = 7_770;
    [NEGOTIATION_INDEX - 1, team_ref(MANAGED), 1, 0xFFFF, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, team_ref(seller), p.internal_id, p.player_id, fee, fee, 0xFFFF, 0xFFFF, 0]
}

pub fn write_save_as(path: &Path, data: Vec<u8>, version: &str, description: &str) -> PathBuf {
    let mut fh = vec![0u8; FILE_HEADER_SIZE];
    let mut desc = vec![0u8; 384];
    put(&mut desc, 0, b"Master League 01");
    put(&mut desc, 0x80, description.as_bytes());
    let logo = vec![0x22u8; 16];
    let serial: Vec<u8> = "S-1-5-21".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    for (i, v) in [data.len(), logo.len(), desc.len(), serial.len() / 2].iter().enumerate() {
        put(&mut fh, 64 + 4 * i, &(*v as u32).to_le_bytes());
    }
    put(&mut fh, 144, b"ML");
    put(&mut fh, 176, version.as_bytes());
    let mut enc = vec![0u8; 320];
    for (i, b) in enc[256..].iter_mut().enumerate() {
        *b = (i * 37 + 11) as u8;
    }
    let save = SaveFile { enc_header: enc, file_header: fh, desc, logo, data, serial };
    std::fs::write(path, crypto::pack(&save)).unwrap();
    path.to_path_buf()
}

pub fn write_save(path: &Path, data: Vec<u8>) -> PathBuf {
    let (y, m, d) = GAME_DATE;
    write_save_as(path, data, "SP Football Life 2026", &format!("Club 0 / League\n{d}/{m}/{y}\nLeague"))
}

/// Unique temporary directory removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("fl-ml-desk-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
