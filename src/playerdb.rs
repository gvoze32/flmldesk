//! Player records: the zlib container inside the data block, one 156-byte
//! record per player, record index = low 16 bits of the internal id.
//!
//! Container (decoded from Football Life 2026 saves; it sits after every
//! fixed-offset table the editor writes and before the contract table, so
//! its size can change without moving them):
//!
//! | Offset | Content |
//! |---|---|
//! | start − 16 | u32 LE container length (start … end of the last chunk) |
//! | start | BE `0x20110912`, BE 0, BE uncompressed size, BE sum of compressed sizes |
//! | chunk | BE uncompressed size (256 KiB, the last one less), BE compressed size, BE offset of the next chunk from start (0 = last), zlib stream (level 9) |
//!
//! Record fields the game keeps in step with transfers and contracts
//! (checked against 50 transfers done by the game, a signing arriving at the
//! managed club, every contract of the managed club, and ~1,600 free agents):
//!
//! | Offset | Content |
//! |---|---|
//! | +32 | u16 internal id |
//! | +36 | u32 player id |
//! | +52 | 4 × (u16 competition, u16 flags): competitions he is registered for, `(0xFFFF, 0)` = free |
//! | +116 | u32 annual salary (× €100), equal to his contract row |
//! | +124 | club he joined from this season (`0xFFFFFFFF` = none) |
//! | +128 | club at the start of the season (`0xFFFFFFFF` = free agent) |
//! | +132 | contract end, equal to his contract row |
//! | +136 | joined / contract start date |
//! | +140 | loan end (`0xFFFF` = not on loan) |
//! | +153 | flags: `0x0C` moved this season, `0x80` on loan |
//! | +154 | 4 after a move, 0 for a free agent |
//!
//! On a move the game sets +124 to the old club, +136 to the move date, sets
//! `0x0C` in +153 and +154 to 4, keeps +128 and re-registers competitions
//! (sometimes none). The editor clears the registrations: a state the game
//! itself leaves, unlike registrations for the old club's competitions.

use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

use crate::Error;
use crate::contracts::Date;
use crate::masterleague::Player;

pub const RECORD: usize = 156;
const MAGIC: [u8; 4] = [0x20, 0x11, 0x09, 0x12];
/// The container must come after the editor's fixed-offset tables (the
/// highest, pending arrivals, ends below this), so resizing it moves none of them.
pub const MIN_START: usize = 0xC9_0000;
const MAX_CHUNKS: usize = 1024;

pub const R_INTERNAL: usize = 32;
pub const R_PLAYER: usize = 36;
pub const R_COMPETITIONS: usize = 52;
pub const COMPETITION_SLOTS: usize = 4;
pub const R_SALARY: usize = 116;
pub const R_FROM_CLUB: usize = 124;
pub const R_SEASON_CLUB: usize = 128;
pub const R_CONTRACT_END: usize = 132;
pub const R_JOINED: usize = 136;
pub const R_LOAN_END: usize = 140;
pub const R_FLAGS: usize = 153;
pub const R_STATE: usize = 154;
pub const FLAG_MOVED: u8 = 0x0C;
pub const FLAG_LOAN: u8 = 0x80;
pub const STATE_MOVED: u8 = 4;
pub const STATE_FREE_AGENT: u8 = 0;
pub const NO_CLUB: u32 = 0xFFFF_FFFF;
pub const NO_LOAN: u32 = 0xFFFF;

fn be32(d: &[u8], o: usize) -> Option<usize> {
    Some(u32::from_be_bytes(d.get(o..o.checked_add(4)?)?.try_into().unwrap()) as usize)
}

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

fn put32(r: &mut [u8], o: usize, v: u32) {
    r[o..o + 4].copy_from_slice(&v.to_le_bytes());
}

#[derive(Clone, Debug)]
struct Chunk {
    /// Data-block offset of the zlib stream.
    stream: usize,
    compressed: usize,
    size: usize,
}

