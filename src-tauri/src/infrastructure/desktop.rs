use std::fs;
use std::path::PathBuf;

/// Icon name used for the menu entry, the notification and the window.
pub const APP_ICON_NAME: &str = "mawaqit-desktop";
const APP_ICON_PNG: &[u8] = include_bytes!("../../icons/icon.png");
/// Tauri sets the GTK application id from the bundle identifier, which is
/// what the desktop environment matches against StartupWMClass.
const WM_CLASS: &str = "com.bilel.mawaqit-desktop";

/// Install the app icon into the user's icon theme and a .desktop entry
/// into ~/.local/share/applications, so the mawaqit icon shows up in the
/// application menu, the dock and the taskbar even when running the binary
/// straight from the workspace (the .deb/AppImage do this system-wide).
/// Idempotent: files are only rewritten when their content changed.
pub fn ensure_menu_entry() {
    let Some(data_dir) = dirs::data_dir() else {
        return;
    };

    let icon_path = data_dir
        .join("icons/hicolor/256x256/apps")
        .join(format!("{APP_ICON_NAME}.png"));
    write_if_changed(&icon_path, APP_ICON_PNG);

    let exec = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    if exec.is_empty() {
        return;
    }
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Mawaqit\n\
         Comment=Prayer times and athan alerts\n\
         Exec={exec}\n\
         Icon={APP_ICON_NAME}\n\
         Terminal=false\n\
         Categories=Utility;\n\
         StartupWMClass={WM_CLASS}\n"
    );
    let desktop_path = data_dir.join("applications").join(format!("{APP_ICON_NAME}.desktop"));
    write_if_changed(&desktop_path, entry.as_bytes());
}

fn write_if_changed(path: &PathBuf, bytes: &[u8]) {
    if fs::read(path).map(|c| c == bytes).unwrap_or(false) {
        return;
    }
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_ok() {
        let _ = fs::write(path, bytes);
    }
}

/// Heal the autostart entry: the autostart plugin rewrites
/// `~/.config/autostart/mawaqit-desktop.desktop` with the running binary's
/// path whenever the setting is toggled, which can leave it pointing at an
/// old build that then fails at login. Whenever an entry exists but points
/// somewhere else, repoint it at this binary. A missing entry means the user
/// turned autostart off — never resurrect it.
pub fn ensure_autostart_entry() {
    let Some(config_dir) = dirs::config_dir() else {
        return;
    };
    let path = config_dir.join("autostart").join(format!("{APP_ICON_NAME}.desktop"));
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let desired = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Version=1.0\n\
         Name={APP_ICON_NAME}\n\
         Comment=mawaqit-desktop startup script\n\
         Exec={} --minimized\n\
         StartupNotify=false\n\
         Terminal=false\n",
        exe.display()
    );

    if let Ok(existing) = fs::read_to_string(&path) {
        if existing != desired {
            let _ = fs::write(&path, desired);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_is_a_real_png() {
        assert_eq!(&APP_ICON_PNG[..8], b"\x89PNG\r\n\x1a\n");
    }
}
