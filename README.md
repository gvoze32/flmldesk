# FL ML Desk

Desktop editor (Rust + egui) for ongoing **Football Life 2026** and
**PES 2021** Master League saves (`ML000000xx`):

- **Sign players for your club** with the fee, annual salary and contract end
  you choose. The fee comes out of the transfer budget and the contract is
  registered straight away.
- **Sell players from your club**: their contract is removed and the fee is
  added to the transfer budget.
- **Move players between AI clubs** (squads only).
- **Edit finances**: transfer budget, annual salary budget, manager name.
- **Update check** against GitHub Releases.

One Windows executable, no Python or installer: download `FLMLDesk-vX.Y.Z-windows-x64.exe`
from the [Releases](../../releases) page. Other systems can build from source
(`cargo build --release`).

## Usage

1. In the game, save the Master League and **quit to the main menu or close the
   game**.
2. Run `FLMLDesk-vX.Y.Z-windows-x64.exe`.
3. **Buka save ML…** (open), or drag the save file onto the window. Saves live in
   `Documents\KONAMI\eFootball PES 2021 SEASON UPDATE\<folder>\save\`.
   - The club you manage and the in-game date are read from the save.
   - An `EDIT00000000` in the same folder is used for player names.
4. **Folder game (nama/OVR)…** (once): pick the game installation folder (the one
   containing `download`). Names, positions, ages and OVR of every player come
   from `Player.bin` in the `.cpk` archives.
5. **Transfer pemain** tab: search a player, fill in **Fee transfer**,
   **Gaji per tahun** (salary per year) and **Kontrak s.d.** (contract end), then
   press **Transfer →**.
   - Amounts accept `12.500.000`, `12,5jt`, `750rb`, `3m` or `500k`.
   - Selecting one of your own players switches to **sell** mode.
6. **Budget & manajer** tab: change the values and press **Terapkan** (apply).
7. **Simpan ke save (backup otomatis)** (save with backup). The current file is
   first copied to `FLMLDesk_backup\` next to the save. Then load the save in
   the game.

Every change can be undone one by one before saving. **Mata uang** (currency)
only affects display and input and uses the game's own rates (e.g. ¥ = € × 125).
The save always stores amounts in units of €100.

## What gets written

Offsets are relative to the decrypted `data` block. "Confirmed" means matched
against real FL26 saves (before a deal, deal agreed, player arrived) and the
values shown in game.

| Data | Location | Status |
|---|---|---|
| Club squad (40 slots) | `0x50 + club·0x690`, roster `+0x14C`, count `+0x426` | Confirmed |
| Transfer budget | FL26 `0xC7DD00`, PES21 `0xC7DE00`, u32 × €100 | FL26 confirmed (¥307,001,287,500 = 24,560,103 × 12,500); PES21 from ML Money Tool |
| Annual salary budget | budget `+0x14`, u32 × €100 | Confirmed; the game shows it minus the wage bill |
| Manager name | `0x1942C0`, 46 bytes UTF-8 | Confirmed |
| Your club's contract table | 48-byte rows, located through the squad | Confirmed, see below |

Contract row (12 × u32):

| Word | Content |
|---|---|
| w0, w1 | internal id, player id |
| w2 | annual salary (× €100) |
| w3 | player value at signing (× €100) |
| w4 | contract end (`year \| month<<16 \| day<<24`) |
| w5 | row index (low 16 bits) |
| w6 | club reference (low 14 bits = club index) |
| w7 | status: 5 = under contract, 4 = deal agreed, 1/3 = negotiation |
| w8 | contract start date |

What the game does on a signing, and the editor reproduces:

- The fee is charged to the transfer budget **when the deal is agreed**. In the
  sample saves the budget dropped by exactly the sum of three fees.
- The stored salary budget is **not** changed. The new wage lowers the remaining
  amount the game displays.
- The contract row gets status 5, your club, and the in-game date as start
  date. A player already in your negotiation list has his row completed in
  place; anyone else gets a new row at the end of the table.

On save the SHA-512 digests are recomputed and the output is decrypted again
for verification before it is written. Saving is refused if any byte changed
outside the recorded edits.

## Limitations

- Still unknown: the row index (w5) the game picks for a new signing, and the
  player value (w3). The editor uses the next index and stores the fee as w3.
  Replaying the Max Dowman signing on the `before` save matched the game's own
  row except for these two fields.
- **Selling** removes the contract row and moves later rows up. No sample save
  of a sale exists yet; check in game.
- **Loans are not supported.** All transfers are permanent.
- **Free agents**: players under "Lainnya (timnas, tanpa klub)" (national team,
  no club) can be *Others* or *free agents* in game. Moving free agents with an
  editor is known to create "ghost players".
- The fee history in the game's finance report is not extended.
- AI clubs have no contracts edited; AI ↔ AI moves only change squads.
- At most 40 players per club. Shirt numbers and line-ups are set in game.
- The vanilla PES 2021 budget offset has not been tested on a real save. If the
  structure does not match, budget editing is locked.

## Releases & updates

Every push to `main` runs the tests and cross-compiles the Windows `.exe` on a
single Ubuntu runner (`.github/workflows/release.yml`). If `version` in `Cargo.toml` has no `vX.Y.Z`
release yet, the workflow publishes it with the downloads. The app checks the
latest release of the `repository` in `Cargo.toml` on start; **Cek update**
checks manually.

To release: bump `version` in `Cargo.toml` and push.

## Development

```bash
cargo run --release                          # run the app
cargo run --release -- path/to/ML00000000    # open a save directly
cargo test --release                         # tests (synthetic ML saves, no game files)
```

Credits and third-party licences: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
