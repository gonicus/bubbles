use gtk::gdk_pixbuf::Pixbuf;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use relm4::adw::prelude::*;
use relm4::factory::{DynamicIndex, FactoryVecDeque};
use relm4::prelude::FactoryComponent;
use relm4::{ComponentParts, ComponentSender, FactorySender, RelmWidgetExt, SimpleComponent};
use serde::Deserialize;

use crate::agent::{agent_request, Agent};

const ICON_SIZE: i32 = 32;

#[derive(Deserialize, Debug, Clone)]
struct GuestIcon {
    data: String,
}

#[derive(Deserialize, Debug, Clone)]
struct GuestApp {
    id: String,
    name: String,
    #[serde(default)]
    comment: String,
    #[serde(default)]
    wm_class: String,
    icon: Option<GuestIcon>,
}

#[derive(Deserialize, Debug, Default)]
pub struct GuestAppList {
    apps: Vec<GuestApp>,
}

// The guest picks the id, and it ends up in the launcher's Exec line and in
// the agent URL. Everything else it sends goes through KeyFile or plain-text
// labels.
fn has_valid_id(app: &GuestApp) -> bool {
    !app.id.is_empty()
        && app
            .id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn icon_bytes(app: &GuestApp) -> Option<Vec<u8>> {
    app.icon.as_ref().map(|icon| glib::base64_decode(&icon.data))
}

// Via gdk-pixbuf, not Texture::from_bytes, which cannot read the SVGs most
// icon themes ship.
fn row_image(app: &GuestApp) -> gtk::Image {
    let texture = icon_bytes(app).and_then(|bytes| {
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
        let pixbuf =
            Pixbuf::from_stream_at_scale(&stream, ICON_SIZE, ICON_SIZE, true, gio::Cancellable::NONE)
                .ok()?;
        Some(gtk::gdk::Texture::for_pixbuf(&pixbuf))
    });
    let image = match texture {
        Some(texture) => gtk::Image::from_paintable(Some(&texture)),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(ICON_SIZE);
    image
}

#[derive(Debug)]
struct RowInit {
    app: GuestApp,
    exported: bool,
}

#[derive(Debug)]
struct AppRow {
    app: GuestApp,
    image: gtk::Image,
    exported: bool,
    busy: bool,
}

#[derive(Debug)]
enum AppRowOutput {
    Launch(DynamicIndex),
    ToggleMenu(DynamicIndex),
}

#[relm4::factory]
impl FactoryComponent for AppRow {
    type Init = RowInit;
    type Input = ();
    type Output = AppRowOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        #[root]
        relm4::adw::ActionRow {
            // Applies to the subtitle too; both come from the guest.
            set_use_markup: false,
            set_title: &self.app.name,
            set_subtitle: &self.app.comment,
            add_prefix: &self.image,
            set_activatable: true,
            connect_activated[sender, index] => move |_| {
                sender.output(AppRowOutput::Launch(index.clone())).unwrap();
            },
            add_suffix = &relm4::adw::Spinner {
                #[watch]
                set_visible: self.busy,
            },
            add_suffix = &gtk::Button {
                set_valign: gtk::Align::Center,
                set_icon_name: "media-playback-start-symbolic",
                set_css_classes: &["flat"],
                set_tooltip_text: Some("Start this application in the bubble"),
                connect_clicked[sender, index] => move |_| {
                    sender.output(AppRowOutput::Launch(index.clone())).unwrap();
                }
            },
            add_suffix = &gtk::Button {
                set_valign: gtk::Align::Center,
                set_icon_name: "view-pin-symbolic",
                set_css_classes: &["flat"],
                #[watch]
                set_class_active: ("accent", self.exported),
                #[watch]
                set_tooltip_text: Some(if !portal::available() {
                    "This desktop's portal cannot install application launchers"
                } else if self.exported {
                    "Remove from the host's application menu"
                } else {
                    "Add to the host's application menu"
                }),
                #[watch]
                set_sensitive: portal::available() && !self.busy,
                connect_clicked[sender, index] => move |_| {
                    sender.output(AppRowOutput::ToggleMenu(index.clone())).unwrap();
                }
            },
        }
    }

    fn init_model(
        RowInit { app, exported }: Self::Init,
        _index: &DynamicIndex,
        _sender: FactorySender<Self>,
    ) -> Self {
        let image = row_image(&app);
        Self { app, image, exported, busy: false }
    }

    fn update(&mut self, _msg: Self::Input, _sender: FactorySender<Self>) {}
}

#[derive(PartialEq, Debug, Clone)]
enum ListState {
    Loading,
    Loaded,
    Message(String, String),
}

pub struct AppsDialog {
    bubble: String,
    /// `None` while the bubble is not running.
    agent: Option<Agent>,
    rows: FactoryVecDeque<AppRow>,
    state: ListState,
    toasts: relm4::adw::ToastOverlay,
}

#[derive(Debug)]
pub enum AppsMsg {
    Load(String, Option<Agent>),
    /// Sent for every bubble whose status changes, shown or not.
    AgentChanged(String, Option<Agent>),
    Refresh,
    // Answers carry the bubble they were asked of; see `update_row`.
    Loaded(String, Box<GuestAppList>),
    Failed(String, String),
    Launch(DynamicIndex),
    LaunchFinished(String, Result<(), String>),
    ToggleMenu(DynamicIndex),
    ExportFinished(String, String, Result<Option<String>, String>),
    RemoveFinished(String, String, Result<(), String>),
}

#[relm4::component(pub)]
impl SimpleComponent for AppsDialog {
    type Init = ();
    type Input = AppsMsg;
    type Output = ();

    view! {
        dialog = relm4::adw::Dialog {
            set_content_width: 520,
            set_content_height: 620,
            set_title: "Applications",
            #[wrap(Some)]
            set_child = &relm4::adw::ToolbarView {
                add_top_bar = &relm4::adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &relm4::adw::WindowTitle {
                        set_title: "Applications",
                        #[watch]
                        set_subtitle: &model.bubble,
                    },
                    pack_end = &gtk::Button {
                        set_icon_name: "view-refresh-symbolic",
                        set_tooltip_text: Some("Reload the list"),
                        #[watch]
                        set_sensitive: model.state != ListState::Loading,
                        connect_clicked => AppsMsg::Refresh,
                    },
                },
                #[wrap(Some)]
                #[local_ref]
                set_content = toasts -> relm4::adw::ToastOverlay {
                    #[wrap(Some)]
                    set_child = &gtk::Stack {
                        add_named[Some("loading")] = &relm4::adw::StatusPage {
                            set_title: "Looking for applications",
                            #[wrap(Some)]
                            set_child = &relm4::adw::Spinner {
                                set_width_request: 32,
                                set_height_request: 32,
                            },
                        },
                        add_named[Some("message")] = &relm4::adw::StatusPage {
                            set_icon_name: Some("application-x-executable-symbolic"),
                            #[watch]
                            set_title: match &model.state {
                                ListState::Message(title, _) => title.as_str(),
                                _ => "",
                            },
                            #[watch]
                            set_description: Some(match &model.state {
                                ListState::Message(_, description) => description.as_str(),
                                _ => "",
                            }),
                        },
                        add_named[Some("list")] = &gtk::ScrolledWindow {
                            set_vexpand: true,
                            #[wrap(Some)]
                            set_child = &relm4::adw::Clamp {
                                set_margin_all: 12,
                                #[local_ref]
                                rows_listbox -> gtk::ListBox {
                                    set_css_classes: &["boxed-list"],
                                    set_selection_mode: gtk::SelectionMode::None,
                                    set_valign: gtk::Align::Start,
                                },
                            },
                        },
                        // Set last: naming a child only works once it exists.
                        #[watch]
                        set_visible_child_name: match &model.state {
                            ListState::Loading => "loading",
                            ListState::Loaded => "list",
                            ListState::Message(_, _) => "message",
                        },
                    },
                },
            },
        }
    }

    fn init(_init: Self::Init, _root: Self::Root, sender: ComponentSender<Self>) -> ComponentParts<Self> {
        let rows: FactoryVecDeque<AppRow> = FactoryVecDeque::builder()
            .launch_default()
            .forward(sender.input_sender(), |output| match output {
                AppRowOutput::Launch(index) => AppsMsg::Launch(index),
                AppRowOutput::ToggleMenu(index) => AppsMsg::ToggleMenu(index),
            });
        let rows_listbox_widget = rows.widget().clone();
        let toasts_widget = relm4::adw::ToastOverlay::new();

        let model = AppsDialog {
            bubble: String::new(),
            agent: None,
            rows,
            state: ListState::Loading,
            toasts: toasts_widget.clone(),
        };

        let rows_listbox = &rows_listbox_widget;
        let toasts = &toasts_widget;
        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            AppsMsg::Load(bubble, agent) => {
                self.bubble = bubble;
                self.agent = agent;
                sender.input(AppsMsg::Refresh);
            }
            AppsMsg::AgentChanged(bubble, agent) => {
                if bubble == self.bubble && agent != self.agent {
                    self.agent = agent;
                    sender.input(AppsMsg::Refresh);
                }
            }
            AppsMsg::Refresh => {
                let Some(agent) = self.agent.clone() else {
                    self.state = ListState::Message(
                        "This bubble is not running".to_string(),
                        "Start it to see the applications installed inside it.".to_string(),
                    );
                    return;
                };
                self.state = ListState::Loading;
                self.rows.guard().clear();
                let bubble = self.bubble.clone();
                relm4::spawn_local(async move {
                    let result = agent_request(&agent, reqwest::Method::GET, "/desktop-apps", None)
                        .await
                        .and_then(|(status, body)| {
                            if !status.is_success() {
                                return Err(format!(
                                    "The agent answered with status {}.",
                                    status.as_u16()
                                ));
                            }
                            serde_json::from_str::<GuestAppList>(&body)
                                .map_err(|error| format!("The bubble sent something unreadable: {error}"))
                        });
                    sender.input(match result {
                        Ok(list) => AppsMsg::Loaded(bubble, Box::new(list)),
                        Err(reason) => AppsMsg::Failed(bubble, reason),
                    });
                });
            }
            // The dialog may have moved on to another bubble while the request
            // was out; its answer must not land under that one's title.
            AppsMsg::Loaded(bubble, _) | AppsMsg::Failed(bubble, _) if bubble != self.bubble => {}
            AppsMsg::Loaded(_, list) => {
                let exported = exports::load(&self.bubble);
                let mut guard = self.rows.guard();
                guard.clear();
                for app in list.apps.into_iter().filter(has_valid_id) {
                    let is_exported =
                        exported.contains(&exports::desktop_file_id(&self.bubble, &app.id));
                    guard.push_back(RowInit { app, exported: is_exported });
                }
                let count = guard.len();
                drop(guard);
                self.state = if count == 0 {
                    ListState::Message(
                        "No applications".to_string(),
                        "This bubble has nothing installed that would show up in a menu."
                            .to_string(),
                    )
                } else {
                    ListState::Loaded
                };
            }
            AppsMsg::Failed(_, reason) => {
                self.state =
                    ListState::Message("Could not read the application list".to_string(), reason);
            }
            AppsMsg::Launch(index) => {
                let Some(app) = self.rows.get(index.current_index()).map(|row| row.app.clone())
                else {
                    return;
                };
                let Some(agent) = self.agent.clone() else {
                    self.report("This bubble is not running");
                    return;
                };
                let bubble = self.bubble.clone();
                relm4::spawn_local(async move {
                    let result = launcher::start(&agent, &bubble, &app.id).await;
                    sender.input(AppsMsg::LaunchFinished(app.name, result));
                });
            }
            AppsMsg::LaunchFinished(name, result) => match result {
                Ok(()) => self.toast(&format!("Starting “{name}”")),
                Err(error) => self.report(&format!("Could not start “{name}”: {error}")),
            },
            AppsMsg::ToggleMenu(index) => {
                let mut rows = self.rows.guard();
                let Some(row) = rows.get_mut(index.current_index()) else {
                    return;
                };
                row.busy = true;
                let (app, exported) = (row.app.clone(), row.exported);
                drop(rows);
                let bubble = self.bubble.clone();
                relm4::spawn_local(async move {
                    if exported {
                        let desktop_file_id = exports::desktop_file_id(&bubble, &app.id);
                        let result = portal::uninstall(&desktop_file_id)
                            .await
                            .and_then(|()| exports::remove(&bubble, &desktop_file_id));
                        sender.input(AppsMsg::RemoveFinished(bubble, app.id, result));
                    } else {
                        let result = export_launcher(&bubble, &app).await;
                        sender.input(AppsMsg::ExportFinished(bubble, app.id, result));
                    }
                });
            }
            AppsMsg::ExportFinished(bubble, app_id, result) => {
                let added = matches!(result, Ok(Some(_)));
                self.update_row(&bubble, &app_id, |row| {
                    row.busy = false;
                    if added {
                        row.exported = true;
                    }
                });
                match result {
                    // The user closed the portal's dialog.
                    Ok(None) => {}
                    Ok(Some(name)) => {
                        self.toast(&format!("“{name}” added to the application menu"));
                    }
                    Err(error) => self.report(&error),
                }
            }
            AppsMsg::RemoveFinished(bubble, app_id, result) => {
                let removed = result.is_ok();
                self.update_row(&bubble, &app_id, |row| {
                    row.busy = false;
                    if removed {
                        row.exported = false;
                    }
                });
                match result {
                    Ok(()) => {
                        self.toast("Removed from the application menu");
                    }
                    Err(error) => self.report(&error),
                }
            }
        }
    }
}

