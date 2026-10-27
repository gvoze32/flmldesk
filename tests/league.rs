mod common;

use common::*;
use fl_ml_desk::masterleague::MasterLeague;

const MATCHES_AT: usize = 0x20_0000;
const LEAGUE_AT: usize = 0x30_0000;
const COMPETITION: u32 = 0x11;
const MATCH_RECORD: usize = 596;
/// Clubs A (managed), B, C, D.
const CLUBS4: [usize; 4] = [MANAGED, 1, 2, 3];
/// (home, away, round, score); `None` is still to be played.
type Match = (usize, usize, u32, Option<(u8, u8)>);
/// A lost 0–1 at home to B and 1–3 away at D.
const FIXTURES: [Match; 6] = [
    (0, 1, 0, Some((0, 1))),
    (2, 3, 0, Some((2, 2))),
    (3, 0, 1, Some((3, 1))),
    (1, 2, 1, Some((1, 0))),
    (0, 2, 2, None),
    (1, 3, 2, None),
];
/// D 3–1 A.
const AWAY_LOSS: usize = 2;

/// Table rows as the game stores them: club, position, points | won << 8 |
/// lost << 14 | drawn << 20 | away wins << 26, goals for | against << 12 |
/// played << 24, away goals.
fn table_before() -> [[u32; 5]; 4] {
    [
        [team_ref(1), 1, 0x0400_0206, 0x0200_0002, 1],
        [team_ref(3), 2, 0x0010_0104, 0x0200_3005, 2],
        [team_ref(2), 3, 0x0010_4001, 0x0200_3002, 0],
        [team_ref(0), 4, 0x0000_8000, 0x0200_4001, 1],
    ]
}

fn league_save(dir: &TempDir, table: [[u32; 5]; 4]) -> MasterLeague {
    let mut data = build_data(false);
    for (k, (home, away, round, score)) in FIXTURES.iter().enumerate() {
        let state = if score.is_some() { 6 } else { 2 };
        let (h, a) = score.unwrap_or((0, 0));
        let when = date(2025, 8, 1 + 7 * *round as u8).pack();
        let words = [k as u32, state << 28 | round << 16 | COMPETITION, when, 0, 0x0F, team_ref(*home), team_ref(*away), h as u32 | (a as u32) << 24];
        put_words(&mut data, MATCHES_AT + k * MATCH_RECORD, &words);
    }
    put_words(&mut data, LEAGUE_AT, &[2025 << 16 | COMPETITION, COMPETITION, 1, 1, 0xFFFF]);
    for j in 0..48 {
        let club = CLUBS4.get(j).map_or(u32::MAX, |c| team_ref(*c));
        put_words(&mut data, LEAGUE_AT + 0x14 + 16 * j, &[club, u32::MAX, 0, 0x2B]);
    }
    for (j, row) in table.iter().enumerate() {
        put_words(&mut data, LEAGUE_AT + 0x318 + 20 * j, row);
    }
    let path = write_save(&dir.0.join("ML00000001"), data);
    let mut game = MasterLeague::open(&path).unwrap();
    game.set_managed_team(MANAGED).unwrap();
    game
}

fn table(game: &MasterLeague) -> Vec<[u32; 5]> {
    (0..4).map(|j| core::array::from_fn(|i| u32_at(&game.data, LEAGUE_AT + 0x318 + 20 * j + 4 * i))).collect()
}

fn score_word(game: &MasterLeague, k: usize) -> u32 {
    u32_at(&game.data, MATCHES_AT + k * MATCH_RECORD + 28)
}

#[test]
fn away_loss_becomes_a_win_and_the_table_is_recalculated() {
    let dir = TempDir::new("league_win");
    let mut game = league_save(&dir, table_before());
    let view = game.league().unwrap();
    assert_eq!(view.fixtures.iter().map(|f| f.index).collect::<Vec<_>>(), [0, 2, 4]);
    assert_eq!(view.fixtures[1].goals_of(team_ref(MANAGED)), (1, 3));

    game.set_league_win(AWAY_LOSS, 4, 3).unwrap();
    // D 3–4 A: the home goals stay in byte 0, A's goals go to byte 3.
    assert_eq!(score_word(&game, AWAY_LOSS), 3 | 4 << 24);
    // A: won 1 (away), lost 1, 4:4, 3 pts, 4 away goals. D: drawn 1, lost 1,
    // 5:6. D and C end level on points and goal difference; D scored more.
    let after = [
        [team_ref(1), 1, 0x0400_0206, 0x0200_0002, 1],
        [team_ref(0), 2, 0x0400_4103, 0x0200_4004, 4],
        [team_ref(3), 3, 0x0010_4001, 0x0200_6005, 2],
        [team_ref(2), 4, 0x0010_4001, 0x0200_3002, 0],
    ];
    assert_eq!(table(&game), after);

    let (path, _) = game.write(None).unwrap();
    let reopened = {
        let mut g = MasterLeague::open(&path).unwrap();
        g.set_managed_team(MANAGED).unwrap();
        g
    };
    let view = reopened.league().unwrap();
    assert_eq!(view.fixtures[1].goals_of(team_ref(MANAGED)), (4, 3));
    assert_eq!(view.standings.iter().map(|s| (s.club, s.position)).collect::<Vec<_>>(), after.map(|r| (r[0], r[1])));
}

#[test]
fn only_a_played_match_with_a_winning_score_is_accepted() {
    let dir = TempDir::new("league_refuse");
    let mut game = league_save(&dir, table_before());
    game.league().unwrap();
    let before = game.data.clone();
    assert!(game.set_league_win(AWAY_LOSS, 3, 3).is_err());
    assert!(game.set_league_win(AWAY_LOSS, 1, 3).is_err());
    assert!(game.set_league_win(4, 2, 0).is_err(), "not played yet");
    assert!(game.set_league_win(1, 3, 0).is_err(), "not your club's match");
    assert!(game.history.is_empty());
    assert_eq!(game.data, before);
}

#[test]
fn table_that_does_not_add_up_is_left_alone() {
    let dir = TempDir::new("league_deduction");
    let mut deducted = table_before();
    deducted[0][2] = 0x0400_0205;
    let mut game = league_save(&dir, deducted);
    game.league().unwrap();
    let before = game.data.clone();
    assert!(game.set_league_win(AWAY_LOSS, 4, 3).is_err());
    assert!(game.history.is_empty());
    assert_eq!(game.data, before);
}
