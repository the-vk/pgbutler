//! Point-based heuristic that recommends indexes for deletion.
//!
//! Every [`Criterion`] inspects the tracked statistics of one index and awards
//! points together with a human-readable reason. The points are summed and an
//! index is recommended once the total reaches [`RECOMMEND_THRESHOLD`]. To add
//! a new rule, implement [`Criterion`] and register it in [`default_criteria`].

use super::IndexReportRow;

/// Minimum total score for an index to be recommended for deletion.
pub const RECOMMEND_THRESHOLD: u32 = 5;

/// Points awarded by a single criterion and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub points: u32,
    pub reason: String,
}

/// One scoring rule of the heuristic.
pub trait Criterion {
    /// Returns `None` when the rule does not apply to the index.
    fn evaluate(&self, index: &IndexReportRow) -> Option<Finding>;
}

/// Total score of an index and the findings that produced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub score: u32,
    pub findings: Vec<Finding>,
    /// Set when the index must not be dropped; it is then never recommended.
    pub exempt: Option<&'static str>,
}

impl Assessment {
    pub fn is_recommended(&self) -> bool {
        self.exempt.is_none() && self.score >= RECOMMEND_THRESHOLD
    }
}

/// An index together with its assessment.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub index: IndexReportRow,
    pub assessment: Assessment,
}

/// Rules used by default.
pub fn default_criteria() -> Vec<Box<dyn Criterion>> {
    vec![
        Box::new(HighReads),
        Box::new(DecliningVelocity),
        Box::new(RecentlyIdle),
    ]
}

/// Why an index must be left alone regardless of its usage: it enforces
/// integrity or backs a foreign key (dropping it slows deletes/updates of the
/// referenced rows), so unused-looking scans say nothing about its value.
pub fn exemption(index: &IndexReportRow) -> Option<&'static str> {
    if index.is_primary {
        Some("primary key")
    } else if index.is_unique {
        Some("unique index")
    } else if index.supports_foreign_key {
        Some("supports a foreign key")
    } else {
        None
    }
}

pub fn assess(index: &IndexReportRow, criteria: &[Box<dyn Criterion>]) -> Assessment {
    if let Some(reason) = exemption(index) {
        return Assessment {
            score: 0,
            findings: Vec::new(),
            exempt: Some(reason),
        };
    }
    let findings: Vec<Finding> = criteria
        .iter()
        .filter_map(|criterion| criterion.evaluate(index))
        .filter(|finding| finding.points > 0)
        .collect();
    Assessment {
        score: findings.iter().map(|finding| finding.points).sum(),
        findings,
        exempt: None,
    }
}

/// Assesses all indexes and returns those reaching the threshold, highest
/// score first; ties keep the input order (public schema first, most scanned first).
pub fn recommend(indexes: &[IndexReportRow], criteria: &[Box<dyn Criterion>]) -> Vec<Candidate> {
    let mut candidates: Vec<Candidate> = indexes
        .iter()
        .map(|index| Candidate {
            index: index.clone(),
            assessment: assess(index, criteria),
        })
        .filter(|candidate| candidate.assessment.is_recommended())
        .collect();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.assessment.score));
    candidates
}

/// Lifetime volume of tuples read: the more an index was relied upon in the
/// past, the more telling its present idleness is (and the more space-worthy
/// it was). Up to 3 points.
pub struct HighReads;

impl Criterion for HighReads {
    fn evaluate(&self, index: &IndexReportRow) -> Option<Finding> {
        let points = match index.idx_tup_read {
            n if n >= 1_000_000 => 3,
            n if n >= 100_000 => 2,
            n if n >= 10_000 => 1,
            _ => return None,
        };
        Some(Finding {
            points,
            reason: format!("{} tuples read in total", index.idx_tup_read),
        })
    }
}

/// Scan velocity that keeps falling from the longest to the shortest window
/// (7d → 24h → 6h → 1h). One point per strict decrease between consecutive
/// available windows, up to 3.
pub struct DecliningVelocity;

impl Criterion for DecliningVelocity {
    fn evaluate(&self, index: &IndexReportRow) -> Option<Finding> {
        // `scans_per_hour` is ordered 1h, 6h, 24h, 7d; walk it from oldest to newest.
        let velocities: Vec<f64> = index
            .scans_per_hour
            .iter()
            .rev()
            .flatten()
            .copied()
            .collect();
        let steps = velocities
            .windows(2)
            .filter(|pair| pair[1] < pair[0])
            .count() as u32;
        (steps > 0).then(|| Finding {
            points: steps,
            reason: format!("scan velocity declining in {steps} step(s) across time windows"),
        })
    }
}

/// Windows without a single scan. One point per idle window (1h, 6h, 24h, 7d), up to 3.
pub struct RecentlyIdle;

