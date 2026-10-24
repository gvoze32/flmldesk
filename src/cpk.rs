//! Read files from PES 2021 / Football Life CPK archives (CRIWARE).
//! Port of `pes21_rating.cpk` (pes21-player-rating, MIT).

use std::collections::HashMap;

use crate::Error;

const CPK_DATA_BASE: u64 = 0x800;
pub const PLAYER_BIN_MEMBER: &str = "common/etc/pesdb/Player.bin";

#[derive(Clone, Debug)]
enum Value {
    Int(u64),
    Str(String),
    Other,
}

impl Value {
    fn int(&self) -> Option<u64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }
    fn text(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }
}

type Row = HashMap<String, Value>;

fn bounds(data: &[u8], offset: usize, size: usize, label: &str) -> Result<(), Error> {
    if offset.checked_add(size).is_none_or(|end| end > data.len()) {
        return Err(Error::new(format!("{label} at 0x{offset:X} with size {size} exceeds {} bytes", data.len())));
    }
    Ok(())
}

fn be_u16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}
fn be_u32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}
fn be_u64(d: &[u8], o: usize) -> u64 {
    u64::from_be_bytes(d[o..o + 8].try_into().unwrap())
}

fn table_string(table: &[u8], strings_offset: usize, string_offset: usize) -> Result<String, Error> {
    let start = strings_offset + string_offset;
    if start >= table.len() {
        return Err(Error::new(format!("@UTF string offset {string_offset} is out of bounds")));
    }
    let end = table[start..]
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| Error::new(format!("@UTF string at offset {string_offset} is unterminated")))?;
    Ok(String::from_utf8_lossy(&table[start..start + end]).into_owned())
}

/// Read one CRI `@UTF` table and return its rows.
fn read_utf(data: &[u8], offset: usize) -> Result<Vec<Row>, Error> {
    bounds(data, offset, 24, "@UTF header")?;
    if &data[offset..offset + 4] != b"@UTF" {
        return Err(Error::new(format!("Expected @UTF at 0x{offset:X}")));
    }
    let table_size = be_u32(data, offset + 4) as usize;
    let table_start = offset + 8;
    bounds(data, table_start, table_size, "@UTF table")?;
    let table = &data[table_start..table_start + table_size];
    if table.len() < 24 {
        return Err(Error::new("@UTF table header is truncated"));
    }
    let rows_offset = be_u32(table, 0) as usize;
    let strings_offset = be_u32(table, 4) as usize;
    let data_offset = be_u32(table, 8) as usize;
    let number_columns = be_u16(table, 16) as usize;
    let row_length = be_u16(table, 18) as usize;
    let number_rows = be_u32(table, 20) as usize;
    if rows_offset > table.len() || strings_offset > table.len() || data_offset > table.len() {
        return Err(Error::new("@UTF section offset exceeds table size"));
    }

    let read_value = |at: usize, content_type: u8| -> Result<(Value, usize), Error> {
        let size = match content_type {
            0 | 1 => 1,
            2 | 3 => 2,
            4 | 5 | 8 | 10 => 4,
            6 | 7 | 9 | 11 => 8,
            _ => return Err(Error::new(format!("Unsupported @UTF content type {content_type}"))),
        };
        bounds(table, at, size, "@UTF value")?;
        let value = match content_type {
            0 | 1 => Value::Int(table[at] as u64),
            2 | 3 => Value::Int(be_u16(table, at) as u64),
            4 | 5 => Value::Int(be_u32(table, at) as u64),
            6 | 7 => Value::Int(be_u64(table, at)),
            10 => Value::Str(table_string(table, strings_offset, be_u32(table, at) as usize)?),
            11 => {
                let start = data_offset + be_u32(table, at) as usize;
                bounds(table, start, be_u32(table, at + 4) as usize, "@UTF data payload")?;
                Value::Other
            }
            _ => Value::Other,
        };
        Ok((value, at + size))
    };

    let mut columns = Vec::with_capacity(number_columns);
    let mut cursor = 24;
    for _ in 0..number_columns {
        bounds(table, cursor, 5, "@UTF column descriptor")?;
        let flags = table[cursor];
        let name = table_string(table, strings_offset, be_u32(table, cursor + 1) as usize)?;
        cursor += 5;
        let storage = flags & 0xF0;
        let content_type = flags & 0x0F;
        let mut constant = Value::Other;
        if storage == 0x30 {
            let (v, next) = read_value(cursor, content_type)?;
            constant = v;
            cursor = next;
        } else if !matches!(storage, 0x00 | 0x10 | 0x50) {
            return Err(Error::new(format!("Unsupported @UTF storage type 0x{storage:02X}")));
        }
        columns.push((name, content_type, storage, constant));
    }

    let mut rows = Vec::with_capacity(number_rows);
    for n in 0..number_rows {
        let row_offset = rows_offset + n * row_length;
        bounds(table, row_offset, row_length, "@UTF row")?;
        let mut row = Row::new();
        let mut at = row_offset;
        for (name, content_type, storage, constant) in &columns {
            let value = match storage {
                0x00 => Value::Other,
                0x10 => Value::Int(0),
                0x30 => constant.clone(),
                _ => {
                    let (v, next) = read_value(at, *content_type)?;
                    at = next;
                    v
                }
            };
            row.insert(name.clone(), value);
        }
        rows.push(row);
    }
    Ok(rows)
}

