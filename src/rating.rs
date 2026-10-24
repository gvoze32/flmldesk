//! PES 2021 / Football Life ability decoding and overall rating.
//! Port of `pes21_rating.rating` (pes21-player-rating, MIT).

pub const POSITION_COUNT: usize = 13;
const ABILITY_COUNT: usize = 20;
const GOALKEEPING_COUNT: usize = 5;
const REGISTERED_GRADE: usize = 3;
const MIN_RATING: i64 = 40;

pub const POSITION_NAMES: [&str; POSITION_COUNT] =
    ["GK", "CB", "LB", "RB", "DMF", "CMF", "LMF", "RMF", "AMF", "LWF", "RWF", "SS", "CF"];

const ABILITY_WEIGHTS: [[f64; ABILITY_COUNT]; POSITION_COUNT] = [
    [0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.11, 0.11, 0., 0., 0., 0., 0.],
    [0., 0., 0., 0., 0., 0., 0., 0.22, 0., 0., 0.1, 0., 0., 0.19, 0.19, 0., 0.09, 0.25, 0.17, 0.08],
    [0.06, 0.09, 0.09, 0.05, 0., 0.14, 0., 0., 0., 0., 0.16, 0.14, 0., 0.11, 0.08, 0.04, 0.14, 0.14, 0.08, 0.05],
    [0.06, 0.09, 0.09, 0.05, 0., 0.14, 0., 0., 0., 0., 0.16, 0.14, 0., 0.11, 0.08, 0.04, 0.14, 0.14, 0.08, 0.05],
    [0.07, 0.18, 0.1, 0.05, 0.18, 0.19, 0., 0., 0., 0.12, 0.03, 0.03, 0., 0.05, 0.09, 0.04, 0.14, 0.07, 0.03, 0.01],
    [0.05, 0.23, 0.16, 0.08, 0.23, 0.21, 0., 0., 0., 0., 0.04, 0.06, 0., 0., 0.03, 0.02, 0.17, 0.03, 0., 0.],
    [0.07, 0.15, 0.17, 0.08, 0.07, 0.12, 0., 0., 0., 0.04, 0.24, 0.21, 0., 0., 0., 0., 0.13, 0., 0., 0.],
    [0.07, 0.15, 0.17, 0.08, 0.07, 0.12, 0., 0., 0., 0.04, 0.24, 0.21, 0., 0., 0., 0., 0.13, 0., 0., 0.],
    [0.14, 0.23, 0.17, 0.07, 0.21, 0.14, 0.17, 0., 0., 0., 0.05, 0.07, 0., 0., 0.02, 0.03, 0.03, 0., 0., 0.],
    [0.17, 0.19, 0.15, 0.07, 0.05, 0.09, 0.11, 0., 0., 0., 0.15, 0.15, 0.05, 0., 0.02, 0.04, 0.06, 0., 0., 0.],
    [0.17, 0.19, 0.15, 0.07, 0.05, 0.09, 0.11, 0., 0., 0., 0.15, 0.15, 0.05, 0., 0.02, 0.04, 0.06, 0., 0., 0.],
    [0.15, 0.19, 0.13, 0.06, 0.09, 0.09, 0.14, 0., 0., 0., 0.09, 0.21, 0.06, 0., 0.02, 0.05, 0.04, 0., 0., 0.],
    [0.31, 0.23, 0.09, 0.05, 0., 0., 0.36, 0.03, 0., 0., 0.05, 0.05, 0.03, 0.03, 0.08, 0.02, 0., 0., 0., 0.],
];
const WEAK_FOOT_ACCURACY_WEIGHTS: [f64; POSITION_COUNT] =
    [0., 0.23, 0.56, 0.56, 0.23, 0.23, 0.56, 0.56, 0.37, 0.47, 0.47, 0.37, 0.38];
