use chrono::{DateTime, FixedOffset};
use std::f64::consts::TAU;

const UNIX_EPOCH_JULIAN_DATE: f64 = 2_440_587.5;
const J2000_JULIAN_DATE: f64 = 2_451_545.0;
const DAYS_PER_JULIAN_CENTURY: f64 = 36_525.0;
const ARCSECONDS_TO_RADIANS: f64 = std::f64::consts::PI / (180.0 * 3_600.0);

fn days_since_j2000(time: &DateTime<FixedOffset>) -> f64 {
    let unix_days = time.timestamp_millis() as f64 / 86_400_000.0;
    UNIX_EPOCH_JULIAN_DATE + unix_days - J2000_JULIAN_DATE
}

/// Greenwich mean sidereal time, shifted eastward to the observer's longitude.
///
/// This is sufficient for rendering J2000 catalogue positions. Nutation, precession and
/// stellar proper motion are intentionally omitted.
pub fn local_sidereal_time(time: &DateTime<FixedOffset>, longitude_deg: f64) -> f32 {
    let gmst_deg = 280.460_618_37 + 360.985_647_366_29 * days_since_j2000(time);
    ((gmst_deg + longitude_deg).to_radians().rem_euclid(TAU)) as f32
}

/// IAU 1976 precession angles from J2000.0 to the target date.
///
/// The catalogue stays immutable; the shader applies these three angles to every star.
pub fn precession_angles(time: &DateTime<FixedOffset>) -> [f32; 3] {
    let centuries = days_since_j2000(time) / DAYS_PER_JULIAN_CENTURY;
    let centuries_squared = centuries * centuries;
    let centuries_cubed = centuries_squared * centuries;
    let zeta = 2_306.218_1 * centuries + 0.301_88 * centuries_squared + 0.017_998 * centuries_cubed;
    let z = 2_306.218_1 * centuries + 1.094_68 * centuries_squared + 0.018_203 * centuries_cubed;
    let theta =
        2_004.310_9 * centuries - 0.426_65 * centuries_squared - 0.041_833 * centuries_cubed;
    [zeta, z, theta].map(|angle| (angle * ARCSECONDS_TO_RADIANS) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn j2000_greenwich_sidereal_time_matches_reference_angle() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let time = utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap();

        let angle = local_sidereal_time(&time, 0.0).to_degrees();

        assert!((angle - 280.460_62).abs() < 0.000_1, "{angle}");
    }

    #[test]
    fn longitude_shifts_local_sidereal_time_eastward() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let time = utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap();
        let greenwich = local_sidereal_time(&time, 0.0);
        let east = local_sidereal_time(&time, 30.0);

        assert!(((east - greenwich).to_degrees() - 30.0).abs() < 0.000_1);
    }

    #[test]
    fn precession_is_zero_at_j2000_and_advances_afterward() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let j2000 = utc.with_ymd_and_hms(2000, 1, 1, 12, 0, 0).unwrap();
        let future = utc.with_ymd_and_hms(2050, 1, 1, 12, 0, 0).unwrap();

        assert_eq!(precession_angles(&j2000), [0.0; 3]);
        let [zeta, z, theta] = precession_angles(&future).map(f32::to_degrees);
        assert!((zeta - 0.320_3).abs() < 0.001);
        assert!((z - 0.320_4).abs() < 0.001);
        assert!((theta - 0.278_3).abs() < 0.001);
    }
}
