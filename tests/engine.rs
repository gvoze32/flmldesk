mod common;

use common::*;
use fl_ml_desk::contracts::{BLANK, RECORD};
use fl_ml_desk::crypto;
use fl_ml_desk::masterleague::{self as ml, Finances, MasterLeague, Source, TransferRequest};

fn open_with(dir: &TempDir, data: Vec<u8>) -> MasterLeague {
    let path = write_save(&dir.0.join("ML00000001"), data);
    let mut game = MasterLeague::open(&path).unwrap();
    game.set_managed_team(MANAGED).unwrap();
    game
}

fn open(dir: &TempDir) -> MasterLeague {
    open_with(dir, build_data(false))
}

fn incoming(source: usize, pid: u32, fee: i64, salary: i64, end: fl_ml_desk::contracts::Date) -> TransferRequest {
    TransferRequest {
        fee_eur: fee,
        salary_eur: Some(salary),
        contract_end: Some(end),
        ..TransferRequest::new(Source::Club(source), MANAGED, pid)
    }
}

fn words(data: &[u8], off: usize) -> [u32; 12] {
    core::array::from_fn(|i| u32::from_le_bytes(data[off + 4 * i..off + 4 * i + 4].try_into().unwrap()))
}

fn salary_budget(game: &MasterLeague) -> i64 {
    game.finances().unwrap().salary_eur
}

#[test]
fn corrupted_block_is_rejected() {
    let dir = TempDir::new("corrupt");
    let path = write_save(&dir.0.join("ML00000001"), vec![0u8; 4096]);
    let mut blob = std::fs::read(path).unwrap();
    let n = blob.len();
    blob[n - 100] ^= 1;
    assert!(crypto::unpack(&blob, true).is_err());
}

#[test]
fn discovers_clubs_finances_contracts_and_description() {
    let dir = TempDir::new("discover");
    let game = open(&dir);
    assert_eq!(game.teams.len(), CLUBS);
    assert_eq!(
        game.finances().unwrap(),
        Finances { transfer_eur: 50_000_000, salary_eur: 20_000_000, manager_name: "Test Manager".into() }
    );
    assert_eq!(game.contract_count(), SQUAD);
    let expected_wages: i64 = (0..SQUAD as i64).map(|j| (10_000 + j) * 100).sum();
    assert_eq!(game.wage_bill_eur(), expected_wages);
    assert_eq!(game.game_date, Some(date(2025, 8, 6)));
    assert_eq!(game.described_club, Some(MANAGED));
    assert_eq!(game.default_contract_end(), date(2028, 8, 31));
    assert_eq!(game.other_candidates().len(), 2 * NATIONAL);
}

#[test]
fn localized_team_names_use_abbreviations() {
    let dir = TempDir::new("localized");
    assert_eq!(open_with(&dir, build_data(true)).teams.len(), CLUBS);
}

#[test]
fn pes21_budget_block_is_found_at_its_own_offset() {
    let dir = TempDir::new("pes21");
    let mut data = build_data(false);
    data[ml::FL26_FINANCE..ml::FL26_FINANCE + 0x18].fill(0xAB);
    put_words(&mut data, ml::PES21_FINANCE, &[123_456, 0, 0, 0, 0, 65_432]);
    let path = write_save_as(&dir.0.join("ML00000001"), data, "eFootball PES 2021 SEASON UPDATE", "Club 0 / League\n1/7/2021\nLeague");
    let game = MasterLeague::open(&path).unwrap();
    let fin = game.finances().unwrap();
    assert_eq!((fin.transfer_eur, fin.salary_eur), (12_345_600, 6_543_200));
}