/// Bonus by playable-position grade C/B/A, then the registered-position bonus.
const POSITION_BONUS: [[i64; 4]; POSITION_COUNT] = [
    [0, 3, 5, 8], [0, 2, 4, 8], [0, 2, 4, 9], [0, 2, 4, 9], [0, 3, 5, 9], [0, 3, 5, 8], [0, 3, 5, 8],
    [0, 3, 5, 8], [0, 3, 5, 8], [0, 2, 4, 10], [0, 2, 4, 10], [0, 3, 5, 9], [0, 3, 5, 8],
];
/// Registered outfield ratings blend 40% of this curve, indexed by the
/// rounded 22-stat average minus 40.
const AVERAGE_CURVE: [i64; 60] = [
    31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 56, 59, 62,
    65, 67, 70, 73, 75, 77, 79, 82, 83, 85, 87, 89, 90, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102,
    103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113,
];

#[derive(Clone, Debug)]
pub struct PlayerAbilities {
    pub registered_position: usize,
    pub position_grades: [usize; POSITION_COUNT],
    pub abilities: [i64; ABILITY_COUNT],
    pub goalkeeping: [i64; GOALKEEPING_COUNT],
    pub weak_foot_accuracy: i64,
}

impl PlayerAbilities {
    /// In-game overall rating at the registered position.
    pub fn overall(&self) -> i64 {
        self.position_rating(self.registered_position)
    }

    pub fn position_rating(&self, position: usize) -> i64 {
        let gk = &self.goalkeeping;
        let reach_awareness = (gk[0] + gk[4]).div_euclid(2);
        let handling = (gk[1] + gk[2] + gk[3]).div_euclid(3);
        let mut weighted = 0.0f64;
        if position == 0 {
            weighted += (reach_awareness - 25) as f64 * 0.5 + (handling - 25) as f64 * 0.48;
        }
        for (value, weight) in self.abilities.iter().zip(ABILITY_WEIGHTS[position]) {
            weighted += (value - 25) as f64 * weight;
        }
        weighted += (self.weak_foot_accuracy - 1) as f64 * WEAK_FOOT_ACCURACY_WEIGHTS[position];
        weighted += 0.5;
        let bonus = POSITION_BONUS[position];
        if position != self.registered_position {
            let grade = self.position_grades[position];
            return (weighted.trunc() as i64 + bonus[grade]).max(MIN_RATING);
        }
        let mut rating = weighted.trunc() as i64 + bonus[REGISTERED_GRADE];
        if position != 0 {
            let total = reach_awareness + handling + self.abilities.iter().sum::<i64>();
            let average = ((total as f64 / 22.0 + 0.5).trunc() as i64).max(MIN_RATING);
            rating = (rating as f64 * 0.6 + AVERAGE_CURVE[(average - MIN_RATING) as usize] as f64 * 0.4 + 0.5)
                .trunc() as i64;
        }
        rating.max(MIN_RATING)
    }
}

/// (byte offset, bit shift, bit count, value added), read from a little-endian 16-bit window.
type Field = (usize, u32, u32, i64);

struct Layout {
    registered_position: Field,
    position_grades: [Field; POSITION_COUNT],
    abilities: [Field; ABILITY_COUNT],
    goalkeeping: [Field; GOALKEEPING_COUNT],
    weak_foot_accuracy: Field,
}

