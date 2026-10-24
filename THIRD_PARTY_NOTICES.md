# Third-party notices

- **pesXdecrypter** (the4chancup): https://github.com/the4chancup/pesXdecrypter.
  It documents the PES 2021 save format, master key and cipher, which
  `src/crypto.rs` reimplements in Rust. No source file from that project is
  included.
- **MT19937** (Makoto Matsumoto and Takuji Nishimura) is implemented in
  `src/crypto.rs`. Its BSD license is in `LICENSES/MT19937-BSD-3-Clause.txt`.
- **FL26 Transfer Editor v0.5.1** (MIT, `LICENSES/FL26-Transfer-Editor-MIT.txt`).
  This project reuses its club table layout, its roster and "Others" logic, and
  its idea of finding the contract table through the squad.
- **pes21-player-rating** (MIT). `src/cpk.rs` and `src/rating.rs` are Rust
  ports of its CPK reader and its ability/OVR decoder.
- **PES 2021 ML Money Tool** by Devil Cold52 is the source of the PES 2021 save
  offsets for the transfer budget, salary budget and manager name. The
  Football Life 2026 finance offset was confirmed against real saves. No code
  or binaries from the tool are included.
- **PES 2021 / FL Cheat Table** by xAranaktu is the source of the runtime field
  layout (contract date packing, money units) used to cross-check the save
  format. No code is included.
- **pes-file-reader** (leo-grayrat) was used as research reference only (money
  unit, club table, integrity chain). No code is included.
- `data/FL2622wc_players.txt` is the ID/name list shipped with
  FL26 Transfer Editor v0.5.1. It is embedded in the binary as a fallback name
  source.

This project is not affiliated with Konami or SmokePatch.