#[test]
fn new_signing_appends_contract_and_charges_only_the_fee() {
    let dir = TempDir::new("incoming");
    let mut game = open(&dir);
    let target = club_player(7, 2);
    game.transfer(incoming(7, target.player_id, 12_345_678, 2_500_049, date(2029, 8, 31))).unwrap();
    let fin = game.finances().unwrap();
    assert_eq!(fin.transfer_eur, 50_000_000 - 12_345_700);
    // The game derives the remaining salary budget from the wage bill.
    assert_eq!(fin.salary_eur, 20_000_000);
    assert!(game.roster(Source::Club(MANAGED)).contains(&target));
    assert!(!game.roster(Source::Club(7)).contains(&target));
    let c = game.contract_of(&target).unwrap();
    // Appended after the last row (squad + negotiation + unassigned).
    assert_eq!(c.offset, FIRST_ROW + (SQUAD + 2) * RECORD);
    assert_eq!((c.salary_units, c.value_units, c.end), (25_000, 123_457, date(2029, 8, 31)));
    assert_eq!((c.index, c.team, c.status, c.start), (32, team_ref(MANAGED), 5, Some(date(2025, 8, 6))));
    assert_eq!(&words(&game.data, c.offset)[9..], &[0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF]);
    assert_eq!(game.contract_count(), SQUAD + 1);
    // Roster count keeps the flag bits of its word.
    let count = u16::from_le_bytes(game.data[ml::TABLE_START + ml::COUNT_OFFSET..][..2].try_into().unwrap());
    assert_eq!(count, 0x8000 | (SQUAD as u16 + 1));
}

#[test]
fn negotiated_player_completes_existing_row_in_place() {
    let dir = TempDir::new("negotiated");
    let mut game = open(&dir);
    let (club, j) = NEGOTIATING;
    let target = club_player(club, j);
    game.transfer(incoming(club, target.player_id, 5_000_000, 3_000_000, date(2028, 8, 31))).unwrap();
    let c = game.contract_of(&target).unwrap();
    assert_eq!(c.offset, FIRST_ROW + SQUAD * RECORD);
    assert_eq!((c.index, c.value_units, c.status, c.team), (31, 777_000, 5, team_ref(MANAGED)));
    assert_eq!(c.start, Some(date(2025, 8, 6)));
}

#[test]
fn unassigned_row_gets_an_index_when_signed() {
    let dir = TempDir::new("unassigned");
    let mut game = open(&dir);
    let (club, j) = UNASSIGNED;
    let target = club_player(club, j);
    game.transfer(incoming(club, target.player_id, 0, 2_000_000, date(2027, 8, 31))).unwrap();
    let c = game.contract_of(&target).unwrap();
    assert_eq!((c.offset, c.index), (FIRST_ROW + (SQUAD + 1) * RECORD, 32));
}

#[test]
fn insufficient_budget_leaves_save_untouched() {
    let dir = TempDir::new("insufficient");
    let mut game = open(&dir);
    let before = game.data.clone();
    let pid = club_player(7, 3).player_id;
    assert!(game.transfer(incoming(7, pid, 60_000_000, 1_000_000, date(2029, 8, 31))).is_err());
    assert_eq!(game.data, before);
    assert!(game.operations().is_empty());
}

#[test]
fn contract_must_end_after_game_date() {
    let dir = TempDir::new("end");
    let mut game = open(&dir);
    let pid = club_player(7, 3).player_id;
    assert!(game.transfer(incoming(7, pid, 0, 1_000_000, date(2025, 8, 1))).is_err());
}

#[test]
fn sale_removes_contract_compacts_table_and_credits_fee() {
    let dir = TempDir::new("outgoing");
    let mut game = open(&dir);
    let seller = club_player(MANAGED, 2);
    let next = words(&game.data, FIRST_ROW + 3 * RECORD);
    game.transfer(TransferRequest { fee_eur: 3_000_000, ..TransferRequest::new(Source::Club(MANAGED), 7, seller.player_id) })
        .unwrap();
    assert!(game.contract_of(&seller).is_none());
    assert_eq!(words(&game.data, FIRST_ROW + 2 * RECORD), next);
    assert_eq!(words(&game.data, FIRST_ROW + (SQUAD + 1) * RECORD), BLANK);
    assert_eq!(game.contract_count(), SQUAD - 1);
    assert_eq!(game.finances().unwrap().transfer_eur, 53_000_000);
    assert_eq!(salary_budget(&game), 20_000_000);
    assert!(game.roster(Source::Club(7)).contains(&seller));
}

#[test]
fn ai_transfer_touches_rosters_only() {
    let dir = TempDir::new("ai");
    let mut game = open(&dir);
    let before = game.finances().unwrap();
    let pid = club_player(5, 0).player_id;
    game.transfer(TransferRequest { fee_eur: 9_000_000, ..TransferRequest::new(Source::Club(5), 7, pid) }).unwrap();
    assert_eq!(game.finances().unwrap(), before);
}