const EDIT_LAYOUT: Layout = Layout {
    registered_position: (0x21, 5, 4, 0),
    position_grades: [
        (0x29, 5, 2, 0), (0x29, 7, 2, 0), (0x2A, 1, 2, 0), (0x2A, 3, 2, 0), (0x2A, 5, 2, 0),
        (0x2A, 7, 2, 0), (0x2B, 1, 2, 0), (0x2B, 3, 2, 0), (0x2B, 5, 2, 0), (0x2E, 4, 2, 0),
        (0x2C, 0, 2, 0), (0x2C, 2, 2, 0), (0x2C, 4, 2, 0),
    ],
    abilities: [
        (0x0E, 0, 7, 0), (0x0E, 7, 7, 0), (0x28, 0, 7, 0), (0x10, 0, 7, 0), (0x10, 7, 7, 0),
        (0x11, 6, 7, 0), (0x12, 5, 7, 0), (0x25, 6, 7, 0), (0x14, 0, 7, 0), (0x14, 7, 7, 0),
        (0x15, 6, 7, 0), (0x16, 5, 7, 0), (0x2D, 5, 7, 0), (0x18, 0, 7, 0), (0x18, 7, 7, 0),
        (0x19, 6, 7, 0), (0x1A, 5, 7, 0), (0x24, 0, 7, 0), (0x1C, 0, 7, 0), (0x1C, 7, 7, 0),
    ],
    goalkeeping: [(0x1D, 6, 7, 0), (0x1E, 5, 7, 0), (0x24, 7, 7, 0), (0x2C, 6, 7, 0), (0x20, 0, 7, 0)],
    weak_foot_accuracy: (0x27, 6, 2, 1),
};

const PLAYERBIN_LAYOUT: Layout = Layout {
    registered_position: (0x36, 2, 4, 0),
    position_grades: [
        (0x2B, 6, 2, 0), (0x3A, 4, 2, 0), (0x27, 6, 2, 0), (0x3B, 2, 2, 0), (0x33, 6, 2, 0),
        (0x39, 0, 2, 0), (0x3A, 2, 2, 0), (0x39, 4, 2, 0), (0x3A, 0, 2, 0), (0x3B, 0, 2, 0),
        (0x3B, 4, 2, 0), (0x3B, 6, 2, 0), (0x3A, 6, 2, 0),
    ],
    abilities: [
        (0x2E, 2, 6, 40), (0x23, 1, 6, 40), (0x2C, 0, 6, 40), (0x34, 0, 6, 40), (0x20, 7, 6, 40),
        (0x32, 2, 6, 40), (0x31, 4, 6, 40), (0x24, 0, 6, 40), (0x1F, 2, 6, 40), (0x29, 4, 6, 40),
        (0x26, 2, 6, 40), (0x2B, 0, 6, 40), (0x2C, 6, 6, 40), (0x24, 6, 6, 40), (0x30, 6, 6, 40),
        (0x2F, 0, 6, 40), (0x2A, 2, 6, 40), (0x22, 3, 6, 40), (0x27, 0, 6, 40), (0x30, 0, 6, 40),
    ],
    goalkeeping: [(0x28, 6, 6, 40), (0x2D, 4, 6, 40), (0x21, 5, 6, 40), (0x28, 0, 6, 40), (0x25, 4, 6, 40)],
    weak_foot_accuracy: (0x39, 6, 2, 1),
};

fn read_field(data: &[u8], base: usize, (offset, shift, bits, add): Field) -> i64 {
    let window = u16::from_le_bytes([data[base + offset], data[base + offset + 1]]) as u32;
    ((window >> shift) & ((1 << bits) - 1)) as i64 + add
}

fn decode(data: &[u8], base: usize, layout: &Layout) -> Option<PlayerAbilities> {
    let registered_position = read_field(data, base, layout.registered_position) as usize;
    if registered_position >= POSITION_COUNT {
        return None;
    }
    Some(PlayerAbilities {
        registered_position,
        position_grades: layout.position_grades.map(|f| read_field(data, base, f) as usize),
        abilities: layout.abilities.map(|f| read_field(data, base, f)),
        goalkeeping: layout.goalkeeping.map(|f| read_field(data, base, f)),
        weak_foot_accuracy: read_field(data, base, layout.weak_foot_accuracy),
    })
}

/// Decode an EDIT `data.dat` player entry; `None` when the position is invalid.
pub fn decode_edit_player(data: &[u8], entry_offset: usize) -> Option<PlayerAbilities> {
    decode(data, entry_offset, &EDIT_LAYOUT)
}

/// Decode a `Player.bin` record; `None` when the position is invalid.
pub fn decode_playerbin_record(data: &[u8], record_offset: usize) -> Option<PlayerAbilities> {
    decode(data, record_offset, &PLAYERBIN_LAYOUT)
}