impl Criterion for RecentlyIdle {
    fn evaluate(&self, index: &IndexReportRow) -> Option<Finding> {
        let idle = index
            .scans_per_hour
            .iter()
            .flatten()
            .filter(|velocity| **velocity == 0.0)
            .count() as u32;
        (idle > 0).then(|| Finding {
            points: idle.min(3),
            reason: format!("no scans in {idle} time window(s)"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `velocity` is ordered 1h, 6h, 24h, 7d like [`IndexReportRow::scans_per_hour`].
    fn index(tup_read: i64, velocity: [Option<f64>; 4]) -> IndexReportRow {
        IndexReportRow {
            schemaname: "public".into(),
            relname: "orders".into(),
            indexrelname: "orders_idx".into(),
            idx_scan: 1000,
            idx_tup_read: tup_read,
            idx_tup_fetch: 0,
            last_idx_scan: None,
            is_primary: false,
            is_unique: false,
            supports_foreign_key: false,
            scans_per_hour: velocity,
        }
    }

    fn points(criterion: &dyn Criterion, index: &IndexReportRow) -> u32 {
        criterion
            .evaluate(index)
            .map_or(0, |finding| finding.points)
    }

    #[test]
    fn high_reads_tiers() {
        let at = |reads| points(&HighReads, &index(reads, [None; 4]));
        assert_eq!(at(0), 0);
        assert_eq!(at(9_999), 0);
        assert_eq!(at(10_000), 1);
        assert_eq!(at(99_999), 1);
        assert_eq!(at(100_000), 2);
        assert_eq!(at(999_999), 2);
        assert_eq!(at(1_000_000), 3);
        assert_eq!(at(i64::MAX), 3);
    }

    #[test]
    fn declining_velocity_counts_each_step_down() {
        let at = |v| points(&DecliningVelocity, &index(0, v));
        // 7d=100, 24h=50, 6h=10, 1h=1
        assert_eq!(at([Some(1.0), Some(10.0), Some(50.0), Some(100.0)]), 3);
        // 7d=100, 24h=50, 6h=10, 1h=10 (no drop in the last step)
        assert_eq!(at([Some(10.0), Some(10.0), Some(50.0), Some(100.0)]), 2);
        // growing usage
        assert_eq!(at([Some(100.0), Some(50.0), Some(10.0), Some(1.0)]), 0);
        // flat usage
        assert_eq!(at([Some(5.0); 4]), 0);
    }

    #[test]
    fn declining_velocity_skips_missing_windows() {
        let at = |v| points(&DecliningVelocity, &index(0, v));
        // only 1h and 6h known: 6h=10 -> 1h=2
        assert_eq!(at([Some(2.0), Some(10.0), None, None]), 1);
        // gap in the middle is skipped: 7d=9 -> 1h=1
        assert_eq!(at([Some(1.0), None, None, Some(9.0)]), 1);
        // a single known window or none cannot show a trend
        assert_eq!(at([Some(1.0), None, None, None]), 0);
        assert_eq!(at([None; 4]), 0);
    }

    #[test]
    fn recently_idle_counts_zero_windows_capped_at_three() {
        let at = |v| points(&RecentlyIdle, &index(0, v));
        assert_eq!(at([None; 4]), 0);
        assert_eq!(at([Some(0.5), Some(1.0), Some(1.0), Some(1.0)]), 0);
        assert_eq!(at([Some(0.0), Some(1.0), Some(1.0), Some(1.0)]), 1);
        assert_eq!(at([Some(0.0), Some(0.0), None, None]), 2);
        assert_eq!(at([Some(0.0), Some(0.0), Some(0.0), Some(1.0)]), 3);
        assert_eq!(at([Some(0.0); 4]), 3);
    }

    #[test]
    fn findings_explain_the_score() {
        let idx = index(2_000_000, [Some(0.0), Some(1.0), Some(5.0), Some(9.0)]);
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.findings.len(), 3);
        assert_eq!(
            assessment.score,
            assessment.findings.iter().map(|f| f.points).sum::<u32>()
        );
        assert!(assessment.findings.iter().all(|f| !f.reason.is_empty()));
    }

    #[test]
    fn heavily_read_but_abandoned_index_is_recommended() {
        // reads 3 + declining 3 + idle 1 = 7
        let idx = index(5_000_000, [Some(0.0), Some(2.0), Some(20.0), Some(100.0)]);
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.score, 7);
        assert!(assessment.is_recommended());
    }

    #[test]
    fn fully_idle_index_with_big_history_is_recommended() {
        // reads 3 + declining 0 + idle 3 = 6
        let idx = index(1_000_000, [Some(0.0); 4]);
        assert_eq!(assess(&idx, &default_criteria()).score, 6);
    }

    #[test]
    fn steadily_used_index_is_not_recommended() {
        let idx = index(50_000_000, [Some(40.0), Some(40.0), Some(40.0), Some(40.0)]);
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.score, 3);
        assert!(!assessment.is_recommended());
    }

    #[test]
    fn growing_usage_is_not_recommended() {
        let idx = index(
            50_000_000,
            [Some(500.0), Some(100.0), Some(50.0), Some(10.0)],
        );
        assert!(!assess(&idx, &default_criteria()).is_recommended());
    }

    #[test]
    fn index_without_history_scores_nothing() {
        let idx = index(0, [None; 4]);
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.score, 0);
        assert!(assessment.findings.is_empty());
    }