/// Compact TOC used by PES CPK mode 5 archives (protected schema, fixed row shape).
fn parse_mode5_toc_rows(data: &[u8], toc_offset: usize) -> Result<Vec<Row>, Error> {
    let utf = toc_offset + 16;
    bounds(data, utf, 24, "CPK mode 5 TOC")?;
    if &data[utf..utf + 4] != b"@UTF" {
        return Err(Error::new("CPK mode 5 TOC does not contain an @UTF table"));
    }
    let table_size = be_u32(data, utf + 4) as usize;
    bounds(data, utf + 8, table_size, "CPK mode 5 TOC table")?;
    let table = &data[utf + 8..utf + 8 + table_size];
    if table.len() < 24 {
        return Err(Error::new("CPK mode 5 TOC table header is truncated"));
    }
    let rows_offset = be_u32(table, 0) as usize;
    let strings_offset = be_u32(table, 4) as usize;
    let (number_columns, row_length) = (be_u16(table, 16), be_u16(table, 18));
    let number_rows = be_u32(table, 20) as usize;
    if (number_columns, row_length) != (8, 32) {
        return Err(Error::new("Unsupported protected CPK TOC shape"));
    }
    let mut rows = Vec::with_capacity(number_rows);
    for n in 0..number_rows {
        let r = rows_offset + n * 32;
        bounds(table, r, 32, "CPK mode 5 TOC row")?;
        let mut row = Row::new();
        row.insert("DirName".into(), Value::Str(table_string(table, strings_offset, be_u32(table, r) as usize)?));
        row.insert("FileName".into(), Value::Str(table_string(table, strings_offset, be_u32(table, r + 4) as usize)?));
        row.insert("FileSize".into(), Value::Int(be_u32(table, r + 8) as u64));
        row.insert("ExtractSize".into(), Value::Int(be_u32(table, r + 12) as u64));
        row.insert("FileOffset".into(), Value::Int(be_u64(table, r + 16)));
        rows.push(row);
    }
    Ok(rows)
}

fn full_name(row: &Row) -> String {
    let dir = row.get("DirName").map(Value::text).unwrap_or("").trim_matches('/');
    let file = row.get("FileName").map(Value::text).unwrap_or("").trim_matches('/');
    match (dir.is_empty(), file.is_empty()) {
        (false, false) => format!("{dir}/{file}"),
        (true, _) => file.to_string(),
        (false, true) => dir.to_string(),
    }
}

fn find_subslice(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.windows(needle.len()).position(|w| w == needle)
}

/// Return the bytes of `member` from a CPK image, or `None` if absent.
pub fn read_member<'a>(data: &'a [u8], member: &str) -> Result<Option<&'a [u8]>, Error> {
    let header = read_utf(data, 16)?;
    if header.len() != 1 {
        return Err(Error::new("CPK header must contain one row"));
    }
    let header = &header[0];
    let mut toc_offset = header
        .get("TocOffset")
        .and_then(Value::int)
        .ok_or_else(|| Error::new("CPK header has no valid TocOffset"))?;
    if toc_offset == u64::MAX {
        toc_offset = find_subslice(data, b"TOC ").ok_or_else(|| Error::new("TOC marker not found in CPK"))? as u64;
    }
    let toc_offset = toc_offset as usize;
    bounds(data, toc_offset, 16, "CPK TOC chunk")?;
    let rows = match read_utf(data, toc_offset + 16) {
        Ok(rows) => rows,
        Err(_) => parse_mode5_toc_rows(data, toc_offset)?,
    };
    let content_offset = header.get("ContentOffset").and_then(Value::int).unwrap_or(CPK_DATA_BASE);
    let Some(row) = rows.iter().find(|r| full_name(r) == member) else {
        return Ok(None);
    };
    let file_offset = row.get("FileOffset").and_then(Value::int).ok_or_else(|| Error::new("CPK row without offset"))?;
    let size = row
        .get("FileSize")
        .and_then(Value::int)
        .or_else(|| row.get("ExtractSize").and_then(Value::int))
        .unwrap_or(0) as usize;
    for start in [file_offset + content_offset, file_offset + CPK_DATA_BASE, file_offset + toc_offset as u64] {
        let start = start as usize;
        if start + size <= data.len() {
            return Ok(Some(&data[start..start + size]));
        }
    }
    Err(Error::new(format!("CPK payload offset 0x{file_offset:X} and size {size} exceed archive bounds")))
}
