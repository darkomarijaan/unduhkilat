//! Pure segment arithmetic (ARCHITECTURE.md §6.1 step 3, §7.1).

/// Split `[0, total_bytes-1]` into contiguous inclusive ranges.
///
/// The effective connection count is `min(connection_count, total_bytes)` so no
/// empty range is produced; `connection_count` has no upper ceiling (DEC-001).
/// Returns an empty vec for `total_bytes <= 0` or `connection_count == 0`.
pub fn allocate_segments(total_bytes: i64, connection_count: u32) -> Vec<(i64, i64)> {
    if total_bytes <= 0 || connection_count == 0 {
        return Vec::new();
    }
    let n = (connection_count as i64).min(total_bytes);
    let chunk = total_bytes / n;
    (0..n)
        .map(|i| {
            let start = i * chunk;
            let end = if i == n - 1 { total_bytes - 1 } else { (i + 1) * chunk - 1 };
            (start, end)
        })
        .collect()
}

/// Range still to fetch for a segment whose next byte is `next` (inclusive), or
/// `None` when the segment is complete. Never re-requests already-stored bytes.
pub fn remaining_range(range_start: i64, range_end: i64, next: i64) -> Option<(i64, i64)> {
    let from = next.max(range_start);
    (from <= range_end).then_some((from, range_end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_partition(total: i64, n: u32) {
        let segs = allocate_segments(total, n);
        assert_eq!(segs.len() as i64, (n as i64).min(total), "total={total} n={n}");
        assert_eq!(segs.first().unwrap().0, 0);
        assert_eq!(segs.last().unwrap().1, total - 1);
        for w in segs.windows(2) {
            assert_eq!(w[1].0, w[0].1 + 1, "contiguous and disjoint");
        }
        for (s, e) in &segs {
            assert!(s <= e, "no empty/negative segment");
        }
        let covered: i64 = segs.iter().map(|(s, e)| e - s + 1).sum();
        assert_eq!(covered, total);
    }

    #[test]
    fn partitions_cover_exactly_for_many_shapes() {
        for total in [1, 2, 3, 7, 8, 9, 100, 1_000_003, 4_294_967_297] {
            for n in [1, 2, 3, 8, 16, 64, 1000] {
                assert_partition(total, n);
            }
        }
    }

    #[test]
    fn connection_count_above_total_is_capped_not_clamped_by_a_ceiling() {
        assert_eq!(allocate_segments(3, 10).len(), 3);
        // DEC-001: 5000 connections on a large file is honoured exactly.
        assert_eq!(allocate_segments(10_000_000, 5000).len(), 5000);
    }

    #[test]
    fn degenerate_inputs() {
        assert!(allocate_segments(0, 8).is_empty());
        assert!(allocate_segments(-1, 8).is_empty());
        assert!(allocate_segments(10, 0).is_empty());
    }

    #[test]
    fn remaining_range_uses_next_byte_semantics() {
        assert_eq!(remaining_range(100, 199, 100), Some((100, 199)), "untouched");
        assert_eq!(remaining_range(100, 199, 150), Some((150, 199)), "next byte, not last received");
        assert_eq!(remaining_range(100, 199, 200), None, "range_end + 1 means done");
    }
}
