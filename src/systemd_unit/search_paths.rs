
/// see https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#User%20Unit%20Search%20Path
/// and https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Unit%20File%20Load%20Path
/// $XDG_CONFIG_HOME defaults to "$HOME/.config"
/// $XDG_CONFIG_DIRS defaults to "/etc/xdg"
/// $XDG_DATA_DIRS defaults to "/usr/local/share" and "/usr/share"
/// $XDG_DATA_HOME defaults to "$HOME/.local/share"
// TODO: init with `systemd-analyze --user unit-paths`
pub static DEFAULT_USER_SEARCH_PATHS: &[&str] = &[
    "$XDG_CONFIG_HOME/systemd/user.control/",
    "$XDG_RUNTIME_DIR/systemd/user.control/",
    "$XDG_RUNTIME_DIR/systemd/transient/",
    "$XDG_RUNTIME_DIR/systemd/generator.early/",
    "$XDG_CONFIG_HOME/systemd/user/",
    "$XDG_CONFIG_DIRS/systemd/user/",
    "/etc/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/user/",
    "/run/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/generator/",
    "$XDG_DATA_HOME/systemd/user/",
    "$XDG_DATA_DIRS/systemd/user/",
    // ...
    "/usr/local/lib/systemd/user/",
    "/usr/lib/systemd/user/",
    "$XDG_RUNTIME_DIR/systemd/generator.late/",
];

/// see https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#System%20Unit%20Search%20Path
/// and https://www.freedesktop.org/software/systemd/man/latest/systemd.unit.html#Unit%20File%20Load%20Path
// TODO: init with `systemd-analyze --system unit-paths`
pub static DEFAULT_SYSTEM_SEARCH_PATHS: &[&str] = &[
    "/etc/systemd/system.control/",
    "/run/systemd/system.control/",
    "/run/systemd/transient/",
    "/run/systemd/generator.early/",
    "/etc/systemd/system/",
    //"/etc/systemd/system.attached/",
    "/run/systemd/system/",
    //"/run/systemd/system.attached/",
    "/run/systemd/generator/",
    // ...
    "/usr/local/lib/systemd/system/",
    "/usr/lib/systemd/system/",
    "/run/systemd/generator.late/",
];

pub static UNIT_PATH_ENV: &str = "SYSTEMD_UNIT_PATH";

/// Directory for global Systemd units (sysadmin owned)
pub static UNIT_DIR_ADMIN: &str = "/etc/systemd/system";
/// Directory for global Systemd units (distro owned)
pub static UNIT_DIR_DISTRO: &str = "/usr/lib/systemd/system";
/// Directory for temporary Systemd units (sysadmin owned)
pub static UNIT_DIR_TEMP: &str = "/run/systemd/system";
