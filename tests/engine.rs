mod common;

use common::*;
use fl_ml_desk::contracts::{BLANK, RECORD};
use fl_ml_desk::deals;
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
    assert_eq!(game.finances().unwrap(), Finances { transfer_eur: 50_000_000, salary_eur: 20_000_000 });
    assert_eq!(game.manager_name().unwrap(), "Test Manager");
    assert_eq!(game.contract_count(), SQUAD);
    let expected_wages: i64 = (0..SQUAD as i64).map(|j| (10_000 + j) * 100).sum();
    assert_eq!(game.wage_bill_eur(), expected_wages);
    assert_eq!(game.game_date, Some(date(2025, 8, 6)));
    assert_eq!(game.described_club, Some(MANAGED));
    assert_eq!(game.contract_end_in(3), date(2028, 8, 31));
    assert_eq!(game.contract_end_in(1), date(2026, 8, 31));
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
fn unrecognised_lists_fall_back_to_joining_right_away() {
    // The row before contract slot 0 lost its marker, so no agreed deal can be written.
    let mut data = build_data(false);
    data[FIRST_ROW - RECORD + 32] ^= 1;
    let mut game = open_with(&TempDir::new("incoming"), data);
    let original = game.data.clone();
    let target = club_player(7, 2);
    let op = game.transfer(incoming(7, target.player_id, 12_345_678, 2_500_049, date(2029, 8, 31))).unwrap();
    assert!(op.description.contains("joined right away"));
    let fin = game.finances().unwrap();
    assert_eq!(fin.transfer_eur, 50_000_000 - 12_345_700);
    // The game derives the remaining salary budget from the wage bill.
    assert_eq!(fin.salary_eur, 20_000_000);
    assert!(game.roster(Source::Club(MANAGED)).contains(&target));
    assert!(!game.roster(Source::Club(7)).contains(&target));
    let c = game.contract_of(&target).unwrap();
    // Appended after the last row (squad + negotiation); the row before it is untouched.
    assert_eq!(c.offset, FIRST_ROW + (SQUAD + 1) * RECORD);
    assert_eq!(words(&game.data, c.offset - RECORD), words(&original, c.offset - RECORD));
    assert_eq!((c.salary_units, c.value_units, c.end), (25_000, 123_457, date(2029, 8, 31)));
    assert_eq!((c.index, c.team, c.status, c.start), (NEGOTIATION_INDEX + 1, team_ref(MANAGED), 5, Some(date(2025, 8, 6))));
    assert_eq!(&words(&game.data, c.offset)[4..7], &[0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF]);
    assert_eq!(words(&game.data, c.offset + RECORD), BLANK);
    assert_eq!(game.contract_count(), SQUAD + 1);
    // Roster count keeps the flag bits of its word.
    let count = u16::from_le_bytes(game.data[ml::TABLE_START + ml::COUNT_OFFSET..][..2].try_into().unwrap());
    assert_eq!(count, 0x8000 | (SQUAD as u16 + 1));
}

fn wordsn<const N: usize>(data: &[u8], off: usize) -> [u32; N] {
    core::array::from_fn(|i| u32::from_le_bytes(data[off + 4 * i..off + 4 * i + 4].try_into().unwrap()))
}

fn agreed(source: usize, pid: u32) -> TransferRequest {
    incoming(source, pid, 12_345_678, 2_500_049, date(2029, 8, 31))
}