/// The decompressed records and where their container lies in the data block.
#[derive(Clone, Debug)]
pub struct PlayerDb {
    start: usize,
    end: usize,
    chunks: Vec<Chunk>,
    pub records: Vec<u8>,
}

/// What the game writes into a player's record when he changes club.
#[derive(Clone, Copy, Debug)]
pub struct Move {
    /// Reference of the club he leaves; `None` for a free agent.
    pub from: Option<u32>,
    pub joined: Date,
    /// New (annual salary units, contract end), when he signs a new contract.
    pub contract: Option<(u32, Date)>,
}

impl PlayerDb {
    pub fn locate(data: &[u8]) -> Result<PlayerDb, Error> {
        let mut from = MIN_START;
        while let Some(pos) = data.get(from..).and_then(|d| memchr::memmem::find(d, &MAGIC)) {
            if let Some(db) = Self::parse(data, from + pos) {
                return Ok(db);
            }
            from += pos + 1;
        }
        Err(Error::new("This save's player records weren't found."))
    }

    fn parse(data: &[u8], start: usize) -> Option<PlayerDb> {
        let end = start.checked_add(le32(data, start.checked_sub(16)?) as usize)?;
        if end > data.len() || be32(data, start + 4)? != 0 {
            return None;
        }
        let (total, compressed_sum) = (be32(data, start + 8)?, be32(data, start + 12)?);
        let mut records = Vec::with_capacity(total.min(data.len() * 8));
        let mut chunks = Vec::new();
        let mut header = start + 16;
        loop {
            let (size, compressed, next) = (be32(data, header)?, be32(data, header + 4)?, be32(data, header + 8)?);
            let stream = header + 12;
            let raw = data.get(stream..stream.checked_add(compressed)?)?;
            let mut out = Vec::with_capacity(size.min(1 << 20));
            ZlibDecoder::new(raw).read_to_end(&mut out).ok()?;
            if out.len() != size || chunks.len() >= MAX_CHUNKS {
                return None;
            }
            records.extend(out);
            chunks.push(Chunk { stream, compressed, size });
            if next == 0 {
                if stream + compressed != end {
                    return None;
                }
                break;
            }
            if start.checked_add(next)? != stream + compressed {
                return None;
            }
            header = start + next;
        }
        let ok = records.len() == total
            && chunks.iter().map(|c| c.compressed).sum::<usize>() == compressed_sum
            && total % RECORD == 0;
        ok.then_some(PlayerDb { start, end, chunks, records })
    }

    /// Offset of the player's record in `records`, after checking it is his.
    pub fn record(&self, records: &[u8], p: &Player) -> Result<usize, Error> {
        let off = (p.internal_id & 0xFFFF) as usize * RECORD;
        let r = records.get(off..off + RECORD).ok_or_else(|| missing(p))?;
        let internal = u16::from_le_bytes([r[R_INTERNAL], r[R_INTERNAL + 1]]) as u32;
        if internal != p.internal_id & 0xFFFF || le32(r, R_PLAYER) != p.player_id {
            return Err(missing(p));
        }
        Ok(off)
    }

