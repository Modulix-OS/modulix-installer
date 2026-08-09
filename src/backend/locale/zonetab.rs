use super::TimezoneEntry;

/// Parses tzdata's `zone.tab`/`zone1970.tab` (`codes\tcoordinates\tTZ[\tcomments]`,
/// `#`-comments and blank lines skipped). Coordinates are ISO 6709
/// (`±DDMM±DDDMM` or `±DDMMSS±DDDMMSS`).
pub fn parse_zone_tab(content: &str) -> Vec<TimezoneEntry> {
    content
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let _codes = fields.next()?;
            let coord = fields.next()?;
            let name = fields.next()?;
            let (latitude, longitude) = parse_iso6709(coord)?;
            Some(TimezoneEntry {
                name: name.to_string(),
                latitude,
                longitude,
            })
        })
        .collect()
}

fn parse_iso6709(coord: &str) -> Option<(f64, f64)> {
    let second_sign_at = coord
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '+' || *c == '-')?
        .0;
    let latitude = parse_component(&coord[..second_sign_at], 2)?;
    let longitude = parse_component(&coord[second_sign_at..], 3)?;
    Some((latitude, longitude))
}

/// `degree_digits` is 2 for latitude (`DD`), 3 for longitude (`DDD`).
fn parse_component(field: &str, degree_digits: usize) -> Option<f64> {
    let sign = if field.starts_with('-') { -1.0 } else { 1.0 };
    let digits = field.get(1..)?;
    let degrees: f64 = digits.get(..degree_digits)?.parse().ok()?;
    let rest = digits.get(degree_digits..)?;
    let minutes: f64 = if rest.len() >= 2 {
        rest[..2].parse().ok()?
    } else {
        0.0
    };
    let seconds: f64 = if rest.len() >= 4 {
        rest[2..4].parse().ok()?
    } else {
        0.0
    };
    Some(sign * (degrees + minutes / 60.0 + seconds / 3600.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minute_precision_coordinates() {
        let entries = parse_zone_tab("FR\t+4852+00220\tEurope/Paris\n");
        assert_eq!(entries.len(), 1);
        let paris = &entries[0];
        assert_eq!(paris.name, "Europe/Paris");
        assert!((paris.latitude - 48.8667).abs() < 0.01);
        assert!((paris.longitude - 2.3333).abs() < 0.01);
    }

    #[test]
    fn parses_second_precision_coordinates() {
        let entries = parse_zone_tab("US\t+404251-0740023\tAmerica/New_York\n");
        assert_eq!(entries.len(), 1);
        let ny = &entries[0];
        assert!((ny.latitude - 40.714_16).abs() < 0.001);
        assert!((ny.longitude - (-74.006_38)).abs() < 0.001);
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let entries = parse_zone_tab("# comment\n\nFR\t+4852+00220\tEurope/Paris\n");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn skips_malformed_lines() {
        let entries = parse_zone_tab("FR\tnot-a-coordinate\tEurope/Paris\n");
        assert!(entries.is_empty());
    }
}
