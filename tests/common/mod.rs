//! Build a synthetic Master League save with the layout observed in real
//! Football Life 2026 saves.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use fl_ml_desk::contracts::{BLANK, Date};
use fl_ml_desk::crypto::{self, FILE_HEADER_SIZE, SaveFile};
use fl_ml_desk::masterleague as ml;

pub const CLUBS: usize = 450;
pub const NATIONAL: usize = 100;
pub const MANAGED: usize = 0;
pub const SQUAD: usize = 20;
pub const TABLE_AT: usize = 0x500000;
/// Row index of the table's first contract (after the header row).
pub const FIRST_ROW: usize = TABLE_AT + 48;
pub const TEAM_FLAGS: u32 = 0x0019_8000;
pub const STATUS_CONTRACTED: u32 = 5;
/// Club 5 player under negotiation (already has a table row, status 1).
pub const NEGOTIATING: (usize, usize) = (5, 3);
/// Club 6 player in the trailing "unassigned" row (index 0xFFFF).
pub const UNASSIGNED: (usize, usize) = (6, 1);
pub const GAME_DATE: (u16, u8, u8) = (2025, 8, 6);
pub const START: (u16, u8, u8) = (2025, 7, 1);

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
        p.internal_id,
        p.player_id,
        salary,
        value,
        end.pack(),
        index,
        team,
        status,
        start.map_or(0xFFFF, Date::pack),
        0xFFFF_FFFF,
        0xFFFF_FFFF,
        0xFFFF,
    ]
}

pub fn build_data(localized: bool) -> Vec<u8> {
    let mut data = vec![0u8; ml::FL26_FINANCE + 0x10000];
    let national_names = ["Albania", "Andorra", "Armenia", "Austria"];
    let team = |data: &mut Vec<u8>, index: usize, name: &str, abbr: &str, players: &[ml::Player]| {
        let base = ml::TABLE_START + index * ml::RECORD_SIZE;
        put(data, base + ml::NAME_OFFSET, name.as_bytes());
        put(data, base + ml::ABBR_OFFSET, abbr.as_bytes());
        for j in 0..ml::MAX_ROSTER {
            let p = players.get(j).copied().unwrap_or(ml::EMPTY);
            put_words(data, base + ml::ROSTER_OFFSET + 8 * j, &[p.internal_id, p.player_id]);
        }
        put(data, base + ml::COUNT_OFFSET, &(0x8000u16 | players.len() as u16).to_le_bytes());
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
        // One club player plus two players without a club ("Lainnya").
        let players = [
            club_player(n, 0),
            ml::Player { internal_id: (500_000 + n * 2) as u32, player_id: (900_000 + n * 2) as u32 },
            ml::Player { internal_id: (500_001 + n * 2) as u32, player_id: (900_001 + n * 2) as u32 },
        ];
        team(&mut data, CLUBS + n, &name, &abbr, &players);
    }
    team(&mut data, CLUBS + NATIONAL, "The Old Boys", "TOB", &[]);

    // Contract table: header row, the managed squad, a negotiation, an unassigned row, blanks.
    let start = Date::new(START.0, START.1, START.2);
    put_words(&mut data, TABLE_AT, &[0xCCCB_0C69, 476_532, 0, 0, 0, 0xCCF5_0000, team_ref(MANAGED), 0xCCF5_4305, start.unwrap().pack(), 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF]);
    let mut off = FIRST_ROW;
    for j in 0..SQUAD {
        let end = date(2026 + (j % 3) as u16, 8, 31);
        let mut w = row(club_player(MANAGED, j), 10_000 + j as u32, 500_000 + j as u32, end, j as u32 + 1, team_ref(MANAGED), STATUS_CONTRACTED, start);
        if j % 4 == 1 {
            // The game leaves stale bytes in unused high halves.
            w[5] |= 0x459C_0000;
            w[7] |= 0xA44F_4D00;
            w[11] |= 0x0005_0000;
        }
        put_words(&mut data, off, &w);
        off += 48;
    }
    let (nc, nj) = NEGOTIATING;
    put_words(&mut data, off, &row(club_player(nc, nj), 30_000, 777_000, date(2027, 8, 31), 31, team_ref(nc), 1, None));
    off += 48;
    let (uc, uj) = UNASSIGNED;
    put_words(&mut data, off, &row(club_player(uc, uj), 20_000, 600_000, date(2026, 8, 31), 0xFFFF, 0xFFFF_FFFF, 0, None));
    off += 48;
    for _ in 0..20 {
        put_words(&mut data, off, &BLANK);
        off += 48;
    }

    put_words(&mut data, ml::FL26_FINANCE, &[500_000, 0, 0, 0, 0, 200_000]); // €50M, €20M
    put(&mut data, ml::MANAGER_NAME, b"Test Manager");
    data[ml::MANAGER_NAME + ml::MANAGER_NAME_SIZE] = b'V';
    data
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
