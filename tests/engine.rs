mod common;

use common::*;
use fl_ml_desk::contracts::{BLANK, RECORD};
use fl_ml_desk::crypto;
use fl_ml_desk::deals;
use fl_ml_desk::masterleague::{self as ml, Finances, LoanRequest, MasterLeague, Source, TransferRequest};
use fl_ml_desk::playerdb as pdb;

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
fn second_season_budget_block_is_editable() {
    // From a second-season FL26 save: +0x10 holds the transfer budget the season started with.
    let dir = TempDir::new("second-season");
    let mut data = build_data(false);
    put_words(&mut data, ml::FL26_FINANCE, &[1_251_105, 0, 0, 0, 1_251_105, 2_340_967]);
    let mut game = open_with(&dir, data);
    assert!(game.finance_error.is_none());
    let fin = game.finances().unwrap();
    assert_eq!((fin.transfer_eur, fin.salary_eur), (125_110_500, 234_096_700));
    game.set_finances(100_000_000, 234_096_700).unwrap();
    assert_eq!(u32_at(&game.data, ml::FL26_FINANCE + 0x10), 1_251_105);
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

fn loan(club: usize, outgoing: bool, pid: u32, fee: i64, end: fl_ml_desk::contracts::Date) -> LoanRequest {
    LoanRequest { club, outgoing, player_id: pid, fee_eur: fee, loan_end: end, use_transfer_budget: true, name: None }
}

fn timeline_slot(n: usize) -> usize {
    FIRST_ROW + deals::TIMELINE_OFFSET + n * deals::TIMELINE_SLOT
}

fn u16_bytes(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

#[test]
fn loan_in_is_an_agreed_offer_without_contract_or_pending_arrival() {
    let dir = TempDir::new("loan-in");
    let mut game = open(&dir);
    let original = game.data.clone();
    let target = club_player(7, 2);
    let (iid, pid) = (target.internal_id, target.player_id);
    let end = date(2025, 12, 31);
    game.loan(loan(7, false, pid, 1_000_000, end)).unwrap();

    assert_eq!(game.finances().unwrap().transfer_eur, 49_000_000);
    assert!(game.roster(Source::Club(7)).contains(&target), "the game moves him on the next day");
    assert_eq!(game.contracts.as_ref().unwrap().find(&game.data, &target).len(), 0, "no contract row");
    let offer_id = NEGOTIATION_INDEX + 1;
    let offers = FIRST_ROW + deals::OFFERS_OFFSET;
    assert_eq!(
        wordsn::<15>(&game.data, offers + deals::OFFER_RECORD),
        [offer_id, team_ref(MANAGED), 4, date(2025, 8, 7).pack(), 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, team_ref(7), iid, pid, 10_000, 10_000, end.pack(), 0xFFFF, 0]
    );
    assert_eq!(&game.data[FIRST_ROW + deals::AGREED_OFFSET..][..6], &u16_bytes(&[offer_id as u16, 0xFFFF, 0xFFFF])[..]);
    assert_eq!(wordsn::<3>(&game.data, deals::PENDING_ARRIVALS), [0xFFFF, 0, 0], "no pending arrival");
    assert_eq!(wordsn::<13>(&game.data, deals::SEASON_DEALS), [iid, pid, 0, 0, 0, 0, 0, 0, (-10_000i32) as u32, 2, 0, 0, 0xFFFF]);
    let mut news = deals::BLANK_NEWS;
    (news[0], news[1], news[2], news[13]) = (0x6E, iid, pid, date(2025, 8, 6).pack());
    assert_eq!(wordsn::<14>(&game.data, deals::NEWS + deals::NEWS_ROW), news);
    let (today, window) = (6 << 8 | 8, 31 << 8 | 8);
    let block = u16_bytes(&[0, 0xFFFF, offer_id as u16, 1, 2025, today, 2025, today, offer_id as u16, 2, 2025, today, 2025, window]);
    assert_eq!(&game.data[timeline_slot(1)..][..block.len()], &block[..]);
    assert_eq!(&game.data[timeline_slot(1) + deals::TIMELINE_BLOCK..][..4], &original[timeline_slot(1) + deals::TIMELINE_BLOCK..][..4], "contract block stays free");

    assert!(game.loan(loan(7, false, pid, 0, end)).is_err(), "agreed twice");
    game.undo().unwrap();
    assert_eq!(game.data, original);
}

#[test]
fn loan_out_credits_the_fee_keeps_the_contract_and_blocks_a_sale() {
    let dir = TempDir::new("loan-out");
    let mut game = open(&dir);
    let player = club_player(MANAGED, 3);
    let (iid, pid) = (player.internal_id, player.player_id);
    let end = date(2026, 6, 30);
    game.loan(loan(7, true, pid, 500_000, end)).unwrap();

    assert_eq!(game.finances().unwrap().transfer_eur, 50_500_000);
    assert!(game.contract_of(&player).is_some(), "his contract stays");
    assert!(game.roster(Source::Club(MANAGED)).contains(&player));
    let offer_id = NEGOTIATION_INDEX + 1;
    let offers = FIRST_ROW + deals::OFFERS_OFFSET;
    assert_eq!(
        wordsn::<15>(&game.data, offers + deals::OFFER_RECORD),
        [offer_id, team_ref(7), 4, date(2025, 8, 7).pack(), 0xFFFF_FFFF, 0xFFFF_FFFF, 0xFFFF, team_ref(MANAGED), iid, pid, 5_000, 5_000, end.pack(), 0xFFFF, 0]
    );
    assert_eq!(wordsn::<13>(&game.data, deals::SEASON_DEALS), [iid, pid, 0, 0, 0, 0, 0, 0, 5_000, 2, 0, 0, 0xFFFF]);
    assert_eq!(wordsn::<14>(&game.data, deals::NEWS + deals::NEWS_ROW)[0], 0x6F);
    let block = u16_bytes(&[0, 0xFFFF, offer_id as u16, 1, 2025, 6 << 8 | 8, 2025, 31 << 8 | 8]);
    assert_eq!(&game.data[timeline_slot(1)..][..block.len()], &block[..]);

    let before = game.data.clone();
    assert!(game.transfer(TransferRequest::new(Source::Club(MANAGED), 8, pid)).is_err(), "sale after an agreed loan");
    assert!(game.release(player, 0, false, None).is_err(), "release after an agreed loan");
    assert!(game.loan(loan(8, true, pid, 0, end)).is_err(), "loaned twice");
    assert_eq!(game.data, before);
}

#[test]
fn loan_checks_end_date_clubs_and_contract() {
    let dir = TempDir::new("loan-bad");
    let mut game = open(&dir);
    let before = game.data.clone();
    let pid = club_player(7, 2).player_id;
    assert!(game.loan(loan(7, false, pid, 0, date(2025, 8, 7))).is_err(), "ends on arrival day");
    assert!(game.loan(loan(MANAGED, false, pid, 0, date(2025, 12, 31))).is_err(), "own club");
    assert!(game.loan(loan(8, false, pid, 0, date(2025, 12, 31))).is_err(), "not at that club");
    assert!(game.loan(loan(7, false, pid, 60_000_000, date(2025, 12, 31))).is_err(), "budget");
    let (nc, nj) = NEGOTIATING;
    assert!(game.loan(loan(nc, false, club_player(nc, nj).player_id, 0, date(2025, 12, 31))).is_err(), "in a negotiation");
    assert_eq!(game.data, before);
    assert!(game.operations().is_empty());
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
fn renewal_changes_only_salary_and_end_of_the_row() {
    let dir = TempDir::new("renew");
    let mut game = open(&dir);
    let player = club_player(MANAGED, 4);
    let off = FIRST_ROW + 4 * RECORD;
    let (before, row_before, records_before) = (game.data.clone(), words(&game.data, off), game.records.clone());
    let wages = game.wage_bill_eur();
    game.renew_contract(player, 2_500_049, date(2030, 6, 30), Some("Renewed")).unwrap();
    let row = words(&game.data, off);
    assert_eq!(row[9], 25_000, "salary rounded to €100 units");
    assert_eq!(row[11], date(2030, 6, 30).pack());
    for w in (0..12).filter(|w| ![9, 11].contains(w)) {
        assert_eq!(row[w], row_before[w], "word {w} must be kept");
    }
    let changed: Vec<usize> = (0..before.len()).filter(|&i| before[i] != game.data[i]).collect();
    assert!(changed.iter().all(|&i| (off + 36..off + 48).contains(&i)), "only the row's salary/end bytes change");
    // His record mirrors the contract's salary and end date.
    let r = rec(&game, player);
    assert_eq!((u32_at(r, pdb::R_SALARY), u32_at(r, pdb::R_CONTRACT_END)), (25_000, date(2030, 6, 30).pack()));
    let changed: Vec<usize> = (0..records_before.len()).filter(|&i| records_before[i] != game.records[i]).collect();
    let at = record_at(player);
    assert!(changed.iter().all(|&i| (at + pdb::R_SALARY..at + pdb::R_SALARY + 4).contains(&i)
        || (at + pdb::R_CONTRACT_END..at + pdb::R_CONTRACT_END + 4).contains(&i)));
    assert_eq!(game.wage_bill_eur(), wages - row_before[9] as i64 * 100 + 2_500_000);
    assert_eq!(game.contract_of(&player).unwrap().end, date(2030, 6, 30));
    game.undo().unwrap();
    assert_eq!(game.data, before);
    assert_eq!(game.records, records_before);
}

fn rec(game: &MasterLeague, p: ml::Player) -> &[u8] {
    &game.records[record_at(p)..record_at(p) + pdb::RECORD]
}

/// Shirt numbers of the club's squad, in squad order.
fn squad_numbers(game: &MasterLeague, club: usize) -> Vec<u16> {
    let base = ml::TABLE_START + club * ml::RECORD_SIZE + ml::NUMBERS_OFFSET;
    let n = game.roster(Source::Club(club)).len();
    (0..n).map(|j| u16::from_le_bytes(game.data[base + 2 * j..][..2].try_into().unwrap())).collect()
}

fn number_after_squad(game: &MasterLeague, club: usize) -> u16 {
    let base = ml::TABLE_START + club * ml::RECORD_SIZE + ml::NUMBERS_OFFSET;
    let n = game.roster(Source::Club(club)).len();
    u16::from_le_bytes(game.data[base + 2 * n..][..2].try_into().unwrap())
}

/// Player ids in the "My Team" list, up to the first free row.
fn my_team(game: &MasterLeague) -> Vec<u32> {
    (0..64)
        .map(|k| wordsn::<4>(&game.data, ml::MY_TEAM + k * ml::MY_TEAM_ROW))
        .take_while(|r| r[3] != 0xC7)
        .map(|r| r[1])
        .collect()
}

#[test]
fn sale_updates_record_shirt_numbers_and_my_team_like_the_game() {
    let dir = TempDir::new("sale-record");
    let mut game = open(&dir);
    let seller = club_player(MANAGED, 2);
    game.transfer(TransferRequest::new(Source::Club(MANAGED), 7, seller.player_id)).unwrap();
    let r = rec(&game, seller);
    assert_eq!(u32_at(r, pdb::R_FROM_CLUB), team_ref(MANAGED));
    assert_eq!(u32_at(r, pdb::R_SEASON_CLUB), team_ref(MANAGED), "kept until the season ends");
    assert_eq!(u32_at(r, pdb::R_JOINED), date(2025, 8, 6).pack());
    assert_eq!((r[pdb::R_FLAGS], r[pdb::R_STATE]), (0x0D, 4));
    assert_eq!(wordsn::<4>(r, pdb::R_COMPETITIONS), [0xFFFF; 4], "not registered for the old club's competitions");
    let (salary, end) = managed_terms(2);
    assert_eq!((u32_at(r, pdb::R_SALARY), u32_at(r, pdb::R_CONTRACT_END)), (salary, end.pack()));
    // Numbers stay with their players; his is free at club 7, so he keeps it.
    let kept: Vec<u16> = (0..SQUAD).filter(|&j| j != 2).map(|j| shirt(MANAGED, j)).collect();
    assert_eq!(squad_numbers(&game, MANAGED), kept);
    assert_eq!(number_after_squad(&game, MANAGED), 0);
    assert_eq!(squad_numbers(&game, 7).last(), Some(&shirt(MANAGED, 2)));
    // "My Team" loses his row; the rows after it move up.
    let listed: Vec<u32> = (0..SQUAD).rev().filter(|&j| j != 2).map(|j| club_player(MANAGED, j).player_id).collect();
    assert_eq!(my_team(&game), listed);
    assert_eq!(wordsn::<4>(&game.data, ml::MY_TEAM + (SQUAD - 1) * ml::MY_TEAM_ROW), [0xFFFF, 0, 0, 0xC7]);
}

#[test]
fn ai_move_takes_the_lowest_free_number_when_his_is_taken() {
    let dir = TempDir::new("ai");
    let mut game = open(&dir);
    let (before, my_list) = (game.finances().unwrap(), my_team(&game));
    let p = club_player(5, 0);
    game.transfer(TransferRequest { fee_eur: 9_000_000, ..TransferRequest::new(Source::Club(5), 7, p.player_id) }).unwrap();
    assert_eq!(game.finances().unwrap(), before);
    assert_eq!(my_team(&game), my_list);
    // Club 7 already uses his 21, and 1 is the lowest it doesn't use.
    assert_eq!(squad_numbers(&game, 7).last(), Some(&1));
    let r = rec(&game, p);
    assert_eq!((u32_at(r, pdb::R_FROM_CLUB), r[pdb::R_STATE]), (team_ref(5), 4));
}

#[test]
fn release_leaves_a_free_agent_record() {
    let dir = TempDir::new("release-record");
    let mut game = open(&dir);
    let player = club_player(MANAGED, 0);
    game.release(player, 0, false, None).unwrap();
    let r = rec(&game, player);
    assert_eq!((u32_at(r, pdb::R_FROM_CLUB), u32_at(r, pdb::R_SEASON_CLUB)), (pdb::NO_CLUB, pdb::NO_CLUB));
    assert_eq!(wordsn::<4>(r, pdb::R_COMPETITIONS), [0xFFFF; 4]);
    assert_eq!((r[pdb::R_FLAGS], r[pdb::R_STATE]), (0x01, 0));
    assert_eq!(u32_at(r, pdb::R_SALARY), managed_terms(0).0, "last salary stays as his demand");
    assert_eq!(squad_numbers(&game, MANAGED), (1..SQUAD).map(|j| shirt(MANAGED, j)).collect::<Vec<_>>());
    assert!(!my_team(&game).contains(&player.player_id));
}

#[test]
fn player_on_loan_is_not_moved() {
    let mut records = player_records();
    let p = club_player(5, 1);
    records[record_at(p) + pdb::R_FLAGS] |= pdb::FLAG_LOAN;
    let mut game = open_with(&TempDir::new("on-loan"), build_data_with_records(false, &records));
    let (data, recs) = (game.data.clone(), game.records.clone());
    assert!(game.transfer(TransferRequest::new(Source::Club(5), 7, p.player_id)).is_err());
    assert_eq!((game.data == data, game.records == recs), (true, true));
    assert!(game.operations().is_empty());
}

#[test]
fn unreadable_records_lock_moves_but_not_agreed_deals() {
    let mut data = build_data(false);
    data[PLAYERS_AT] ^= 0xFF;
    let mut game = open_with(&TempDir::new("no-records"), data);
    assert!(game.players_error.is_some());
    let before = game.data.clone();
    let seller = club_player(MANAGED, 2);
    assert!(game.transfer(TransferRequest::new(Source::Club(MANAGED), 7, seller.player_id)).is_err());
    assert!(game.release(seller, 0, false, None).is_err());
    assert!(game.renew_contract(seller, 1_000_000, date(2030, 6, 30), None).is_err());
    assert_eq!(game.data, before);
    game.transfer(agreed(7, club_player(7, 2).player_id)).unwrap();
}

#[test]
fn saved_records_are_recompressed_and_tables_found_again() {
    let dir = TempDir::new("save-records");
    let mut game = open(&dir);
    let (seller, renewed) = (club_player(MANAGED, 2), club_player(MANAGED, 4));
    game.transfer(TransferRequest::new(Source::Club(MANAGED), 7, seller.player_id)).unwrap();
    game.renew_contract(renewed, 2_500_000, date(2030, 6, 30), None).unwrap();
    let records = game.records.clone();
    let size_before = game.data.len();
    game.write(None).unwrap();
    assert_ne!(game.data.len(), size_before, "the container was recompressed");
    assert!(game.data.ends_with(&TAIL), "data after the container survives");
    assert_eq!(game.records, records);
    assert_eq!(game.contract_count(), SQUAD - 1, "contract table found again after the save");
    // Another edit on the reloaded state, saved and opened from disk.
    game.renew_contract(renewed, 3_000_000, date(2030, 6, 30), None).unwrap();
    game.write(None).unwrap();
    let mut again = MasterLeague::open(&game.path).unwrap();
    again.set_managed_team(MANAGED).unwrap();
    assert_eq!(again.records, game.records);
    assert_eq!(u32_at(rec(&again, renewed), pdb::R_SALARY), 30_000);
    assert_eq!(again.contract_of(&renewed).unwrap().salary_units, 30_000);
}

#[test]
fn renewal_must_end_after_game_date_and_change_something() {
    let dir = TempDir::new("renew-bad");
    let mut game = open(&dir);
    let player = club_player(MANAGED, 1);
    let before = game.data.clone();
    assert!(game.renew_contract(player, 1_000_000, date(2025, 8, 6), None).is_err());
    let c = game.contract_of(&player).unwrap();
    assert!(game.renew_contract(player, c.annual_eur() as i64, c.end, None).is_err());
    assert!(game.renew_contract(club_player(7, 1), 1_000_000, date(2030, 6, 30), None).is_err(), "AI player");
    assert_eq!(game.data, before);
    assert!(game.operations().is_empty());
}

#[test]
fn release_removes_contract_and_squad_place_and_pays_compensation() {
    let dir = TempDir::new("release");
    let mut game = open(&dir);
    let player = club_player(MANAGED, 2);
    let next = words(&game.data, FIRST_ROW + 3 * RECORD);
    game.release(player, 1_500_000, true, Some("Released")).unwrap();
    assert!(game.contract_of(&player).is_none());
    assert_eq!(words(&game.data, FIRST_ROW + 2 * RECORD), next);
    assert_eq!(words(&game.data, FIRST_ROW + SQUAD * RECORD), BLANK);
    assert_eq!(game.roster(Source::Club(MANAGED)).len(), SQUAD - 1);
    assert!(game.club_of(player.player_id).is_empty(), "no club after release");
    assert_eq!(game.finances().unwrap().transfer_eur, 48_500_000);
}

#[test]
fn release_without_budget_keeps_finances_and_too_large_fee_fails_closed() {
    let dir = TempDir::new("release-budget");
    let mut game = open(&dir);
    let before = game.data.clone();
    assert!(game.release(club_player(MANAGED, 0), 60_000_000, true, None).is_err());
    assert_eq!(game.data, before);
    let fin = game.finances().unwrap();
    game.release(club_player(MANAGED, 0), 60_000_000, false, None).unwrap();
    assert_eq!(game.finances().unwrap(), fin);
}

#[test]
fn months_until_counts_whole_months() {
    let today = date(2025, 8, 6);
    assert_eq!(today.months_until(date(2026, 8, 6)), 12);
    assert_eq!(today.months_until(date(2026, 8, 5)), 11);
    assert_eq!(today.months_until(date(2025, 9, 30)), 1);
    assert_eq!(today.months_until(date(2025, 8, 31)), 0);
    assert_eq!(today.months_until(date(2024, 1, 1)), 0);
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
