use crate::projection::ProjectionKind;
use serde::Deserialize;
use std::{error::Error, fs, path::Path, time::Duration};

type ConfigResult<T> = Result<T, Box<dyn Error>>;

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    pub location: LocationConfig,
    pub rendering: RenderingConfig,
    pub refresh: RefreshConfig,
}

impl AppConfig {
    fn validate(&self) -> ConfigResult<()> {
        if !(-90.0..=90.0).contains(&self.location.latitude) {
            return Err("location.latitude must be between -90 and 90".into());
        }
        if let Some(longitude) = self.location.longitude
            && !(-180.0..=180.0).contains(&longitude)
        {
            return Err("location.longitude must be between -180 and 180".into());
        }
        if self.refresh.reload_check_interval_ms == 0 {
            return Err("refresh.reload_check_interval_ms must be greater than 0".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LocationConfig {
    pub latitude: f64,
    pub longitude: Option<f64>,
}

impl Default for LocationConfig {
    fn default() -> Self {
        Self {
            latitude: 35.0,
            longitude: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RenderingConfig {
    pub projection: ProjectionKind,
    pub force_sdr: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct RefreshConfig {
    pub reload_check_interval_ms: u64,
}

impl Default for RefreshConfig {
    fn default() -> Self {
        Self {
            reload_check_interval_ms: 500,
        }
    }
}

impl RefreshConfig {
    pub fn reload_check_interval(self) -> Duration {
        Duration::from_millis(self.reload_check_interval_ms)
    }
}

pub fn load_config(path: &Path) -> ConfigResult<AppConfig> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read config {}: {error}", path.display()))?;
    let config: AppConfig = toml::from_str(&source)
        .map_err(|error| format!("failed to parse config {}: {error}", path.display()))?;
    config
        .validate()
        .map_err(|error| format!("invalid config {}: {error}", path.display()))?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert_eq!(config, AppConfig::default());
    }

    #[test]
    fn complete_config_is_deserialized() {
        let config: AppConfig = toml::from_str(
            r#"
[location]
latitude = 31.2304
longitude = 121.4737

[rendering]
projection = "equirectangular"
force_sdr = true

[refresh]
reload_check_interval_ms = 250
"#,
        )
        .unwrap();

        assert_eq!(config.location.latitude, 31.2304);
        assert_eq!(config.location.longitude, Some(121.4737));
        assert_eq!(config.rendering.projection, ProjectionKind::Equirectangular);
        assert!(config.rendering.force_sdr);
        assert_eq!(config.refresh.reload_check_interval_ms, 250);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_coordinates_and_reload_interval_are_rejected() {
        let mut config = AppConfig::default();
        config.location.latitude = 90.1;
        assert!(config.validate().is_err());
        config.location.latitude = 35.0;
        config.location.longitude = Some(-180.1);
        assert!(config.validate().is_err());
        config.location.longitude = None;
        config.refresh.reload_check_interval_ms = 0;
        assert!(config.validate().is_err());
    }
}
