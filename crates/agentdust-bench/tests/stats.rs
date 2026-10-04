use agentdust_bench::stats::{LatencySummary, Spread, percentile};

fn one_to_hundred() -> Vec<u64> {
    (1..=100).collect()
}

#[test]
fn percentiles_use_the_nearest_rank() {
    let values = one_to_hundred();
    assert_eq!(percentile(&values, 50.0), Some(50));
    assert_eq!(percentile(&values, 95.0), Some(95));
    assert_eq!(percentile(&values, 99.0), Some(99));
    assert_eq!(percentile(&values, 100.0), Some(100));
}

#[test]
fn a_rank_between_two_values_rounds_up() {
    let values = [10, 20, 30, 40];
    assert_eq!(percentile(&values, 50.0), Some(20));
    assert_eq!(percentile(&values, 51.0), Some(30));
    assert_eq!(percentile(&values, 99.0), Some(40));
}

#[test]
fn the_zeroth_percentile_is_the_minimum() {
    assert_eq!(percentile(&[7, 8, 9], 0.0), Some(7));
}

#[test]
fn a_single_sample_is_every_percentile() {
    for p in [0.0, 50.0, 99.0, 100.0] {
        assert_eq!(percentile(&[42], p), Some(42));
    }
}

#[test]
fn no_samples_have_no_percentile() {
    assert_eq!(percentile(&[], 50.0), None);
}

#[test]
fn a_summary_sorts_its_samples_first() {
    let mut samples = one_to_hundred();
    samples.reverse();
    let summary = LatencySummary::from_samples(samples).unwrap();
    assert_eq!(summary.count, 100);
    assert_eq!(summary.p50, 50);
    assert_eq!(summary.p95, 95);
    assert_eq!(summary.p99, 99);
    assert_eq!(summary.max, 100);
}

#[test]
fn a_summary_of_nothing_is_none() {
    assert_eq!(LatencySummary::from_samples(Vec::new()), None);
}

#[test]
fn the_spread_of_an_odd_count_has_the_middle_value_as_median() {
    let spread = Spread::of(&[5.0, 1.0, 3.0, 2.0, 4.0]).unwrap();
    assert_eq!((spread.min, spread.median, spread.max), (1.0, 3.0, 5.0));
}

#[test]
fn the_median_of_an_even_count_is_the_mean_of_the_middle_pair() {
    let spread = Spread::of(&[4.0, 1.0, 3.0, 2.0]).unwrap();
    assert_eq!((spread.min, spread.median, spread.max), (1.0, 2.5, 4.0));
}

#[test]
fn the_spread_of_nothing_is_none() {
    assert_eq!(Spread::of(&[]), None);
}
