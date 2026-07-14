use super::{AppConfig, load_config};
use std::{error::Error, fs, path::PathBuf, time::Instant};

type ConfigResult<T> = Result<T, Box<dyn Error>>;

pub struct RuntimeConfig {
    path: PathBuf,
    config: AppConfig,
    last_seen_bytes: Vec<u8>,
    next_check_at: Instant,
}

impl RuntimeConfig {
    pub fn load(path: PathBuf) -> ConfigResult<Self> {
        let last_seen_bytes = fs::read(&path)
            .map_err(|error| format!("failed to read config {}: {error}", path.display()))?;
        let config = load_config(&path)?;
        let next_check_at = Instant::now() + config.refresh.reload_check_interval();
        Ok(Self {
            path,
            config,
            last_seen_bytes,
            next_check_at,
        })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn path(&self) -> &PathBuf {
        &self.path
    }

    pub fn next_check_at(&self) -> Instant {
        self.next_check_at
    }

    pub fn refresh_if_changed(&mut self, now: Instant) -> ConfigResult<Option<AppConfig>> {
        if now < self.next_check_at {
            return Ok(None);
        }
        self.next_check_at = now + self.config.refresh.reload_check_interval();

        let candidate = fs::read(&self.path)
            .map_err(|error| format!("failed to read config {}: {error}", self.path.display()))?;
        if candidate == self.last_seen_bytes {
            return Ok(None);
        }
        // Remember invalid or partially-written content as well. A later completed save changes the
        // bytes again and retries; meanwhile the running application keeps its valid configuration.
        self.last_seen_bytes = candidate;

        let next = load_config(&self.path)?;
        if next == self.config {
            return Ok(None);
        }
        Ok(Some(next))
    }

    pub fn apply(&mut self, config: AppConfig, now: Instant) {
        self.config = config;
        self.next_check_at = now + self.config.refresh.reload_check_interval();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};

    fn config_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "scatter-config-{name}-{}-{}.toml",
            std::process::id(),
            thread::current().name().unwrap_or("unnamed")
        ))
    }

    #[test]
    fn changed_valid_config_is_offered_for_application() {
        let path = config_path("valid-reload");
        fs::write(&path, "[refresh]\nreload_check_interval_ms = 1\n").unwrap();
        let mut runtime = RuntimeConfig::load(path.clone()).unwrap();
        fs::write(
            &path,
            "[rendering]\nprojection = \"equirectangular\"\n[refresh]\nreload_check_interval_ms = 2\n",
        )
        .unwrap();

        let next = runtime
            .refresh_if_changed(Instant::now() + Duration::from_millis(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            next.rendering.projection,
            crate::projection::ProjectionKind::Equirectangular
        );
        assert_eq!(
            runtime.config().rendering.projection,
            crate::projection::ProjectionKind::Perspective
        );

        runtime.apply(next, Instant::now());
        assert_eq!(
            runtime.config().rendering.projection,
            crate::projection::ProjectionKind::Equirectangular
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_update_is_not_reparsed_until_bytes_change_again() {
        let path = config_path("invalid-reload");
        fs::write(&path, "[refresh]\nreload_check_interval_ms = 1\n").unwrap();
        let mut runtime = RuntimeConfig::load(path.clone()).unwrap();
        fs::write(&path, "this is not toml = [").unwrap();

        assert!(
            runtime
                .refresh_if_changed(Instant::now() + Duration::from_millis(2))
                .is_err()
        );
        assert!(
            runtime
                .refresh_if_changed(Instant::now() + Duration::from_millis(4))
                .unwrap()
                .is_none()
        );
        let _ = fs::remove_file(path);
    }
}
