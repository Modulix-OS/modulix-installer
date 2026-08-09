use crate::config::SwapMode;

const GIB: u64 = 1024 * 1024 * 1024;

/// Swap size for the chosen mode. `Hibernation` must be able to hold a full
/// RAM image (plus the encryption caveat in CLAUDE.md: it must then live
/// inside the LUKS container).
pub fn compute_swap_bytes(mode: SwapMode, ram_bytes: u64) -> u64 {
    match mode {
        SwapMode::None => 0,
        SwapMode::Standard => standard_swap_bytes(ram_bytes),
        SwapMode::Hibernation => ram_bytes,
    }
}

/// Classic rule of thumb: double small amounts of RAM, match mid-range RAM,
/// flatten out for large-RAM machines.
fn standard_swap_bytes(ram_bytes: u64) -> u64 {
    if ram_bytes <= 2 * GIB {
        ram_bytes * 2
    } else if ram_bytes <= 8 * GIB {
        ram_bytes
    } else {
        4 * GIB
    }
}

/// Parses `MemTotal:    16309420 kB` out of `/proc/meminfo` content.
pub fn parse_mem_total_bytes(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

pub async fn detect_ram_bytes() -> crate::mx::Result<u64> {
    let content = tokio::fs::read_to_string("/proc/meminfo").await?;
    parse_mem_total_bytes(&content)
        .ok_or_else(|| crate::mx::Error::Backend("MemTotal not found in /proc/meminfo".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_swap_for_none_mode() {
        assert_eq!(compute_swap_bytes(SwapMode::None, 16 * GIB), 0);
    }

    #[test]
    fn standard_swap_doubles_small_ram() {
        assert_eq!(compute_swap_bytes(SwapMode::Standard, 1 * GIB), 2 * GIB);
    }

    #[test]
    fn standard_swap_matches_mid_ram() {
        assert_eq!(compute_swap_bytes(SwapMode::Standard, 8 * GIB), 8 * GIB);
    }

    #[test]
    fn standard_swap_flattens_for_large_ram() {
        assert_eq!(compute_swap_bytes(SwapMode::Standard, 32 * GIB), 4 * GIB);
    }

    #[test]
    fn hibernation_swap_matches_ram_exactly() {
        assert_eq!(
            compute_swap_bytes(SwapMode::Hibernation, 16 * GIB),
            16 * GIB
        );
    }

    #[test]
    fn parses_mem_total() {
        let meminfo = "MemTotal:       16309420 kB\nMemFree:         1234 kB\n";
        assert_eq!(parse_mem_total_bytes(meminfo), Some(16_309_420 * 1024));
    }

    #[test]
    fn missing_mem_total_is_none() {
        assert_eq!(parse_mem_total_bytes("MemFree: 1234 kB\n"), None);
    }
}
