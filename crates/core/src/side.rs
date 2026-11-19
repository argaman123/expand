//! Which side one computer sits on, seen from the other.

use serde::{Deserialize, Serialize};

/// Where the other computer sits relative to this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_sides_pair_up() {
        for s in [Side::Left, Side::Right, Side::Top, Side::Bottom] {
            assert_eq!(s.opposite().opposite(), s);
            assert_ne!(s.opposite(), s);
        }
    }
}