impl AppsDialog {
    // By id, not index: a refresh mid-export would move the answer onto
    // whatever row sits at that index now. And only while the dialog still
    // shows the bubble the answer is about: another may hold the same id.
    fn update_row(&mut self, bubble: &str, app_id: &str, update: impl FnOnce(&mut AppRow)) {
        if bubble != self.bubble {
            return;
        }
        let Some(position) = self.rows.iter().position(|row| row.app.id == app_id) else {
            return;
        };
        update(self.rows.guard().get_mut(position).expect("the row just found"));
    }

    // A toast truncates, and a portal error is the one message worth reading in
    // full, so it also goes to stderr.
    fn toast(&self, message: &str) {
        // Names and error bodies come from the guest.
        self.toasts.add_toast(
            relm4::adw::Toast::builder()
                .title(message)
                .use_markup(false)
                .build(),
        );
    }

    fn report(&self, message: &str) {
        eprintln!("bubbles: {message}");
        self.toast(message);
    }
}

/// The name the launcher ended up with, `None` if the user cancelled.
async fn export_launcher(bubble: &str, app: &GuestApp) -> Result<Option<String>, String> {
    let proposed = format!("{} ({bubble})", app.name);
    let Some(token) = portal::prepare_install(&proposed, icon_bytes(app)).await? else {
        return Ok(None);
    };

    let desktop_file_id = exports::desktop_file_id(bubble, &app.id);
    let entry = exports::desktop_entry(bubble, &app.id, &app.comment, &app.wm_class);
    portal::install(&token, &desktop_file_id, &entry).await?;
    // Unrecorded, the launcher would outlive the bubble: deleting it reads the
    // record. This is also how an export racing a deletion ends, the bubble's
    // directory being gone.
    if let Err(error) = exports::insert(bubble, &app.id) {
        if let Err(uninstall_error) = portal::uninstall(&desktop_file_id).await {
            eprintln!("could not remove launcher {desktop_file_id}: {uninstall_error}");
        }
        return Err(error);
    }
    Ok(Some(proposed))
}

