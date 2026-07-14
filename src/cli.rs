use std::{ffi::OsStr, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum WindowMode {
    #[default]
    Normal,
    Wallpaper,
}

impl WindowMode {
    pub(crate) fn is_wallpaper(self) -> bool {
        self == Self::Wallpaper
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct StartupOptions {
    pub(crate) window_mode: WindowMode,
    pub(crate) config_path: PathBuf,
}

pub(crate) fn parse_startup_options<I, S>(arguments: I) -> Result<Option<StartupOptions>, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut mode = WindowMode::Normal;
    let mut config_path = None;
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_ref().to_str() {
            Some("--wallpaper") => mode = WindowMode::Wallpaper,
            Some("--help" | "-h") => return Ok(None),
            Some("--config") => {
                let path = arguments
                    .next()
                    .ok_or_else(|| "--config requires a path argument".to_owned())?;
                config_path = Some(PathBuf::from(path.as_ref()));
            }
            Some(argument) if argument.starts_with("--config=") => {
                config_path = Some(PathBuf::from(&argument["--config=".len()..]));
            }
            Some(argument) => return Err(format!("unknown argument: {argument}")),
            None => return Err("arguments must be valid UTF-8".into()),
        }
    }
    Ok(Some(StartupOptions {
        window_mode: mode,
        config_path: config_path.unwrap_or_else(|| PathBuf::from("config.toml")),
    }))
}

pub(crate) fn print_usage() {
    println!(
        "Usage: scatter [--wallpaper] [--config PATH]\n\n  --wallpaper    Render behind desktop icons on macOS\n  --config PATH  Load and hot-reload this TOML file (default: config.toml)"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wallpaper_flag_selects_wallpaper_mode() {
        assert_eq!(
            parse_startup_options(["--wallpaper"]),
            Ok(Some(StartupOptions {
                window_mode: WindowMode::Wallpaper,
                config_path: PathBuf::from("config.toml"),
            }))
        );
    }

    #[test]
    fn no_flag_keeps_normal_window_mode() {
        assert_eq!(
            parse_startup_options(std::iter::empty::<&str>()),
            Ok(Some(StartupOptions {
                window_mode: WindowMode::Normal,
                config_path: PathBuf::from("config.toml"),
            }))
        );
    }

    #[test]
    fn help_stops_before_opening_a_window() {
        assert_eq!(parse_startup_options(["--help"]), Ok(None));
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert_eq!(
            parse_startup_options(["--unknown"]),
            Err("unknown argument: --unknown".into())
        );
    }

    #[test]
    fn config_path_accepts_separate_and_joined_forms() {
        assert_eq!(
            parse_startup_options(["--config", "custom.toml"])
                .unwrap()
                .unwrap()
                .config_path,
            PathBuf::from("custom.toml")
        );
        assert_eq!(
            parse_startup_options(["--config=other.toml"])
                .unwrap()
                .unwrap()
                .config_path,
            PathBuf::from("other.toml")
        );
    }
}
