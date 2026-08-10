//! ext2/3/4 used-space probe — parses `dumpe2fs -h` output (pure, mirrors how
//! `ntfs.rs` splits parsing from execution). NTFS used-space instead reuses
//! the `ntfsresize --info` pass already run once per disk load (see
//! `ntfs::parse_ntfsresize_info`'s `Space in use` line) rather than adding a
//! second, expensive full-accounting pass here.

/// Parses `dumpe2fs -h <dev>` output into bytes in use: `(Block count - Free
/// blocks) * Block size`. `None` if any of the three fields is missing or
/// unparsable.
pub fn parse_dumpe2fs_usage(stdout: &str) -> Option<u64> {
    let field = |label: &str| -> Option<u64> {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(label))
            .map(str::trim)
            .and_then(|v| v.parse::<u64>().ok())
    };

    let block_count = field("Block count:")?;
    let free_blocks = field("Free blocks:")?;
    let block_size = field("Block size:")?;
    Some(block_count.saturating_sub(free_blocks) * block_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_used_bytes_from_dumpe2fs_output() {
        let stdout = "Filesystem volume name:   <none>\n\
                       Block count:              6553600\n\
                       Reserved GDT blocks:      1024\n\
                       Free blocks:              6000000\n\
                       Block size:               4096\n";
        assert_eq!(
            parse_dumpe2fs_usage(stdout),
            Some((6553600u64 - 6000000) * 4096)
        );
    }

    #[test]
    fn missing_field_is_none() {
        let stdout = "Block count:              6553600\nBlock size:               4096\n";
        assert_eq!(parse_dumpe2fs_usage(stdout), None);
    }

    #[test]
    fn unparsable_field_is_none() {
        let stdout = "Block count:              not-a-number\n\
                       Free blocks:              6000000\n\
                       Block size:               4096\n";
        assert_eq!(parse_dumpe2fs_usage(stdout), None);
    }
}