#[test]
fn agreed_deal_waits_for_the_game_to_move_the_player() {
    let dir = TempDir::new("agreed");
    let mut game = open(&dir);
    let original = game.data.clone();
    let wages = game.wage_bill_eur();
    let target = club_player(7, 2);
    game.transfer(agreed(7, target.player_id)).unwrap();

    // The fee is paid now; the player stays at his club until the game moves him.
    assert_eq!(game.finances().unwrap().transfer_eur, 50_000_000 - 12_345_700);
    assert!(game.roster(Source::Club(7)).contains(&target));
    assert!(!game.roster(Source::Club(MANAGED)).contains(&target));
    assert_eq!((game.contract_count(), game.wage_bill_eur()), (SQUAD, wages));

    // Offer ids and contract indexes share one numbering: the squad uses
    // 0..SQUAD and the open negotiation SQUAD (offer) and SQUAD + 1 (contract).
    let (offer_id, index) = (NEGOTIATION_INDEX + 1, NEGOTIATION_INDEX + 2);
    let arrival = date(2025, 8, 7).pack();
    let (iid, pid) = (target.internal_id, target.player_id);
    assert_eq!(
        words(&game.data, FIRST_ROW + (SQUAD + 1) * RECORD),
        [index, team_ref(MANAGED), 4, arrival, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, iid, pid, 25_000, 123_457, date(2029, 8, 31).pack()]
    );
    let offers = FIRST_ROW + deals::OFFERS_OFFSET;
    let (nc, nj) = NEGOTIATING;
    assert_eq!(wordsn::<15>(&game.data, offers), negotiation_offer(club_player(nc, nj), nc));
    assert_eq!(
        wordsn::<15>(&game.data, offers + deals::OFFER_RECORD),
        [offer_id, team_ref(MANAGED), 4, arrival, 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, team_ref(7), iid, pid, 123_457, 123_457, 0xFFFF, 0xFFFF, 0]
    );
    let pair: Vec<u8> = [offer_id as u16, index as u16, 0xFFFF].iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(&game.data[FIRST_ROW + deals::AGREED_OFFSET..][..6], &pair[..]);
    assert_eq!(wordsn::<3>(&game.data, deals::PENDING_ARRIVALS), [iid, pid, 25_000]);
    assert_eq!(wordsn::<13>(&game.data, deals::SEASON_DEALS), [iid, pid, 0, 0, 0, 0, 0, 0, (-123_457i32) as u32, 1, 0, 0, 0xFFFF]);
    // News item after the existing one, dated the agreement day (6 August).
    let mut news = deals::BLANK_NEWS;
    (news[0], news[1], news[2], news[13]) = (0x6E, iid, pid, date(2025, 8, 6).pack());
    assert_eq!(wordsn::<14>(&game.data, deals::NEWS + deals::NEWS_ROW), news);
    // Timeline: slot 1 (slot 0 is the AI club's) gets an offer block and a
    // contract block, owner 0, round 1 that day, round 2 until 31 August.
    let slot = |n: usize| FIRST_ROW + deals::TIMELINE_OFFSET + n * deals::TIMELINE_SLOT;
    let block = |number: u32| -> Vec<u8> {
        [0, 0xFFFF, number as u16, 1, 2025, 6 << 8 | 8, 2025, 6 << 8 | 8, number as u16, 2, 2025, 6 << 8 | 8, 2025, 31 << 8 | 8]
            .iter()
            .flat_map(|v: &u16| v.to_le_bytes())
            .collect()
    };
    let len = block(0).len();
    assert_eq!(&game.data[slot(1)..slot(1) + len], &block(offer_id)[..]);
    let contract_block = slot(1) + deals::TIMELINE_BLOCK;
    assert_eq!(&game.data[contract_block..contract_block + len], &block(index)[..]);

    // A second deal takes the next free numbers, slots and timeline slot.
    let second = club_player(8, 1);
    game.transfer(agreed(8, second.player_id)).unwrap();
    assert_eq!(wordsn::<15>(&game.data, offers + 2 * deals::OFFER_RECORD)[0], index + 1);
    assert_eq!(wordsn::<3>(&game.data, deals::PENDING_ARRIVALS + deals::PENDING_ENTRY)[1], second.player_id);
    assert_eq!(&game.data[slot(2)..slot(2) + len], &block(index + 1)[..]);
    // Agreeing again on the same player is refused.
    assert!(game.transfer(agreed(7, target.player_id)).is_err());

    game.undo().unwrap();
    game.undo().unwrap();
    assert_eq!(game.data, original);
}

#[test]
fn player_in_a_game_negotiation_is_refused() {
    let dir = TempDir::new("negotiating");
    let mut game = open(&dir);
    let (nc, nj) = NEGOTIATING;
    let before = game.data.clone();
    assert!(game.transfer(agreed(nc, club_player(nc, nj).player_id)).is_err());
    assert_eq!(game.data, before);
}

