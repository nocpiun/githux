//! Validated, bounded contribution-calendar presentation.
//!
//! The host chooses and validates the requested interval. This module only
//! consumes that interval and the returned observations; it never calculates a
//! default year or treats service colour strings as CSS.

use crate::domain::{ContributionCalendar, ContributionRange};

/// A known GitHub level has a stable theme-independent ramp index. Unknown
/// levels deliberately have no ramp index and no inferred contribution count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellLevel {
    Known(u8),
    Unknown,
}

impl CellLevel {
    fn from_service(level: &str) -> Self {
        match level {
            "NONE" => Self::Known(0),
            "FIRST_QUARTILE" => Self::Known(1),
            "SECOND_QUARTILE" => Self::Known(2),
            "THIRD_QUARTILE" => Self::Known(3),
            "FOURTH_QUARTILE" => Self::Known(4),
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DayPresentation {
    Observed {
        date: String,
        count: u64,
        level: CellLevel,
    },
    /// A validated date in the requested interval for which no day was
    /// returned. This is not a zero-contribution observation.
    Missing { date: String },
    /// A returned day failed date/interval validation. Its count is not shown
    /// as an observation of another date.
    Invalid { date: String },
}

impl DayPresentation {
    pub fn date(&self) -> &str {
        match self {
            Self::Observed { date, .. } | Self::Missing { date } | Self::Invalid { date } => date,
        }
    }

    pub fn level(&self) -> Option<CellLevel> {
        match self {
            Self::Observed { level, .. } => Some(*level),
            _ => None,
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Self::Observed {
                date,
                count,
                level: CellLevel::Known(_),
            } => format!(
                "{date}: {count} contribution{}",
                if *count == 1 { "" } else { "s" }
            ),
            Self::Observed {
                date,
                count,
                level: CellLevel::Unknown,
            } => format!(
                "{date}: {count} contribution{}; level unavailable",
                if *count == 1 { "" } else { "s" }
            ),
            Self::Missing { date } => format!("{date}: contribution data missing"),
            Self::Invalid { date } => format!("{date}: invalid contribution day"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WeekPresentation {
    pub days: Vec<DayPresentation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeatmapPresentation {
    pub weeks: Vec<WeekPresentation>,
    pub observed_days: usize,
    pub missing_days: usize,
    pub invalid_days: usize,
    pub ignored_out_of_range_days: usize,
    pub total_contributions: u64,
    pub has_field_errors: bool,
}

/// Number of days in an already validated inclusive range.
fn interval_days(range: &ContributionRange) -> i64 {
    day_number(range.to()) - day_number(range.from()) + 1
}

fn day_number(date: &str) -> i64 {
    // `ContributionRange` is the sole constructor of the interval, so its
    // private validation has already established the exact ISO shape.
    let year: i64 = date[0..4].parse().expect("validated year");
    let month: i64 = date[5..7].parse().expect("validated month");
    let day: i64 = date[8..10].parse().expect("validated day");
    let shifted_year = if month <= 2 { year - 1 } else { year };
    let era = if shifted_year >= 0 {
        shifted_year
    } else {
        shifted_year - 399
    } / 400;
    let year_of_era = shifted_year - era * 400;
    let month_position = (month + 9) % 12;
    let day_of_year = (153 * month_position + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn month_days(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => unreachable!("validated month"),
    }
}

fn next_date(date: &str) -> String {
    let mut year: i64 = date[0..4].parse().expect("validated year");
    let mut month: i64 = date[5..7].parse().expect("validated month");
    let mut day: i64 = date[8..10].parse().expect("validated day");
    day += 1;
    if day > month_days(year, month) {
        day = 1;
        month += 1;
        if month > 12 {
            month = 1;
            year += 1;
        }
    }
    format!("{year:04}-{month:02}-{day:02}")
}

/// Construct cells in explicit interval order, never in whatever order the
/// service returned. At most 366 cells are produced by the validated range.
pub fn heatmap_presentation(
    range: &ContributionRange,
    calendar: &ContributionCalendar,
) -> HeatmapPresentation {
    let from = day_number(range.from());
    let to = day_number(range.to());
    let mut date = range.from().to_string();
    let mut days = Vec::with_capacity(interval_days(range) as usize);
    for offset in 0..interval_days(range) {
        days.push((from + offset, DayPresentation::Missing { date }));
        date = next_date(days.last().expect("just inserted").1.date());
    }
    let mut invalid_days = 0;
    let mut ignored_out_of_range_days = 0;

    for week in &calendar.weeks {
        for day in &week.days {
            let valid_shape = day.date.len() == 10
                && day.date.as_bytes().get(4) == Some(&b'-')
                && day.date.as_bytes().get(7) == Some(&b'-')
                && day
                    .date
                    .bytes()
                    .enumerate()
                    .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit());
            let number = valid_shape.then(|| day_number(&day.date));
            let Some(number) = number.filter(|number| (from..=to).contains(number)) else {
                if valid_shape {
                    ignored_out_of_range_days += 1;
                } else {
                    invalid_days += 1;
                }
                continue;
            };
            let position = (number - from) as usize;
            let expected = days[position].1.date().to_string();
            if !matches!(days[position].1, DayPresentation::Missing { .. }) {
                invalid_days += 1;
                continue;
            }
            // Round-trip through the same civil calendar to reject 2023-02-29
            // and other shapes that are numeric but not real dates.
            if day_number(&expected) != number || !same_civil_date(&day.date, &expected) {
                invalid_days += 1;
                continue;
            }
            let slot = &mut days[position].1;
            *slot = DayPresentation::Observed {
                date: day.date.clone(),
                count: day.count,
                level: CellLevel::from_service(&day.level),
            };
        }
    }

    let missing_days = days
        .iter()
        .filter(|(_, day)| matches!(day, DayPresentation::Missing { .. }))
        .count();
    let observed_days = days.len() - missing_days;
    let weeks = days
        .chunks(7)
        .map(|chunk| WeekPresentation {
            days: chunk.iter().map(|(_, day)| day.clone()).collect(),
        })
        .collect();
    HeatmapPresentation {
        weeks,
        observed_days,
        missing_days,
        invalid_days,
        ignored_out_of_range_days,
        total_contributions: calendar.total_contributions,
        has_field_errors: !calendar.errors.is_empty(),
    }
}

fn same_civil_date(actual: &str, expected: &str) -> bool {
    actual == expected
}

/// Tests construct valid domain ranges, including leap/year boundaries. These
/// pure helpers keep date arithmetic independent from GPUI layout.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ContributionDay, ContributionWeek};

    fn calendar(dates: &[(&str, u64, &str)]) -> ContributionCalendar {
        ContributionCalendar {
            weeks: vec![ContributionWeek {
                days: dates
                    .iter()
                    .map(|(date, count, level)| ContributionDay {
                        date: (*date).into(),
                        count: *count,
                        level: (*level).into(),
                        color: "service-colour-is-not-css".into(),
                    })
                    .collect(),
            }],
            months: vec![],
            total_contributions: dates.iter().map(|day| day.1).sum(),
            errors: vec![],
        }
    }

    #[test]
    fn leap_and_year_boundaries_are_real_interval_days() {
        let range = ContributionRange::new("2023-12-31", "2024-03-01").unwrap();
        let view = heatmap_presentation(
            &range,
            &calendar(&[
                ("2023-12-31", 0, "NONE"),
                ("2024-02-29", 2, "FIRST_QUARTILE"),
                ("2024-03-01", 1, "SECOND_QUARTILE"),
            ]),
        );
        assert_eq!(view.observed_days, 3);
        assert_eq!(
            view.weeks.iter().map(|week| week.days.len()).sum::<usize>(),
            62
        );
        assert_eq!(view.missing_days, 59);
        assert!(
            view.weeks
                .iter()
                .flat_map(|week| &week.days)
                .any(|day| day.date() == "2024-02-29")
        );
        assert_eq!(
            view.weeks.last().unwrap().days.last().unwrap().date(),
            "2024-03-01"
        );
    }

    #[test]
    fn missing_zero_unknown_and_invalid_are_distinct() {
        let range = ContributionRange::new("2024-01-01", "2024-01-04").unwrap();
        let view = heatmap_presentation(
            &range,
            &calendar(&[
                ("2024-01-01", 0, "NONE"),
                ("2024-01-02", 0, "NONE"),
                ("2024-01-03", 8, "FUTURE_LEVEL"),
                ("2024-02-30", 99, "FOURTH_QUARTILE"),
                ("2024-01-04", 1, "FIRST_QUARTILE"),
            ]),
        );
        let days = view.weeks[0].days.as_slice();
        assert!(matches!(days[0].level(), Some(CellLevel::Known(0))));
        assert!(matches!(days[2].level(), Some(CellLevel::Unknown)));
        assert_eq!(
            days[2].detail(),
            "2024-01-03: 8 contributions; level unavailable"
        );
        assert_eq!(days[1].detail(), "2024-01-02: 0 contributions");
        assert_eq!(view.invalid_days, 0);
        assert_eq!(view.ignored_out_of_range_days, 1);
        assert_eq!(view.observed_days, 4);
        assert_eq!(view.missing_days, 0);
    }

    #[test]
    fn missing_is_not_zero_and_duplicates_do_not_replace_an_observation() {
        let range = ContributionRange::new("2024-01-01", "2024-01-02").unwrap();
        let view = heatmap_presentation(
            &range,
            &calendar(&[
                ("2024-01-01", 1, "FIRST_QUARTILE"),
                ("2024-01-01", 900, "FOURTH_QUARTILE"),
            ]),
        );
        assert_eq!(view.invalid_days, 1);
        assert_eq!(view.missing_days, 1);
        assert_eq!(view.weeks[0].days[0].detail(), "2024-01-01: 1 contribution");
        assert_eq!(
            view.weeks[0].days[1].detail(),
            "2024-01-02: contribution data missing"
        );
    }

    #[test]
    fn out_of_range_days_are_not_folded_into_the_graph() {
        let range = ContributionRange::new("2024-01-01", "2024-01-02").unwrap();
        let view =
            heatmap_presentation(&range, &calendar(&[("2023-01-01", 30, "FOURTH_QUARTILE")]));
        assert_eq!(view.ignored_out_of_range_days, 1);
        assert_eq!(view.observed_days, 0);
        assert_eq!(view.missing_days, 2);
    }
}
