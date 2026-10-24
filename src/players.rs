//! Player names and details for display.
//!
//! Master League saves only store player IDs. Names, positions, ages and
//! overall ratings come from the game database (`Player.bin` inside the CPK
//! archives), the option file `EDIT00000000` (edited players win) or a plain
//! ID/name list.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

use flate2::read::ZlibDecoder;
use regex::Regex;

use crate::rating::{POSITION_NAMES, decode_edit_player, decode_playerbin_record};
use crate::{Error, crypto, cpk};

const PLAYERBIN_RECORD_SIZE: usize = 312;
const PLAYERBIN_ID_OFFSET: usize = 0x08;
const PLAYERBIN_NAME_OFFSET: usize = 0x44;
const PLAYERBIN_AGE_OFFSET: usize = 0x33; // low 6 bits, +15
const EDIT_HEADER_SIZE: usize = 0x7C;
const EDIT_PLAYER_COUNT_OFFSET: usize = 0x60;
const EDIT_MAX_PLAYERS: usize = 30_000;
const EDIT_SLOT_SIZE: usize = 312;
const EDIT_NAME_OFFSET: usize = 0x36;
const EDIT_AGE_FIELD: usize = 0x20; // u16, bits 7..12, real age
const EDIT_DATA_SIZE: usize = 10_995_800;
const NAME_SIZE: usize = 61;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerInfo {
    pub player_id: u32,
    pub name: String,
    pub position: Option<&'static str>,
    pub age: Option<u32>,
    pub overall: Option<i64>,
}

#[derive(Default)]
pub struct PlayerCatalog {
    pub players: HashMap<u32, PlayerInfo>,
    pub sources: Vec<String>,
}