    /// `data` with this container rebuilt from `records`: changed chunks are
    /// recompressed, the others copied; the container length and sizes are
    /// updated. Everything after the container moves by the size difference.
    pub fn splice(&self, data: &[u8], records: &[u8]) -> Result<Vec<u8>, Error> {
        if records.len() != self.records.len() {
            return Err(Error::new("The player records changed size."));
        }
        let mut streams: Vec<Vec<u8>> = Vec::with_capacity(self.chunks.len());
        let mut pos = 0;
        for c in &self.chunks {
            let range = pos..pos + c.size;
            pos += c.size;
            if records[range.clone()] == self.records[range.clone()] {
                streams.push(data[c.stream..c.stream + c.compressed].to_vec());
                continue;
            }
            let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
            let stream = z.write_all(&records[range]).and_then(|_| z.finish());
            streams.push(stream.map_err(|e| Error::new(format!("Couldn't compress the player records: {e}")))?);
        }
        let mut body = Vec::new();
        body.extend(MAGIC);
        body.extend(0u32.to_be_bytes());
        body.extend((records.len() as u32).to_be_bytes());
        body.extend((streams.iter().map(Vec::len).sum::<usize>() as u32).to_be_bytes());
        for (i, (c, s)) in self.chunks.iter().zip(&streams).enumerate() {
            let next = if i + 1 == streams.len() { 0 } else { body.len() + 12 + s.len() };
            body.extend((c.size as u32).to_be_bytes());
            body.extend((s.len() as u32).to_be_bytes());
            body.extend((next as u32).to_be_bytes());
            body.extend(s);
        }
        let mut out = Vec::with_capacity(data.len() + body.len());
        out.extend(&data[..self.start - 16]);
        out.extend((body.len() as u32).to_le_bytes());
        out.extend(&data[self.start - 12..self.start]);
        out.extend(&body);
        out.extend(&data[self.end..]);
        Ok(out)
    }

    /// True when `spliced` holds `records` in a container at the same place
    /// and every byte outside the container equals `data`.
    pub fn check_splice(&self, data: &[u8], spliced: &[u8], records: &[u8]) -> bool {
        let Some(new) = Self::parse(spliced, self.start) else { return false };
        new.records == records
            && spliced[..self.start - 16] == data[..self.start - 16]
            && spliced[self.start - 12..self.start] == data[self.start - 12..self.start]
            && spliced[new.end..] == data[self.end..]
    }
}

fn missing(p: &Player) -> Error {
    Error::new(format!("Player ID {}'s record in the save couldn't be found; cancelled.", p.player_id))
}

fn on_loan(r: &[u8]) -> bool {
    r[R_FLAGS] & FLAG_LOAN != 0 || le32(r, R_LOAN_END) != NO_LOAN
}

fn clear_competitions(r: &mut [u8]) {
    for k in 0..COMPETITION_SLOTS {
        put32(r, R_COMPETITIONS + 4 * k, 0xFFFF);
    }
}

/// The record after a move to another club. A player on loan is refused:
/// his loan belongs to the game's own lists.
pub fn moved(record: &[u8], p: &Player, m: &Move) -> Result<Vec<u8>, Error> {
    if on_loan(record) {
        return Err(Error::new(format!("Player ID {} is on loan; the game ends the loan, not the editor.", p.player_id)));
    }
    let mut r = record.to_vec();
    put32(&mut r, R_FROM_CLUB, m.from.unwrap_or(NO_CLUB));
    put32(&mut r, R_JOINED, m.joined.pack());
    if let Some((salary, end)) = m.contract {
        put32(&mut r, R_SALARY, salary);
        put32(&mut r, R_CONTRACT_END, end.pack());
    }
    clear_competitions(&mut r);
    r[R_FLAGS] |= FLAG_MOVED;
    r[R_STATE] = STATE_MOVED;
    Ok(r)
}

/// The record of a released player, shaped like the game's free agents; his
/// last salary and contract dates stay.
pub fn released(record: &[u8], p: &Player) -> Result<Vec<u8>, Error> {
    if on_loan(record) {
        return Err(Error::new(format!("Player ID {} is on loan; the game ends the loan, not the editor.", p.player_id)));
    }
    let mut r = record.to_vec();
    put32(&mut r, R_FROM_CLUB, NO_CLUB);
    put32(&mut r, R_SEASON_CLUB, NO_CLUB);
    clear_competitions(&mut r);
    r[R_FLAGS] &= !FLAG_MOVED;
    r[R_STATE] = STATE_FREE_AGENT;
    Ok(r)
}

/// The record with the renewed salary and contract end, which it mirrors
/// from the contract row.
pub fn renewed(record: &[u8], salary: u32, end: Date) -> Vec<u8> {
    let mut r = record.to_vec();
    put32(&mut r, R_SALARY, salary);
    put32(&mut r, R_CONTRACT_END, end.pack());
    r
}