// The caller reads the ids first: the file naming them lives in the directory
// that is about to be deleted.
pub async fn uninstall_launchers(desktop_file_ids: exports::Exports) {
    for desktop_file_id in desktop_file_ids {
        if let Err(error) = portal::uninstall(&desktop_file_id).await {
            eprintln!("could not remove launcher {desktop_file_id}: {error}");
        }
    }
}

pub mod exports {
    //! Launcher ids, the record of a bubble's launchers, and their desktop entries.

    use std::collections::BTreeSet;
    use std::fs;
    use std::path::PathBuf;

    use gtk::glib;

    use crate::config::vm_dir;
    use super::launcher::exec_line;
    use crate::APP_ID;

    /// The desktop file ids of a bubble's launchers. Kept because deleting the
    /// bubble has to uninstall them, and the portal cannot list them.
    pub type Exports = BTreeSet<String>;

    fn exports_path(bubble: &str) -> PathBuf {
        vm_dir(bubble).join("exports.json")
    }

    pub fn load(bubble: &str) -> Exports {
        fs::read_to_string(exports_path(bubble))
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    // A lost write leaves a launcher behind when the bubble is deleted.
    fn save(bubble: &str, exports: &Exports) -> Result<(), String> {
        let data = serde_json::to_string_pretty(exports).expect("a string set to serialise");
        let path = exports_path(bubble);
        fs::write(&path, data).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    // The portal requires flatpak_is_valid_name() below our app id: dot-separated
    // `[A-Za-z_][A-Za-z0-9_-]*`, hence the `b_` prefix and the hash. Bubble names
    // are `[A-Za-z0-9-]+` from creation on, so they fit as they are.
    pub fn desktop_file_id(bubble: &str, guest_app_id: &str) -> String {
        let hash = glib::compute_checksum_for_string(glib::ChecksumType::Sha256, guest_app_id)
            .expect("SHA-256 to be supported");
        format!("{APP_ID}.b_{bubble}_{:.8}.desktop", hash.as_str())
    }

    pub fn insert(bubble: &str, guest_app_id: &str) -> Result<(), String> {
        let mut exports = load(bubble);
        exports.insert(desktop_file_id(bubble, guest_app_id));
        save(bubble, &exports)
    }

    pub fn remove(bubble: &str, desktop_file_id: &str) -> Result<(), String> {
        let mut exports = load(bubble);
        exports.remove(desktop_file_id);
        save(bubble, &exports)
    }

    // KeyFile, not concatenation: a newline in a guest field would inject keys.
    // Name and Icon are left out; the portal overwrites them from PrepareInstall.
    pub fn desktop_entry(bubble: &str, guest_app_id: &str, comment: &str, wm_class: &str) -> String {
        let key_file = glib::KeyFile::new();
        key_file.set_string("Desktop Entry", "Type", "Application");
        key_file.set_string("Desktop Entry", "Exec", &exec_line(bubble, guest_app_id));
        if !comment.is_empty() {
            key_file.set_string("Desktop Entry", "Comment", comment);
        }
        if !wm_class.is_empty() {
            key_file.set_string("Desktop Entry", "StartupWMClass", wm_class);
        }
        key_file.set_string("Desktop Entry", "X-Bubbles-Bubble", bubble);
        key_file.to_data().to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `flatpak_is_valid_name()`: dot-separated `[A-Za-z_][A-Za-z0-9_-]*`.
        fn is_valid_flatpak_name(name: &str) -> bool {
            !name.is_empty()
                && name.split('.').count() >= 2
                && name.split('.').all(|element| {
                    let mut chars = element.chars();
                    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                })
        }

        #[test]
        fn the_portal_accepts_what_we_hand_it() {
            for bubble in ["work", "9lives", "with-dash"] {
                let id = desktop_file_id(bubble, "chromium.desktop");
                let name = id.strip_suffix(".desktop").expect("a .desktop suffix");
                assert!(name.starts_with(APP_ID), "{id} is not below the app id");
                assert!(is_valid_flatpak_name(name), "{id} would be rejected");
            }

            let entry = desktop_entry(
                "work",
                "chromium.desktop",
                "harmless\nExec=rm -rf ~",
                "wm\nTerminal=true",
            );
            let key_file = glib::KeyFile::new();
            key_file
                .load_from_data(&entry, glib::KeyFileFlags::NONE)
                .expect("the entry we produced to parse");
            assert_eq!(key_file.groups().len(), 1);
            assert!(key_file.string("Desktop Entry", "Terminal").is_err());
            assert_eq!(
                key_file.string("Desktop Entry", "Exec").unwrap(),
                exec_line("work", "chromium.desktop")
            );
        }
    }
}

pub mod launcher {
    //! Exported launchers run `bubbles --bubble <bubble> --start-app <id>`.
    //! GApplication hands that command line to the running instance, or starts
    //! one, so a failure is shown by the main window rather than by a process
    //! nobody watches.

    use gtk::gio::prelude::*;
    use gtk::gio::{self, ApplicationCommandLine};
    use gtk::glib::{self, thread_guard::ThreadGuard, ExitCode};
    use relm4::adw::prelude::*;

    use crate::agent::{agent_request, Agent};

    const BUBBLE_OPTION: &str = "bubble";
    const START_APP_OPTION: &str = "start-app";

    // The portal rewrites this into `flatpak run --command=bubbles ...`. With
    // `=`, a value starting with `-` cannot be read as an option.
    pub fn exec_line(bubble: &str, guest_app_id: &str) -> String {
        format!("bubbles --{BUBBLE_OPTION}={bubble} --{START_APP_OPTION}={guest_app_id}")
    }

    /// Before the application runs: the options have to be known when GApplication
    /// parses the command line, which happens before `startup`.
    pub fn setup(application: &gtk::Application) {
        application.set_flags(application.flags() | gio::ApplicationFlags::HANDLES_COMMAND_LINE);
        application.add_main_option(
            BUBBLE_OPTION,
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::String,
            "The bubble to start an application in",
            Some("BUBBLE"),
        );
        application.add_main_option(
            START_APP_OPTION,
            glib::Char::from(0),
            glib::OptionFlags::NONE,
            glib::OptionArg::String,
            "The desktop file id of the application to start",
            Some("APP"),
        );
    }

    /// A launcher's request, held until it is answered: the calling process waits
    /// until the last reference to the command line is gone, and exits with the
    /// status set by then.
    pub struct StartRequest {
        pub bubble: String,
        app_id: String,
        // AppMsg has to be Send for the bubbles' async factory; the request never
        // leaves the main thread.
        command_line: ThreadGuard<ApplicationCommandLine>,
    }

    impl std::fmt::Debug for StartRequest {
        fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            write!(f, "StartRequest({}, {})", self.bubble, self.app_id)
        }
    }

    /// From the component that knows the bubbles, since only it can say where a
    /// bubble's agent listens. `command-line` comes after `startup`, in which
    /// that component is built.
    pub fn connect(on_request: impl Fn(StartRequest) + 'static) {
        relm4::main_application().connect_command_line(move |application, command_line| {
            let options = command_line.options_dict();
            let option = |name| options.lookup::<String>(name).expect("a string option");
            match (option(BUBBLE_OPTION), option(START_APP_OPTION)) {
                (Some(bubble), Some(app_id)) => on_request(StartRequest {
                    bubble,
                    app_id,
                    command_line: ThreadGuard::new(command_line.clone()),
                }),
                _ => application.activate(),
            }
            ExitCode::SUCCESS
        });
    }

    /// `agent` is `None` while the bubble is not running.
    pub fn answer(request: StartRequest, agent: Option<Agent>) {
        let StartRequest { bubble, app_id, command_line } = request;
        let command_line = command_line.into_inner();
        let Some(agent) = agent else {
            show_error(
                &format!("The bubble “{bubble}” is not running"),
                "Start it, then launch the application again.",
            );
            command_line.set_exit_status(1);
            return;
        };
        relm4::spawn_local(async move {
            if let Err(error) = start(&agent, &bubble, &app_id).await {
                show_error(&format!("“{app_id}” could not be started"), &error);
                command_line.set_exit_status(1);
            }
        });
    }

    fn show_error(heading: &str, body: &str) {
        let application = relm4::main_application();
        application.activate();
        let dialog = relm4::adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("close", "Close");
        dialog.present(application.active_window().as_ref());
    }

    pub async fn start(agent: &Agent, bubble: &str, app_id: &str) -> Result<(), String> {
        let body = serde_json::json!({ "app": app_id });
        agent_request(agent, reqwest::Method::POST, "/start-desktop-app", Some(body))
            .await
            .and_then(|(status, body)| match status {
                status if status.is_success() => Ok(()),
                // Uninstalled inside the bubble since it was exported.
                reqwest::StatusCode::NOT_FOUND => {
                    Err(format!("“{app_id}” is not installed in “{bubble}”"))
                }
                status => match body.trim() {
                    "" => Err(format!("the bubble answered with status {}", status.as_u16())),
                    body => Err(body.to_string()),
                },
            })
    }
}

pub mod portal {
    //! The Dynamic Launcher portal.

