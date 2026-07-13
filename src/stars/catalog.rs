use include_dir::{Dir, include_dir};
use std::{error::Error, io};

static STAR_DATA: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/data/stars");

pub struct CatalogStar {
    pub right_ascension: f32,
    pub declination: f32,
    pub magnitude: f32,
    pub color: [f32; 3],
}

pub fn load_embedded_catalog() -> Result<Vec<CatalogStar>, Box<dyn Error>> {
    let file = STAR_DATA.get_file("visible.csv").ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, "embedded star catalogue missing")
    })?;
    let text = file
        .contents_utf8()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "star catalogue is not UTF-8"))?;
    parse_catalog(text)
}

fn parse_catalog(text: &str) -> Result<Vec<CatalogStar>, Box<dyn Error>> {
    let mut stars = Vec::new();
    for (index, line) in text.lines().skip(1).enumerate() {
        let values: Vec<_> = line.split(',').collect();
        if values.len() != 4 {
            return Err(format!("invalid star catalogue row {}", index + 2).into());
        }
        let right_ascension = values[0].parse()?;
        let declination = values[1].parse()?;
        let magnitude = values[2].parse()?;
        let color_index = values[3].parse()?;
        stars.push(CatalogStar {
            right_ascension,
            declination,
            magnitude,
            color: bv_to_linear_rgb(color_index),
        });
    }
    Ok(stars)
}

fn bv_to_linear_rgb(bv: f32) -> [f32; 3] {
    let bv = bv.clamp(-0.4, 2.0);
    let temperature = 4_600.0 * (1.0 / (0.92 * bv + 1.7) + 1.0 / (0.92 * bv + 0.62));
    let t = temperature / 100.0;
    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_73 * (t - 60.0).powf(-0.133_204_76)
    };
    let green = if t <= 66.0 {
        99.470_8 * t.ln() - 161.119_57
    } else {
        288.122_16 * (t - 60.0).powf(-0.075_514_846)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_73 * (t - 10.0).ln() - 305.044_8
    };
    [red, green, blue].map(|component| srgb_to_linear((component / 255.0).clamp(0.0, 1.0)))
}

fn srgb_to_linear(component: f32) -> f32 {
    if component <= 0.040_45 {
        component / 12.92
    } else {
        ((component + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_contains_the_visible_sky_without_the_sun() {
        let stars = load_embedded_catalog().unwrap();

        assert_eq!(stars.len(), 8_920);
        assert!(stars.iter().all(|star| star.magnitude <= 6.5));
        assert!(stars.iter().all(|star| star.magnitude > -2.0));
    }

    #[test]
    fn color_index_orders_blue_and_red_stars() {
        let blue = bv_to_linear_rgb(-0.3);
        let red = bv_to_linear_rgb(1.8);

        assert!(blue[2] > red[2]);
        assert!(red[0] > blue[0]);
    }
}