#[test]
fn other_source_signing_keeps_national_team() {
    let dir = TempDir::new("others");
    let mut game = open(&dir);
    let cand = game.other_candidates()[0];
    game.transfer(TransferRequest {
        salary_eur: Some(500_000),
        contract_end: Some(date(2028, 8, 31)),
        ..TransferRequest::new(Source::Others, MANAGED, cand.player_id)
    })
    .unwrap();
    assert!(game.roster(Source::Club(MANAGED)).contains(&cand));
    assert!(!game.other_candidates().contains(&cand));
    assert!(game.contract_of(&cand).is_some());
}

#[test]
fn manager_rename_keeps_neighbouring_byte() {
    let dir = TempDir::new("rename");
    let mut game = open(&dir);
    game.set_finances(50_000_000, 20_000_000, "Pelatih Baru").unwrap();
    assert_eq!(game.finances().unwrap().manager_name, "Pelatih Baru");
    assert_eq!(game.data[ml::MANAGER_NAME + ml::MANAGER_NAME_SIZE], b'V');
    assert!(game.set_finances(50_000_000, 20_000_000, &"x".repeat(46)).is_err());
}

#[test]
fn undo_restores_exact_bytes() {
    let dir = TempDir::new("undo");
    let mut game = open(&dir);
    let original = game.data.clone();
    game.set_finances(99_000_000, 30_000_000, "Pelatih Baru").unwrap();
    game.transfer(incoming(7, club_player(7, 1).player_id, 1_000_000, 900_000, date(2028, 8, 31))).unwrap();
    game.transfer(TransferRequest { fee_eur: 500_000, ..TransferRequest::new(Source::Club(MANAGED), 9, club_player(0, 4).player_id) })
        .unwrap();
    for _ in 0..3 {
        game.undo().unwrap();
    }
    assert_eq!(game.data, original);
}

#[test]
fn save_refuses_unjournaled_changes() {
    let dir = TempDir::new("unjournaled");
    let mut game = open(&dir);
    game.set_finances(80_000_000, 20_000_000, "Test Manager").unwrap();
    game.data[0x100] ^= 1;
    let blob = std::fs::read(&game.path).unwrap();
    assert!(game.write(None).is_err());
    assert_eq!(std::fs::read(&game.path).unwrap(), blob);
}

#[test]
fn write_backs_up_and_reloads() {
    let dir = TempDir::new("write");
    let base = build_data(false);
    let mut game = open_with(&dir, base.clone());
    let original_blob = std::fs::read(&game.path).unwrap();
    game.set_finances(80_000_000, 20_000_000, "Test Manager").unwrap();
    let (target, backup) = game.write(None).unwrap();
    assert_eq!(std::fs::read(backup.unwrap()).unwrap(), original_blob);
    let again = MasterLeague::open(&target).unwrap();
    assert_eq!(again.finances().unwrap().transfer_eur, 80_000_000);
    let diff: Vec<usize> = (0..base.len()).filter(|&i| base[i] != again.data[i]).collect();
    assert!(!diff.is_empty() && diff.iter().all(|&i| (ml::FL26_FINANCE..ml::FL26_FINANCE + 4).contains(&i)));
    assert!(game.history.is_empty());
}

#[test]
fn unknown_finance_layout_fails_closed() {
    let dir = TempDir::new("finance");
    let mut data = build_data(false);
    put_words(&mut data, ml::FL26_FINANCE, &[500_000, 1, 2, 3, 4, 200_000]);
    let mut game = open_with(&dir, data);
    assert!(game.finance_error.is_some());
    assert!(game.finances().is_err());
    let target = club_player(7, 3);
    assert!(game.transfer(incoming(7, target.player_id, 1_000_000, 1_000_000, date(2029, 8, 31))).is_err());
    game.transfer(TransferRequest { use_transfer_budget: false, ..incoming(7, target.player_id, 1_000_000, 1_000_000, date(2029, 8, 31)) })
        .unwrap();
    assert_eq!(game.contract_of(&target).unwrap().end, date(2029, 8, 31));
}

#[test]
fn rounding_matches_python_ties_to_even() {
    assert_eq!(ml::eur_to_units(77_000_050), 770_000);
    assert_eq!(ml::eur_to_units(77_000_150), 770_002);
    assert_eq!(ml::div_round_even(-12_345_678, 100), -123_457);
    assert_eq!(ml::fmt_eur(12_345_678), "€12.345.678");
}
