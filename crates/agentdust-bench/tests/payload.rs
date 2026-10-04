use agentdust_bench::payload::{identify, line_len, record_with};

const SIZES: [usize; 2] = [150, 4000];

#[test]
fn an_encoded_record_has_exactly_the_requested_size() {
    for size in SIZES {
        for writer in [0, 2, 15, 99] {
            for seq in [0, 9, 123_456] {
                let record = record_with(writer, seq, size, 1_800_000_000_000, 987_654_321_012_345);
                assert_eq!(line_len(&record), size, "writer {writer} seq {seq} size {size}");
            }
        }
    }
}

#[test]
fn the_size_holds_when_the_monotonic_stamp_changes_width() {
    for mono_ts in [7, 1_234_567, 98_765_432_101_234_567] {
        let record = record_with(3, 4, 4000, 1_800_000_000_000, mono_ts);
        assert_eq!(line_len(&record), 4000, "mono {mono_ts}");
    }
}

#[test]
fn a_record_identifies_its_writer_and_sequence() {
    for size in SIZES {
        let record = record_with(7, 4242, size, 1_800_000_000_000, 55);
        assert_eq!(identify(&record, size), Some((7, 4242)));
    }
}

#[test]
fn records_of_different_writers_and_sequences_differ_in_content() {
    let base = record_with(1, 1, 4000, 10, 10);
    assert_ne!(base, record_with(2, 1, 4000, 10, 10));
    assert_ne!(base, record_with(1, 2, 4000, 10, 10));
    assert_ne!(
        base.session_id[6..],
        record_with(2, 1, 4000, 10, 10).session_id[6..]
    );
    assert_ne!(
        base.session_id[6..],
        record_with(1, 2, 4000, 10, 10).session_id[6..]
    );
}

#[test]
fn a_record_with_a_mixed_in_padding_is_not_identified() {
    let mut record = record_with(1, 1, 4000, 10, 10);
    let other = record_with(2, 5, 4000, 10, 10);
    let half = record.session_id.len() / 2;
    record.session_id.replace_range(half.., &other.session_id[half..]);
    assert_eq!(identify(&record, 4000), None);
}

#[test]
fn a_truncated_padding_is_not_identified() {
    let mut record = record_with(1, 1, 4000, 10, 10);
    record.session_id.truncate(100);
    assert_eq!(identify(&record, 4000), None);
}

#[test]
fn a_record_checked_against_another_size_is_not_identified() {
    let record = record_with(1, 1, 4000, 10, 10);
    assert_eq!(identify(&record, 150), None);
}

#[test]
fn a_foreign_session_id_is_not_identified() {
    let mut record = record_with(1, 1, 150, 10, 10);
    record.session_id = "s-1".to_owned();
    assert_eq!(identify(&record, 150), None);
}

#[test]
fn a_size_below_the_base_record_gives_the_base_record() {
    let record = record_with(1, 1, 10, 10, 10);
    assert!(line_len(&record) <= 150);
    assert_eq!(identify(&record, 10), Some((1, 1)));
}

#[test]
fn the_marker_record_is_named_and_never_mistaken_for_a_writer_record() {
    let marker = agentdust_bench::payload::marker(7, 1_800_000_000_000, 99);
    assert_eq!(marker.session_id, "expired-7");
    assert!(agentdust_bench::payload::is_marker(&marker));
    assert_eq!(identify(&marker, 150), None);
    assert!(!agentdust_bench::payload::is_marker(&record_with(
        1, 1, 150, 10, 10
    )));
}

#[test]
fn the_accessors_read_the_two_timestamps_of_a_record() {
    let record = record_with(1, 2, 150, 1_800_000_000_123, 456);
    assert_eq!(agentdust_bench::payload::wall(&record), 1_800_000_000_123);
    assert_eq!(agentdust_bench::payload::mono(&record), 456);
}