    use std::sync::OnceLock;

    use ashpd::desktop::dynamic_launcher::{
        DynamicLauncherProxy, InstallOptions, PrepareInstallOptions, UninstallOptions,
    };
    use ashpd::desktop::{Icon, ResponseError};
    use gtk::gio::prelude::*;
    use gtk::gio::{self, DBusProxy, DBusProxyFlags};

    const FALLBACK_ICON: &[u8] = include_bytes!("../../flatpak/de.gonicus.bubbles.svg");

    static AVAILABLE: OnceLock<bool> = OnceLock::new();

    // xdg-desktop-portal-wlr and -hyprland ship no DynamicLauncher backend, and
    // the frontend cannot answer PrepareInstall without one. Synchronous, for the
    // view: GDBus loads the properties while building the proxy.
    pub fn available() -> bool {
        *AVAILABLE.get_or_init(|| {
            DBusProxy::for_bus_sync(
                gio::BusType::Session,
                DBusProxyFlags::NONE,
                None,
                "org.freedesktop.portal.Desktop",
                "/org/freedesktop/portal/desktop",
                "org.freedesktop.portal.DynamicLauncher",
                gio::Cancellable::NONE,
            )
            .ok()
            .and_then(|proxy| proxy.cached_property("version"))
            .is_some()
        })
    }

