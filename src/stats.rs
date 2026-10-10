use chrono::{Days, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const FILE: &str = "stats.toml";
const KEEP_DAYS: u64 = 365;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Stats {
    pub total: u64,
    pub hardest: u8,
    pub hardest_at: String,
    pub best_combo: u32,
    // hits per day keyed by yyyy mm dd
    pub days: BTreeMap<String, u32>,
}

impl Stats {
    pub fn load() -> anyhow::Result<Self> {
        crate::config::read(FILE)
    }

    pub fn save(&self) -> anyhow::Result<()> {
        crate::config::write(FILE, self)
    }

    // returns true when this was the hardest hit so far
    pub fn record(&mut self, power: u8, combo: u32) -> bool {
        self.record_on(Local::now().date_naive(), power, combo)
    }

    fn record_on(&mut self, day: NaiveDate, power: u8, combo: u32) -> bool {
        self.total += 1;
        *self.days.entry(key(day)).or_default() += 1;
        self.best_combo = self.best_combo.max(combo);

        // iso dates sort as text so old days are the smallest keys
        if let Some(oldest) = day.checked_sub_days(Days::new(KEEP_DAYS)) {
            let oldest = key(oldest);
            self.days.retain(|d, _| *d >= oldest);
        }

        let record = power > self.hardest;
        if record {
            self.hardest = power;
            self.hardest_at = Local::now().format("%Y-%m-%d %H:%M").to_string();
        }
        record
    }

    pub fn today(&self) -> u32 {
        self.days
            .get(&key(Local::now().date_naive()))
            .copied()
            .unwrap_or(0)
    }

    // oldest first and including days with no hits
    pub fn last_days(&self, n: u64) -> Vec<(NaiveDate, u32)> {
        let today = Local::now().date_naive();
        (0..n)
            .rev()
            .filter_map(|back| today.checked_sub_days(Days::new(back)))
            .map(|day| (day, self.days.get(&key(day)).copied().unwrap_or(0)))
            .collect()
    }
}

fn key(day: NaiveDate) -> String {
    day.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_hits_and_records() {
        let mut stats = Stats::default();
        let day = NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        assert!(stats.record_on(day, 40, 1));
        assert!(!stats.record_on(day, 30, 3));
        assert!(stats.record_on(day, 90, 2));
        assert_eq!(stats.total, 3);
        assert_eq!(stats.hardest, 90);
        assert_eq!(stats.best_combo, 3);
        assert_eq!(stats.days["2026-10-09"], 3);
    }

    #[test]
    fn prunes_old_days() {
        let mut stats = Stats::default();
        stats.days.insert("2020-01-01".into(), 5);
        stats.record_on(NaiveDate::from_ymd_opt(2026, 10, 9).unwrap(), 10, 1);
        assert!(!stats.days.contains_key("2020-01-01"));
    }
}
