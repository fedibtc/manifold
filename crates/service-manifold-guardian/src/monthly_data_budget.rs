//! The operator's answer about this MG's monthly network data
//! (`crates/mg/specs/REQ-seat-capacity-default.md#data-budget`).

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// How much network data a month the operator lets this MG's seats use, in
/// GB (10^9 bytes) per 30 days.
///
/// Every wire carries it as one token: `not_set`, `unlimited`, or the GB as
/// a decimal number, such as `"500"`. The admin API, the database and the
/// `seat capacity` telemetry event all use this token, so telemetry tells a
/// host without a cap from one whose operator has not answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MonthlyDataBudget {
    /// The operator has not answered: an MG set up before the question
    /// existed. No data limit applies.
    NotSet,
    /// The operator said the host's internet has no monthly data cap.
    Unlimited,
    /// The operator's monthly data for this MG.
    Gb(NonZeroU32),
}

impl MonthlyDataBudget {
    /// The GB that limit sales, or `None` when no data limit applies.
    pub fn gb(self) -> Option<NonZeroU32> {
        match self {
            Self::NotSet | Self::Unlimited => None,
            Self::Gb(gb) => Some(gb),
        }
    }
}

const NOT_SET: &str = "not_set";
const UNLIMITED: &str = "unlimited";

impl fmt::Display for MonthlyDataBudget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSet => f.write_str(NOT_SET),
            Self::Unlimited => f.write_str(UNLIMITED),
            Self::Gb(gb) => write!(f, "{gb}"),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("a monthly data budget is `not_set`, `unlimited`, or a whole number of GB from 1")]
pub struct InvalidMonthlyDataBudget;

impl FromStr for MonthlyDataBudget {
    type Err = InvalidMonthlyDataBudget;

    fn from_str(token: &str) -> Result<Self, Self::Err> {
        match token {
            NOT_SET => Ok(Self::NotSet),
            UNLIMITED => Ok(Self::Unlimited),
            // Only the canonical decimal form, so each budget has one token.
            gb if gb.bytes().all(|b| b.is_ascii_digit()) && !gb.starts_with('0') => gb
                .parse()
                .map(Self::Gb)
                .map_err(|_| InvalidMonthlyDataBudget),
            _ => Err(InvalidMonthlyDataBudget),
        }
    }
}

impl Serialize for MonthlyDataBudget {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MonthlyDataBudget {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let token = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        token.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_budget_has_one_token() {
        let gb = |n| MonthlyDataBudget::Gb(NonZeroU32::new(n).unwrap());
        for (budget, token) in [
            (MonthlyDataBudget::NotSet, "not_set"),
            (MonthlyDataBudget::Unlimited, "unlimited"),
            (gb(1), "1"),
            (gb(179), "179"),
            (gb(u32::MAX), "4294967295"),
        ] {
            assert_eq!(budget.to_string(), token);
            assert_eq!(token.parse(), Ok(budget));
            let json = serde_json::to_string(&budget).unwrap();
            assert_eq!(json, format!("\"{token}\""));
            assert_eq!(
                serde_json::from_str::<MonthlyDataBudget>(&json).unwrap(),
                budget
            );
        }
        assert_eq!(gb(179).gb(), NonZeroU32::new(179));
        assert_eq!(MonthlyDataBudget::Unlimited.gb(), None);
        assert_eq!(MonthlyDataBudget::NotSet.gb(), None);
    }

    #[test]
    fn zero_and_non_canonical_numbers_are_not_budgets() {
        // Zero would read as "sell nothing", which the operator says by
        // stopping sales, not by a data budget.
        for token in [
            "0",
            "00",
            "0179",
            "+5",
            "-1",
            " 5",
            "5 ",
            "1.5",
            "4294967296",
            "",
            "none",
            "Unlimited",
        ] {
            assert_eq!(
                token.parse::<MonthlyDataBudget>(),
                Err(InvalidMonthlyDataBudget),
                "{token:?}"
            );
        }
        // A JSON number is not the token.
        assert!(serde_json::from_str::<MonthlyDataBudget>("179").is_err());
    }
}