#[test]
fn next_day_rolls_over_months_and_years() {
    assert_eq!(date(2025, 8, 31).next_day(), date(2025, 9, 1));
    assert_eq!(date(2025, 12, 31).next_day(), date(2026, 1, 1));
    assert_eq!(date(2028, 2, 28).next_day(), date(2028, 2, 29));
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
    assert_eq!(words(&game.data, FIRST_ROW + SQUAD * RECORD), BLANK);
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
fn free_agent_deal_has_no_offer_and_no_fee() {
    let dir = TempDir::new("others");
    let mut game = open(&dir);
    let original = game.data.clone();
    let cand = game.other_candidates()[0];
    game.transfer(TransferRequest {
        salary_eur: Some(500_000),
        contract_end: Some(date(2028, 8, 31)),
        ..TransferRequest::new(Source::Others, MANAGED, cand.player_id)
    })
    .unwrap();
    // The game moves him on the arrival date; until then he stays with his national team only.
    assert!(!game.roster(Source::Club(MANAGED)).contains(&cand));
    assert!(game.other_candidates().contains(&cand));
    // Without an offer the contract takes the lowest free number (0..SQUAD and
    // the negotiation's two numbers are taken).
    let index = NEGOTIATION_INDEX + 1;
    let (iid, pid) = (cand.internal_id, cand.player_id);
    assert_eq!(
        words(&game.data, FIRST_ROW + (SQUAD + 1) * RECORD),
        [index, team_ref(MANAGED), 4, date(2025, 8, 7).pack(), 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, iid, pid, 5_000, 0, date(2028, 8, 31).pack()]
    );
    let pair: Vec<u8> = [index as u16, 0xFFFF, 0xFFFF].iter().flat_map(|v| v.to_le_bytes()).collect();
    assert_eq!(&game.data[FIRST_ROW + deals::AGREED_OFFSET..][..6], &pair[..]);
    assert_eq!(wordsn::<3>(&game.data, deals::PENDING_ARRIVALS), [iid, pid, 5_000]);
    // No offer and no fee entry.
    let offers = FIRST_ROW + deals::OFFERS_OFFSET;
    let unchanged = |off: usize, len: usize| game.data[off..off + len] == original[off..off + len];
    assert!(unchanged(offers + deals::OFFER_RECORD, deals::OFFER_RECORD));
    assert!(unchanged(deals::SEASON_DEALS, deals::DEAL_ENTRY));
    // A news item, but no negotiation timeline.
    assert_eq!(wordsn::<3>(&game.data, deals::NEWS + deals::NEWS_ROW), [0x6E, iid, pid]);
    assert!(unchanged(FIRST_ROW + deals::TIMELINE_OFFSET + deals::TIMELINE_SLOT, deals::TIMELINE_SLOT));
}

#[test]
fn manager_rename_keeps_neighbouring_byte() {
    let dir = TempDir::new("rename");
    let mut game = open(&dir);
    let budget = game.finances().unwrap();
    game.set_manager_name("New Coach").unwrap();
    assert_eq!(game.manager_name().unwrap(), "New Coach");
    assert_eq!(game.finances().unwrap(), budget);
    assert_eq!(game.data[ml::MANAGER_NAME + ml::MANAGER_NAME_SIZE], b'V');
    assert!(game.set_manager_name(&"x".repeat(46)).is_err());
}

#[test]
fn manager_name_and_budget_are_independent() {
    let dir = TempDir::new("independent");
    let mut data = build_data(false);
    data[ml::MANAGER_NAME..ml::MANAGER_NAME + ml::MANAGER_NAME_SIZE].fill(0);
    let mut game = open_with(&dir, data);
    assert!(game.manager_error.is_some() && game.manager_name().is_err());
    game.set_finances(60_000_000, 20_000_000).unwrap();
    assert_eq!(game.finances().unwrap().transfer_eur, 60_000_000);

    let dir = TempDir::new("independent-budget");
    let mut data = build_data(false);
    put_words(&mut data, ml::FL26_FINANCE, &[500_000, 1, 2, 3, 4, 200_000]);
    let mut game = open_with(&dir, data);
    assert!(game.finance_error.is_some());
    game.set_manager_name("New Coach").unwrap();
    assert_eq!(game.manager_name().unwrap(), "New Coach");
}

#[test]
fn undo_restores_exact_bytes() {
    let dir = TempDir::new("undo");
    let mut game = open(&dir);
    let original = game.data.clone();
    game.set_finances(99_000_000, 30_000_000).unwrap();
    game.set_manager_name("New Coach").unwrap();
    game.transfer(incoming(7, club_player(7, 1).player_id, 1_000_000, 900_000, date(2028, 8, 31))).unwrap();
    game.transfer(TransferRequest { fee_eur: 500_000, ..TransferRequest::new(Source::Club(MANAGED), 9, club_player(0, 4).player_id) })
        .unwrap();
    for _ in 0..4 {
        game.undo().unwrap();
    }
    assert_eq!(game.data, original);
}

#[test]
fn save_refuses_unjournaled_changes() {
    let dir = TempDir::new("unjournaled");
    let mut game = open(&dir);
    game.set_finances(80_000_000, 20_000_000).unwrap();
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
    game.set_finances(80_000_000, 20_000_000).unwrap();
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
    let row = game.contracts.as_ref().unwrap().find(&game.data, &target)[0];
    assert_eq!((row.status, row.end), (4, date(2029, 8, 31)));
}

#[test]
fn rounding_matches_python_ties_to_even() {
    assert_eq!(ml::eur_to_units(77_000_050), 770_000);
    assert_eq!(ml::eur_to_units(77_000_150), 770_002);
    assert_eq!(ml::div_round_even(-12_345_678, 100), -123_457);
    assert_eq!(ml::fmt_eur(12_345_678), "€12,345,678");
}
