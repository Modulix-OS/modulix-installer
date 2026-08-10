//! Free-space computation over a disk's existing partition layout — pure and
//! backend-independent, so both the planner (`engine::plan`) and its tests
//! use it without a `DiskBackend` at all.

use super::PartitionInfo;

pub const ALIGNMENT: u64 = 1024 * 1024; // 1 MiB
/// Space reserved at the very end of the disk for the GPT secondary header
/// and partition table — never allocate into it.
pub const GPT_TAIL_RESERVE: u64 = 1024 * 1024;
/// Gaps smaller than this aren't worth surfacing as a `FreeSpace` choice.
const MIN_GAP_BYTES: u64 = ALIGNMENT;

pub fn align_up(v: u64) -> u64 {
    v.div_ceil(ALIGNMENT) * ALIGNMENT
}

pub fn align_down(v: u64) -> u64 {
    (v / ALIGNMENT) * ALIGNMENT
}

#[derive(Debug, Clone, PartialEq)]
pub struct FreeGap {
    /// `"{start_bytes}+{size_bytes}"` — stable across calls as long as the
    /// layout doesn't change, which is what `selected_free_space_id` stores.
    pub id: String,
    pub start_bytes: u64,
    pub size_bytes: u64,
}

fn push_gap(gaps: &mut Vec<FreeGap>, raw_start: u64, raw_end: u64) {
    let start = align_up(raw_start);
    let end = align_down(raw_end);
    if end <= start || end - start < MIN_GAP_BYTES {
        return;
    }
    let size = end - start;
    gaps.push(FreeGap {
        id: format!("{start}+{size}"),
        start_bytes: start,
        size_bytes: size,
    });
}

/// Gaps between/around `parts` on a `disk_size`-byte disk, aligned to
/// [`ALIGNMENT`] and stopping short of [`GPT_TAIL_RESERVE`] at the very end.
/// `parts` need not be pre-sorted.
pub fn free_gaps(disk_size: u64, parts: &[PartitionInfo]) -> Vec<FreeGap> {
    let mut sorted: Vec<&PartitionInfo> = parts.iter().collect();
    sorted.sort_by_key(|p| p.start_bytes);

    let mut gaps = Vec::new();
    let mut cursor = 0u64;
    let usable_end = disk_size.saturating_sub(GPT_TAIL_RESERVE);

    for p in &sorted {
        if p.start_bytes > cursor {
            push_gap(&mut gaps, cursor, p.start_bytes);
        }
        cursor = cursor.max(p.start_bytes + p.size_bytes);
    }

    if usable_end > cursor {
        push_gap(&mut gaps, cursor, usable_end);
    }

    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(start: u64, size: u64) -> PartitionInfo {
        PartitionInfo {
            path: "/dev/fake0p".into(),
            disk_path: "/dev/fake0".into(),
            fs_type: None,
            label: None,
            size_bytes: size,
            start_bytes: start,
            type_guid: None,
            uuid: None,
            is_esp: false,
            used_bytes: None,
        }
    }

    const GIB: u64 = 1024 * 1024 * 1024;
    const MIB: u64 = 1024 * 1024;

    #[test]
    fn align_up_rounds_to_next_mib() {
        assert_eq!(align_up(0), 0);
        assert_eq!(align_up(1), MIB);
        assert_eq!(align_up(MIB), MIB);
        assert_eq!(align_up(MIB + 1), 2 * MIB);
    }

    #[test]
    fn align_down_rounds_to_previous_mib() {
        assert_eq!(align_down(0), 0);
        assert_eq!(align_down(MIB - 1), 0);
        assert_eq!(align_down(MIB), MIB);
        assert_eq!(align_down(MIB + 1), MIB);
    }

    #[test]
    fn no_partitions_is_one_gap_minus_gpt_tail_reserve() {
        let gaps = free_gaps(100 * GIB, &[]);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].start_bytes, 0);
        assert_eq!(gaps[0].size_bytes, 100 * GIB - GPT_TAIL_RESERVE);
    }

    #[test]
    fn gap_in_head() {
        let parts = [part(10 * GIB, 10 * GIB)];
        let gaps = free_gaps(100 * GIB, &parts);
        assert_eq!(gaps[0].start_bytes, 0);
        assert_eq!(gaps[0].size_bytes, 10 * GIB);
        // Tail gap present too.
        assert_eq!(gaps.len(), 2);
    }

    #[test]
    fn gap_in_middle() {
        let parts = [part(0, 10 * GIB), part(50 * GIB, 10 * GIB)];
        let gaps = free_gaps(100 * GIB, &parts);
        let middle = gaps
            .iter()
            .find(|g| g.start_bytes == 10 * GIB)
            .expect("middle gap");
        assert_eq!(middle.size_bytes, 40 * GIB);
    }

    #[test]
    fn gap_in_tail() {
        let parts = [part(0, 10 * GIB)];
        let gaps = free_gaps(100 * GIB, &parts);
        assert_eq!(gaps.len(), 1);
        assert_eq!(gaps[0].start_bytes, 10 * GIB);
        assert_eq!(gaps[0].size_bytes, 100 * GIB - 10 * GIB - GPT_TAIL_RESERVE);
    }

    #[test]
    fn no_gap_when_disk_is_full() {
        let parts = [part(0, 100 * GIB - GPT_TAIL_RESERVE)];
        let gaps = free_gaps(100 * GIB, &parts);
        assert!(gaps.is_empty());
    }

    #[test]
    fn sub_alignment_gap_is_dropped() {
        let parts = [part(0, 10 * GIB), part(10 * GIB + 500_000, 10 * GIB)];
        let gaps = free_gaps(20 * GIB + 1_000_000, &parts);
        // The sliver between the two partitions is well under 1 MiB once
        // aligned inward from both sides, and shouldn't be surfaced.
        assert!(gaps.iter().all(
            |g| g.start_bytes >= 10 * GIB + 500_000 || g.start_bytes + g.size_bytes <= 10 * GIB
        ));
    }

    #[test]
    fn unsorted_input_is_handled() {
        let parts = [part(50 * GIB, 10 * GIB), part(0, 10 * GIB)];
        let gaps = free_gaps(100 * GIB, &parts);
        assert!(gaps.iter().any(|g| g.start_bytes == 10 * GIB));
    }
}