fn text(data: &[u8], offset: usize) -> String {
    let raw = &data[offset..offset + NAME_SIZE];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

fn u16_at(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
fn u32_at(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

/// Payload of a WESYS/zlib container, or the input unchanged.
fn decompress_wesys(raw: &[u8]) -> Result<Vec<u8>, Error> {
    if raw.len() < 8 || &raw[3..8] != b"WESYS" {
        return Ok(raw.to_vec());
    }
    if raw.len() < 16 {
        return Err(Error::new("WESYS header is truncated"));
    }
    let compressed = u32_at(raw, 8) as usize;
    let expected = u32_at(raw, 12) as usize;
    if compressed == 0 || 16 + compressed > raw.len() {
        return Err(Error::new("WESYS compressed payload is truncated"));
    }
    let mut out = Vec::with_capacity(expected);
    ZlibDecoder::new(&raw[16..16 + compressed])
        .read_to_end(&mut out)
        .map_err(|e| Error::new(format!("WESYS zlib: {e}")))?;
    if expected != 0 && out.len() != expected {
        return Err(Error::new(format!("WESYS payload is {} bytes; expected {expected}", out.len())));
    }
    Ok(out)
}

impl PlayerCatalog {
    pub fn get(&self, player_id: u32) -> PlayerInfo {
        self.players.get(&player_id).cloned().unwrap_or_else(|| PlayerInfo {
            player_id,
            name: format!("ID {player_id}"),
            position: None,
            age: None,
            overall: None,
        })
    }

    /// Later sources win field by field; missing fields keep earlier values.
    fn add(&mut self, info: PlayerInfo) {
        match self.players.get_mut(&info.player_id) {
            None => {
                self.players.insert(info.player_id, info);
            }
            Some(old) => {
                if !info.name.is_empty() {
                    old.name = info.name;
                }
                old.position = info.position.or(old.position);
                old.age = info.age.or(old.age);
                old.overall = info.overall.or(old.overall);
            }
        }
    }

    /// Merge another catalog on top of this one (other wins).
    pub fn merge(&mut self, other: PlayerCatalog) {
        let mut entries: Vec<_> = other.players.into_values().collect();
        entries.sort_by_key(|p| p.player_id);
        for info in entries {
            self.add(info);
        }
        self.sources.extend(other.sources);
    }

    pub fn load_playerbin(&mut self, raw: &[u8], source: &str) -> Result<usize, Error> {
        let data = decompress_wesys(raw)?;
        if data.len() % PLAYERBIN_RECORD_SIZE != 0 {
            return Err(Error::new(format!("{source}: ukuran Player.bin tidak valid")));
        }
        let mut count = 0;
        for base in (0..data.len()).step_by(PLAYERBIN_RECORD_SIZE) {
            let pid = u32_at(&data, base + PLAYERBIN_ID_OFFSET);
            if pid == 0 {
                continue;
            }
            let name = text(&data, base + PLAYERBIN_NAME_OFFSET);
            let abilities = decode_playerbin_record(&data, base);
            self.add(PlayerInfo {
                player_id: pid,
                name: if name.is_empty() { format!("ID {pid}") } else { name },
                position: abilities.as_ref().map(|a| POSITION_NAMES[a.registered_position]),
                age: Some((data[base + PLAYERBIN_AGE_OFFSET] & 0x3F) as u32 + 15),
                overall: abilities.map(|a| a.overall()),
            });
            count += 1;
        }
        self.sources.push(source.to_string());
        Ok(count)
    }

    /// Load Player.bin from a CPK; returns 0 when the archive has none.
    pub fn load_cpk(&mut self, path: &Path) -> Result<usize, Error> {
        let file = File::open(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        // SAFETY: the archive is opened read-only and only read while mapped.
        let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let Some(raw) = cpk::read_member(&map, cpk::PLAYER_BIN_MEMBER)? else {
            return Ok(0);
        };
        let raw = raw.to_vec();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.load_playerbin(&raw, &name)
    }

    /// Load every Player.bin in `<folder>/download/*.cpk` (or the folder itself).
    /// Archives whose name contains `extra` load last, as in Football Life.
    pub fn load_game_folder(&mut self, folder: &Path) -> Result<usize, Error> {
        let download = folder.join("download");
        let dir = if download.is_dir() { download } else { folder.to_path_buf() };
        let mut archives: Vec<_> = std::fs::read_dir(&dir)
            .map_err(|e| Error::new(format!("{}: {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cpk")))
            .collect();
        archives.sort_by_key(|p| {
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            (name.contains("extra"), name)
        });
        let mut total = 0;
        for archive in archives {
            total += self.load_cpk(&archive).unwrap_or(0);
        }
        if total == 0 {
            return Err(Error::new(format!("Tidak ada Player.bin di CPK folder {}.", dir.display())));
        }
        Ok(total)
    }

    pub fn load_edit(&mut self, path: &Path) -> Result<usize, Error> {
        let blob = std::fs::read(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let save = crypto::unpack(&blob, false)?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if save.file_type() != "EDIT" {
            return Err(Error::new(format!("{name} bukan file EDIT (tipe {}).", save.file_type())));
        }
        let data = &save.data;
        let used = if data.len() == EDIT_DATA_SIZE { u16_at(data, EDIT_PLAYER_COUNT_OFFSET) as usize } else { usize::MAX };
        if used > EDIT_MAX_PLAYERS {
            return Err(Error::new(format!("{name}: struktur data EDIT tidak dikenali.")));
        }
        let mut count = 0;
        for index in 0..used {
            let base = EDIT_HEADER_SIZE + index * EDIT_SLOT_SIZE;
            let pid = u32_at(data, base);
            if pid == 0 {
                continue;
            }
            let player_name = text(data, base + EDIT_NAME_OFFSET);
            let age = ((u16_at(data, base + EDIT_AGE_FIELD) >> 7) & 0x3F) as u32;
            let abilities = decode_edit_player(data, base);
            self.add(PlayerInfo {
                player_id: pid,
                name: if player_name.is_empty() { format!("ID {pid}") } else { player_name },
                position: abilities.as_ref().map(|a| POSITION_NAMES[a.registered_position]),
                age: (age != 0).then_some(age),
                overall: abilities.map(|a| a.overall()),
            });
            count += 1;
        }
        self.sources.push(name);
        Ok(count)
    }

    /// Read `id,name` / `name,id` CSV or `id - name` text lists.
    pub fn load_name_list_text(&mut self, raw: &str, source: &str) -> usize {
        static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d{1,7}$").unwrap());
        static ID_FIRST: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"^(\d{1,7})\s*[-|;:\t ]\s*(.{2,95})$").unwrap());
        static ID_LAST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.{2,95}?)\s*[|;\t]\s*(\d{1,7})$").unwrap());
        let raw = raw.strip_prefix('\u{feff}').unwrap_or(raw);
        let mut found: Vec<(u32, String)> = Vec::new();
        let lines: Vec<&str> = raw.lines().collect();
        for line in &lines {
            let row = split_csv(line);
            if row.is_empty() || row.len() > 8 {
                continue;
            }
            let items: Vec<String> = row.iter().map(|c| c.trim().trim_matches('"').to_string()).collect();
            let ids: Vec<usize> = (0..items.len()).filter(|&i| ID.is_match(&items[i])).collect();
            if ids.len() == 1 && items.len() >= 2 {
                let name = items
                    .iter()
                    .enumerate()
                    .filter(|(i, x)| *i != ids[0] && !x.is_empty())
                    .map(|(_, x)| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                let len = name.chars().count();
                if len > 0 && len < 95 {
                    found.push((items[ids[0]].parse().unwrap(), name));
                }
            }
        }
        for line in &lines {
            let line = line.trim().trim_matches('"');
            if let Some(m) = ID_FIRST.captures(line) {
                let name = m[2].trim();
                if !name.chars().all(char::is_numeric) {
                    found.push((m[1].parse().unwrap(), name.trim_matches('"').to_string()));
                    continue;
                }
            }
            if let Some(m) = ID_LAST.captures(line) {
                found.push((m[2].parse().unwrap(), m[1].trim().trim_matches('"').to_string()));
            }
        }
        let mut unique: HashMap<u32, String> = HashMap::new();
        for (pid, name) in found {
            unique.insert(pid, name);
        }
        let count = unique.len();
        let mut entries: Vec<_> = unique.into_iter().collect();
        entries.sort();
        for (pid, name) in entries {
            self.add(PlayerInfo { player_id: pid, name, position: None, age: None, overall: None });
        }
        if count > 0 {
            self.sources.push(source.to_string());
        }
        count
    }

    pub fn load_name_list(&mut self, path: &Path) -> Result<usize, Error> {
        let bytes = std::fs::read(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(self.load_name_list_text(&String::from_utf8_lossy(&bytes), &name))
    }
}

/// Minimal CSV field splitter (comma, `"` quoting, `""` escape) like Python's csv module.
fn split_csv(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    let mut at_start = true;
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && at_start {
            quoted = true;
            at_start = false;
        } else if c == ',' {
            fields.push(std::mem::take(&mut field));
            at_start = true;
        } else {
            field.push(c);
            at_start = false;
        }
    }
    if !line.is_empty() {
        fields.push(field);
    }
    fields
}
