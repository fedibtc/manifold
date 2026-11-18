//! Staged rollouts that the setup-payment federation publication schedules
//! for MG features.
//!
//! Contract: `specs/SPEC-setup-payment-federations.md#staged-rollouts`.
//!
//! A rollout gives each feature a schedule: the percentage of seats that run
//! it, as a piecewise-linear function of Unix time. Each seat holds a private
//! uniform [`RolloutDraw`] per feature and runs the feature while its draw is
//! below the schedule's percentage, so raising the percentage only adds seats
//! and lowering it only removes them. The publisher never learns which seats a
//! percentage selects.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::fmt;

/// Most features one publication may schedule.
pub const ROLLOUT_MAX_FEATURES: usize = 32;

/// Most points one feature's schedule may have.
pub const ROLLOUT_MAX_POINTS: usize = 16;

/// Longest feature name, in bytes.
pub const ROLLOUT_FEATURE_NAME_MAX_BYTES: usize = 64;

/// Name of a feature under staged rollout: 1 to 64 bytes of lowercase ASCII
/// letters, digits and `_`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(transparent)]
pub struct RolloutFeature(String);

impl RolloutFeature {
    /// Validate a feature name.
    ///
    /// # Errors
    ///
    /// Returns an error when the name is empty, longer than
    /// [`ROLLOUT_FEATURE_NAME_MAX_BYTES`], or has a byte other than `a-z`,
    /// `0-9` and `_`.
    pub fn new(name: impl Into<String>) -> Result<Self, InvalidRolloutFeature> {
        let name = name.into();
        if (1..=ROLLOUT_FEATURE_NAME_MAX_BYTES).contains(&name.len())
            && name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            Ok(Self(name))
        } else {
            Err(InvalidRolloutFeature)
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RolloutFeature {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for RolloutFeature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(name).map_err(serde::de::Error::custom)
    }
}

/// A feature name outside `[a-z0-9_]{1,64}`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("a rollout feature name must be 1 to 64 bytes of a-z, 0-9 and _")]
pub struct InvalidRolloutFeature;

/// One point of a schedule: at Unix time `at_secs`, `percent` of seats run
/// the feature. On the wire, the array `[at_secs, percent]`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolloutPoint {
    pub at_secs: u64,
    /// From 0 to 100.
    pub percent: u8,
}

impl serde::Serialize for RolloutPoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        (self.at_secs, self.percent).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for RolloutPoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let (at_secs, percent) = <(u64, u8) as serde::Deserialize>::deserialize(deserializer)?;
        if percent > 100 {
            return Err(serde::de::Error::custom(
                "a rollout percentage must be from 0 to 100",
            ));
        }
        Ok(Self { at_secs, percent })
    }
}

/// The share of seats that run one feature over time: 1 to
/// [`ROLLOUT_MAX_POINTS`] points with strictly increasing times. Between two
/// points the percentage moves linearly; before the first point it is the
/// first point's and after the last point the last point's.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct RolloutSchedule(Vec<RolloutPoint>);

impl RolloutSchedule {
    /// Validate a schedule.
    ///
    /// # Errors
    ///
    /// Returns an error for no points, more than [`ROLLOUT_MAX_POINTS`], or
    /// times that do not strictly increase.
    pub fn new(points: Vec<RolloutPoint>) -> Result<Self, InvalidRolloutSchedule> {
        if !(1..=ROLLOUT_MAX_POINTS).contains(&points.len()) {
            return Err(InvalidRolloutSchedule::PointCount);
        }
        if points
            .windows(2)
            .any(|pair| pair[1].at_secs <= pair[0].at_secs)
        {
            return Err(InvalidRolloutSchedule::TimesNotIncreasing);
        }
        Ok(Self(points))
    }

    #[must_use]
    pub fn points(&self) -> &[RolloutPoint] {
        &self.0
    }

    /// The segment that holds `now_secs`: its start point and, unless the
    /// percentage is constant there, its end point.
    fn segment(&self, now_secs: u64) -> (RolloutPoint, Option<RolloutPoint>) {
        let after = self.0.partition_point(|point| point.at_secs <= now_secs);
        match (after.checked_sub(1), self.0.get(after)) {
            (None, _) => (self.0[0], None),
            (Some(before), next) => (self.0[before], next.copied()),
        }
    }

