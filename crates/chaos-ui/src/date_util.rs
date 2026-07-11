//! Shared month-grid math for the dashboard calendar widget and the
//! calendar page: both render 6 fixed Monday-first weeks (42 days), and
//! both shift the shown (year, month) with the same arithmetic.

use chrono::{Datelike, Days, NaiveDate};

/// Days in the fixed 6-week grid.
pub(crate) const GRID_DAYS: u64 = 42;

/// The Monday starting the 6-week grid that shows (year, month).
/// `None` only for dates outside chrono's supported range — unreachable
/// through the UI, where months always come from [`shift_month`].
pub(crate) fn grid_start(year: i32, month: u32) -> Option<NaiveDate> {
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    Some(first - Days::new(first.weekday().num_days_from_monday() as u64))
}

/// The 42 days of the grid, Monday-first, oldest first.
pub(crate) fn month_grid(year: i32, month: u32) -> Option<impl Iterator<Item = NaiveDate>> {
    let start = grid_start(year, month)?;
    Some((0..GRID_DAYS).map(move |i| start + Days::new(i)))
}

/// (year, month) moved by `delta` months, carrying across year boundaries.
pub(crate) fn shift_month(year: i32, month: u32, delta: i32) -> (i32, u32) {
    let total = year * 12 + (month as i32 - 1) + delta;
    (total.div_euclid(12), (total.rem_euclid(12) + 1) as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Weekday;

    #[test]
    fn grid_starts_on_the_monday_before_the_first() {
        // July 2026 starts on a Wednesday; the grid opens Mon June 29.
        let start = grid_start(2026, 7).unwrap();
        assert_eq!(start, NaiveDate::from_ymd_opt(2026, 6, 29).unwrap());
        assert_eq!(start.weekday(), Weekday::Mon);
        // June 2026 starts ON a Monday: no back-fill.
        assert_eq!(
            grid_start(2026, 6).unwrap(),
            NaiveDate::from_ymd_opt(2026, 6, 1).unwrap()
        );
        // Out-of-range months are None, not a panic.
        assert!(grid_start(2026, 13).is_none());
    }

    #[test]
    fn grid_covers_42_days_across_a_year_boundary() {
        // January 2026 starts on a Thursday: the grid runs Mon Dec 29 2025
        // through Sun Feb 8 2026.
        let days: Vec<NaiveDate> = month_grid(2026, 1).unwrap().collect();
        assert_eq!(days.len(), 42);
        assert_eq!(days[0], NaiveDate::from_ymd_opt(2025, 12, 29).unwrap());
        assert_eq!(days[41], NaiveDate::from_ymd_opt(2026, 2, 8).unwrap());
    }

    #[test]
    fn february_grid_handles_short_months() {
        // February 2026 starts on a Sunday: grid opens Mon Jan 26 and the
        // short month still fills all 42 cells, ending Sun Mar 8.
        let days: Vec<NaiveDate> = month_grid(2026, 2).unwrap().collect();
        assert_eq!(days[0], NaiveDate::from_ymd_opt(2026, 1, 26).unwrap());
        assert_eq!(days[41], NaiveDate::from_ymd_opt(2026, 3, 8).unwrap());
    }

    #[test]
    fn shift_month_carries_across_years() {
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2025, 12, 1), (2026, 1));
        assert_eq!(shift_month(2026, 7, 0), (2026, 7));
        assert_eq!(shift_month(2026, 7, -19), (2024, 12));
        assert_eq!(shift_month(2026, 7, 18), (2028, 1));
    }
}
