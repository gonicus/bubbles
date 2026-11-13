use std::ffi::OsStr;
use std::fs;
use std::path::Path;

use gio::prelude::*;
use serde::Serialize;

// Keeps the whole list far below the host's MAX_RESPONSE_BYTES.
const MAX_ICON_BYTES: u64 = 64 * 1024;

const ICON_SIZES: [&str; 5] = ["scalable", "256x256", "128x128", "64x64", "48x48"];

#[derive(Serialize)]
pub struct App {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub wm_class: String,
    /// Base64.
    pub icon: Option<String>,
}

#[derive(Serialize)]
pub struct AppList {
    pub apps: Vec<App>,
}

// Only the formats the host's gdk-pixbuf and the portal both read.
fn read_icon(path: &Path) -> Option<Vec<u8>> {
    if !matches!(path.extension().and_then(OsStr::to_str)?, "svg" | "png") {
        return None;
    }
    let size = fs::metadata(path).ok()?.len();
    if size == 0 || size > MAX_ICON_BYTES {
        return None;
    }
    fs::read(path).ok()
}

// The icon theme lookup needs a display, so hicolor and pixmaps by hand.
fn find_themed_icon(name: &str) -> Option<Vec<u8>> {
    let hicolor = ICON_SIZES.iter().flat_map(|size| {
        ["svg", "png"].map(|extension| format!("icons/hicolor/{size}/apps/{name}.{extension}"))
    });
    // Where applications shipped icons before the icon theme spec.
    let pixmaps = ["png", "svg"].map(|extension| format!("pixmaps/{name}.{extension}"));
    let candidates: Vec<String> = hicolor.chain(pixmaps).collect();
    std::iter::once(gio::glib::user_data_dir())
        .chain(gio::glib::system_data_dirs())
        .find_map(|root| candidates.iter().find_map(|path| read_icon(&root.join(path))))
}

fn icon_bytes(icon: &gio::Icon) -> Option<Vec<u8>> {
    if let Some(file_icon) = icon.downcast_ref::<gio::FileIcon>() {
        return read_icon(&file_icon.file().path()?);
    }
    // GDesktopAppInfo has already dropped any .png or .svg suffix.
    icon.downcast_ref::<gio::ThemedIcon>()?
        .names()
        .iter()
        .find_map(|name| find_themed_icon(name))
}

pub fn list_apps() -> AppList {
    let apps = gio::AppInfo::all()
        .into_iter()
        // NoDisplay, OnlyShowIn and NotShowIn; Hidden entries are never listed.
        .filter(|info| info.should_show())
        .filter_map(|info| {
            let wm_class = info
                .downcast_ref::<gio::DesktopAppInfo>()
                .and_then(|desktop| desktop.startup_wm_class())
                .map(String::from)
                .unwrap_or_default();
            Some(App {
                id: info.id()?.into(),
                name: info.name().into(),
                comment: info.description().map(String::from).unwrap_or_default(),
                wm_class,
                icon: info
                    .icon()
                    .as_ref()
                    .and_then(icon_bytes)
                    .map(|bytes| gio::glib::base64_encode(&bytes).into()),
            })
        })
        .collect();
    AppList { apps }
}