    async fn proxy() -> Result<DynamicLauncherProxy, String> {
        DynamicLauncherProxy::new()
            .await
            .map_err(|e| format!("no connection to the desktop portal: {e}"))
    }

    /// `None` means the user cancelled the portal's confirmation dialog.
    pub async fn prepare_install(name: &str, icon: Option<Vec<u8>>) -> Result<Option<String>, String> {
        // The portal accepts icon bytes only, so an application without an icon of
        // its own is exported under ours rather than a named fallback.
        let icon = Icon::Bytes(icon.unwrap_or_else(|| FALLBACK_ICON.to_vec()));
        let options = PrepareInstallOptions::default()
            .set_editable_name(true)
            .set_editable_icon(true);
        // No parent window: a real handle needs ashpd's gtk4 feature.
        let request = proxy()
            .await?
            .prepare_install(None, name, icon, options)
            .await
            .map_err(|e| format!("the portal refused to prepare the launcher: {e}"))?;
        match request.response() {
            Ok(response) => Ok(Some(response.token().to_string())),
            Err(ashpd::Error::Response(ResponseError::Cancelled)) => Ok(None),
            Err(e) => Err(format!("the portal could not prepare the launcher: {e}")),
        }
    }

    pub async fn install(token: &str, desktop_file_id: &str, desktop_entry: &str) -> Result<(), String> {
        proxy()
            .await?
            .install(token, desktop_file_id, desktop_entry, InstallOptions::default())
            .await
            .map_err(|e| format!("the portal could not install the launcher: {e}"))
    }

    pub async fn uninstall(desktop_file_id: &str) -> Result<(), String> {
        proxy()
            .await?
            .uninstall(desktop_file_id, UninstallOptions::default())
            .await
            .map_err(|e| format!("the portal could not remove the launcher: {e}"))
    }
}
