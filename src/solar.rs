use chrono::{DateTime, Datelike, FixedOffset, Timelike};
use std::f64::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct SolarPosition {
    /// Degrees above the astronomical horizon.
    pub elevation_deg: f64,
    /// Degrees clockwise from true north.
    pub azimuth_deg: f64,
}

impl SolarPosition {
    /// East/up/north direction, matching the coordinate system used by the shader.
    pub fn direction(self) -> [f32; 3] {
        let elevation = self.elevation_deg.to_radians();
        let azimuth = self.azimuth_deg.to_radians();
        let horizontal = elevation.cos();

        [
            (azimuth.sin() * horizontal) as f32,
            elevation.sin() as f32,
            (azimuth.cos() * horizontal) as f32,
        ]
    }
}

/// Approximate apparent solar position using NOAA's fractional-year equations.
///
/// This is accurate enough for sky rendering while avoiding ephemeris tables. Longitude is
/// positive eastward and latitude is positive northward.
pub fn solar_position(
    local_time: &DateTime<FixedOffset>,
    latitude_deg: f64,
    longitude_deg: f64,
) -> SolarPosition {
    let days_in_year = if local_time.date_naive().with_ordinal(366).is_some() {
        366.0
    } else {
        365.0
    };
    let local_hour = local_time.hour() as f64
        + local_time.minute() as f64 / 60.0
        + local_time.second() as f64 / 3600.0;
    let gamma =
        2.0 * PI / days_in_year * (local_time.ordinal() as f64 - 1.0 + (local_hour - 12.0) / 24.0);

    let equation_of_time = 229.18
        * (0.000_075 + 0.001_868 * gamma.cos()
            - 0.032_077 * gamma.sin()
            - 0.014_615 * (2.0 * gamma).cos()
            - 0.040_849 * (2.0 * gamma).sin());
    let declination = 0.006_918 - 0.399_912 * gamma.cos() + 0.070_257 * gamma.sin()
        - 0.006_758 * (2.0 * gamma).cos()
        + 0.000_907 * (2.0 * gamma).sin()
        - 0.002_697 * (3.0 * gamma).cos()
        + 0.001_48 * (3.0 * gamma).sin();

    let utc_offset_minutes = local_time.offset().local_minus_utc() as f64 / 60.0;
    let time_offset = equation_of_time + 4.0 * longitude_deg - utc_offset_minutes;
    let local_minutes = local_hour * 60.0;
    let true_solar_minutes = (local_minutes + time_offset).rem_euclid(1440.0);
    let hour_angle = (true_solar_minutes / 4.0 - 180.0).to_radians();
    let latitude = latitude_deg.to_radians();

    let cos_zenith = (latitude.sin() * declination.sin()
        + latitude.cos() * declination.cos() * hour_angle.cos())
    .clamp(-1.0, 1.0);
    let elevation = PI / 2.0 - cos_zenith.acos();
    let azimuth = (hour_angle
        .sin()
        .atan2(hour_angle.cos() * latitude.sin() - declination.tan() * latitude.cos())
        .to_degrees()
        + 180.0)
        .rem_euclid(360.0);

    SolarPosition {
        elevation_deg: elevation.to_degrees(),
        azimuth_deg: azimuth,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn equinox_noon_is_near_zenith_at_equator() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let time = utc.with_ymd_and_hms(2025, 3, 20, 12, 0, 0).unwrap();
        let sun = solar_position(&time, 0.0, 0.0);

        assert!(sun.elevation_deg > 87.0, "{sun:?}");
    }

    #[test]
    fn morning_sun_is_east_and_evening_sun_is_west() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let morning = utc.with_ymd_and_hms(2025, 3, 20, 6, 0, 0).unwrap();
        let evening = utc.with_ymd_and_hms(2025, 3, 20, 18, 0, 0).unwrap();

        let morning_sun = solar_position(&morning, 0.0, 0.0);
        let evening_sun = solar_position(&evening, 0.0, 0.0);

        assert!((60.0..120.0).contains(&morning_sun.azimuth_deg));
        assert!((240.0..300.0).contains(&evening_sun.azimuth_deg));
    }

    #[test]
    fn direction_is_normalized() {
        let direction = SolarPosition {
            elevation_deg: 23.0,
            azimuth_deg: 217.0,
        }
        .direction();
        let length = direction.iter().map(|v| v * v).sum::<f32>().sqrt();

        assert!((length - 1.0).abs() < 1e-6);
    }
}