    #[test]
    fn threshold_boundary() {
        // reads 2 + declining 3 = 5 -> recommended
        let at_threshold = index(100_000, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        let assessment = assess(&at_threshold, &default_criteria());
        assert_eq!(assessment.score, RECOMMEND_THRESHOLD);
        assert!(assessment.is_recommended());
        // reads 1 + declining 3 = 4 -> not recommended
        let below = index(10_000, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        let assessment = assess(&below, &default_criteria());
        assert_eq!(assessment.score, RECOMMEND_THRESHOLD - 1);
        assert!(!assessment.is_recommended());
    }

    #[test]
    fn recommend_filters_and_sorts_by_score() {
        let mut medium = index(100_000, [Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
        medium.indexrelname = "medium".into();
        let mut worst = index(5_000_000, [Some(0.0), Some(2.0), Some(20.0), Some(100.0)]);
        worst.indexrelname = "worst".into();
        let mut healthy = index(5_000_000, [Some(9.0); 4]);
        healthy.indexrelname = "healthy".into();

        let result = recommend(&[medium, healthy, worst], &default_criteria());
        let names: Vec<_> = result
            .iter()
            .map(|c| c.index.indexrelname.as_str())
            .collect();
        assert_eq!(names, ["worst", "medium"]);
        assert!(result[0].assessment.score > result[1].assessment.score);
    }

    #[test]
    fn recommend_keeps_input_order_for_equal_scores() {
        let mut first = index(1_000_000, [Some(0.0); 4]);
        first.indexrelname = "first".into();
        let mut second = first.clone();
        second.indexrelname = "second".into();
        let result = recommend(&[first, second], &default_criteria());
        let names: Vec<_> = result
            .iter()
            .map(|c| c.index.indexrelname.as_str())
            .collect();
        assert_eq!(names, ["first", "second"]);
    }

    fn abandoned() -> IndexReportRow {
        index(5_000_000, [Some(0.0), Some(2.0), Some(20.0), Some(100.0)])
    }

    #[test]
    fn primary_key_is_never_recommended() {
        let idx = IndexReportRow {
            is_primary: true,
            ..abandoned()
        };
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.exempt, Some("primary key"));
        assert_eq!(assessment.score, 0);
        assert!(assessment.findings.is_empty());
        assert!(!assessment.is_recommended());
    }

    #[test]
    fn unique_index_is_never_recommended() {
        let idx = IndexReportRow {
            is_unique: true,
            ..abandoned()
        };
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.exempt, Some("unique index"));
        assert!(!assessment.is_recommended());
    }

    #[test]
    fn foreign_key_index_is_never_recommended() {
        let idx = IndexReportRow {
            supports_foreign_key: true,
            ..abandoned()
        };
        let assessment = assess(&idx, &default_criteria());
        assert_eq!(assessment.exempt, Some("supports a foreign key"));
        assert!(!assessment.is_recommended());
    }

    #[test]
    fn plain_index_is_not_exempt() {
        assert_eq!(exemption(&abandoned()), None);
        assert_eq!(assess(&abandoned(), &default_criteria()).exempt, None);
    }

    #[test]
    fn recommend_skips_exempt_indexes() {
        let mut protected = abandoned();
        protected.indexrelname = "protected".into();
        protected.is_unique = true;
        let mut plain = abandoned();
        plain.indexrelname = "plain".into();
        let result = recommend(&[protected, plain], &default_criteria());
        let names: Vec<_> = result
            .iter()
            .map(|c| c.index.indexrelname.as_str())
            .collect();
        assert_eq!(names, ["plain"]);
    }

    #[test]
    fn custom_criteria_can_be_added() {
        struct Always(u32);
        impl Criterion for Always {
            fn evaluate(&self, _: &IndexReportRow) -> Option<Finding> {
                Some(Finding {
                    points: self.0,
                    reason: "always".into(),
                })
            }
        }
        let criteria: Vec<Box<dyn Criterion>> = vec![Box::new(Always(2)), Box::new(Always(3))];
        let idx = index(0, [None; 4]);
        assert_eq!(assess(&idx, &criteria).score, 5);
        assert!(assess(&idx, &[]).findings.is_empty());
    }
}