    /// Whether a seat with `draw` runs the feature at Unix time `now_secs`:
    /// whether `draw / 2^32 * 100` is below the percentage then. Exact, so the
    /// seats selected at a percentage are those selected at any higher one.
    #[must_use]
    pub fn includes(&self, draw: RolloutDraw, now_secs: u64) -> bool {
        // draw / 2^32 * 100 < start + (end - start) * elapsed / span, scaled
        // by 2^32 * span. Every term is below 2^104, so i128 is exact.
        let draw_scaled = i128::from(draw.0) * 100;
        let (start, end) = self.segment(now_secs);
        let Some(end) = end else {
            return draw_scaled < (i128::from(start.percent) << 32);
        };
        let span = i128::from(end.at_secs - start.at_secs);
        let elapsed = i128::from(now_secs - start.at_secs);
        let rise = i128::from(end.percent) - i128::from(start.percent);
        draw_scaled * span < (i128::from(start.percent) * span + rise * elapsed) << 32
    }

    /// The percentage at Unix time `now_secs`, for display and for comparing
    /// schedules; [`Self::includes`] decides membership exactly.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn percent_at(&self, now_secs: u64) -> f64 {
        let (start, end) = self.segment(now_secs);
        let Some(end) = end else {
            return f64::from(start.percent);
        };
        let progress = (now_secs - start.at_secs) as f64 / (end.at_secs - start.at_secs) as f64;
        f64::from(start.percent) + (f64::from(end.percent) - f64::from(start.percent)) * progress
    }
}

impl<'de> serde::Deserialize<'de> for RolloutSchedule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let points = <Vec<RolloutPoint> as serde::Deserialize>::deserialize(deserializer)?;
        Self::new(points).map_err(serde::de::Error::custom)
    }
}

/// An invalid [`RolloutSchedule`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum InvalidRolloutSchedule {
    #[error("a rollout schedule must have 1 to 16 points")]
    PointCount,
    #[error("rollout schedule times must strictly increase")]
    TimesNotIncreasing,
}

/// The schedules of every feature under staged rollout, by feature name; at
/// most [`ROLLOUT_MAX_FEATURES`]. A feature without a schedule runs on no
/// seat. On the wire, a JSON object; a repeated name is invalid.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
#[serde(transparent)]
pub struct Rollouts(BTreeMap<RolloutFeature, RolloutSchedule>);

impl Rollouts {
    /// Validate a set of schedules.
    ///
    /// # Errors
    ///
    /// Returns an error for more than [`ROLLOUT_MAX_FEATURES`] features.
    pub fn new(
        schedules: BTreeMap<RolloutFeature, RolloutSchedule>,
    ) -> Result<Self, TooManyRolloutFeatures> {
        if schedules.len() > ROLLOUT_MAX_FEATURES {
            return Err(TooManyRolloutFeatures);
        }
        Ok(Self(schedules))
    }

    #[must_use]
    pub fn get(&self, feature: &RolloutFeature) -> Option<&RolloutSchedule> {
        self.0.get(feature)
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&RolloutFeature, &RolloutSchedule)> {
        self.0.iter()
    }

    /// The percentage of seats that run `feature` at `now_secs`: 0 for a
    /// feature without a schedule.
    #[must_use]
    pub fn percent_at(&self, feature: &RolloutFeature, now_secs: u64) -> f64 {
        self.get(feature)
            .map_or(0.0, |schedule| schedule.percent_at(now_secs))
    }
}

impl<'de> serde::Deserialize<'de> for Rollouts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Rollouts;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of rollout schedules by feature name")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Rollouts, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut schedules = BTreeMap::new();
                while let Some((feature, schedule)) =
                    map.next_entry::<RolloutFeature, RolloutSchedule>()?
                {
                    if schedules.len() == ROLLOUT_MAX_FEATURES {
                        return Err(serde::de::Error::custom(TooManyRolloutFeatures));
                    }
                    if schedules.insert(feature, schedule).is_some() {
                        return Err(serde::de::Error::custom(
                            "a rollout feature is scheduled twice",
                        ));
                    }
                }
                Ok(Rollouts(schedules))
            }
        }

        deserializer.deserialize_map(Visitor)
    }
}

/// More than [`ROLLOUT_MAX_FEATURES`] features in one publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("a publication may schedule at most 32 rollout features")]
pub struct TooManyRolloutFeatures;

/// A seat's private uniform draw for one feature: the seat runs the feature
/// while `draw / 2^32 * 100` is below the schedule's percentage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolloutDraw(u32);

impl RolloutDraw {
    #[must_use]
    pub fn new(bits: u32) -> Self {
        Self(bits)
    }
}
