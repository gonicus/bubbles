use relm4::adw::prelude::*;
use gtk::gio::SubprocessFlags;
use gtk::prelude::{BoxExt, ButtonExt, GtkWindowExt};
use relm4::factory::DynamicIndex;
use relm4::prelude::{AsyncFactoryComponent, AsyncFactoryVecDeque};
use relm4::{
    AsyncFactorySender, Component, ComponentController, ComponentParts, ComponentSender, Controller, RelmApp, SimpleComponent, spawn
};
use std::{env, fs, path::Path, ffi::OsStr};
use libc::SIGTERM;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
struct BubbleConfig {
    cpus: u32,
    ram_mb: u32,
    sound_forwarding: bool,
    tcp_ports: String,
    map_host_loopback: String,
    shared_dirs: String,
}

impl Default for BubbleConfig {
    fn default() -> Self {
        Self {
            cpus: 4,
            ram_mb: 7000,
            sound_forwarding: false,
            tcp_ports: String::new(),
            map_host_loopback: String::new(),
            shared_dirs: String::new(),
        }
    }
}

fn config_path(vm_name: &str) -> std::path::PathBuf {
    env::current_dir()
        .expect("cwd to be set")
        .join(".bubbles/vms")
        .join(vm_name)
        .join("config.json")
}

fn load_config(vm_name: &str) -> BubbleConfig {
    let path = config_path(vm_name);
    match fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => BubbleConfig::default(),
    }
}

fn save_config(vm_name: &str, config: &BubbleConfig) {
    let path = config_path(vm_name);
    let data = serde_json::to_string_pretty(config).expect("config to serialize");
    fs::write(path, data).expect("config to be written");
}

struct BubbleSettingsDialog {
    root_dialog: relm4::adw::PreferencesDialog,
    vm_name: String,
    cpu_row: relm4::adw::SpinRow,
    ram_row: relm4::adw::SpinRow,
    sound_row: relm4::adw::SwitchRow,
    ports_row: relm4::adw::EntryRow,
    loopback_row: relm4::adw::EntryRow,
    dirs_row: relm4::adw::EntryRow,
}

#[derive(Debug)]
enum BubbleSettingsMsg {
    Load(String),
    Save,
}

#[relm4::component]
impl SimpleComponent for BubbleSettingsDialog {
    type Init = ();
    type Input = BubbleSettingsMsg;
    type Output = ();

    view! {
        dialog = relm4::adw::PreferencesDialog {
            set_title: "Bubble Settings",
            connect_closed => BubbleSettingsMsg::Save,
            add = &relm4::adw::PreferencesPage {
                add = &relm4::adw::PreferencesGroup {
                    set_title: "Resources",
                    #[local_ref]
                    add = cpu_row -> relm4::adw::SpinRow {
                        set_title: "CPU Cores",
                    },
                    #[local_ref]
                    add = ram_row -> relm4::adw::SpinRow {
                        set_title: "RAM (MB)",
                    },
                },
                add = &relm4::adw::PreferencesGroup {
                    set_title: "Features",
                    #[local_ref]
                    add = sound_row -> relm4::adw::SwitchRow {
                        set_title: "Sound Socket Forwarding",
                        set_subtitle: "Forward PulseAudio socket via VSOCK",
                    },
                },
                add = &relm4::adw::PreferencesGroup {
                    set_title: "Network",
                    set_description: Some("Applied on next startup"),
                    #[local_ref]
                    add = ports_row -> relm4::adw::EntryRow {
                        set_title: "TCP Port Forwards",
                    },
                    #[local_ref]
                    add = loopback_row -> relm4::adw::EntryRow {
                        set_title: "Map Host Loopback",
                    },
                },
                add = &relm4::adw::PreferencesGroup {
                    set_title: "Shared Directories",
                    set_description: Some("Comma-separated host paths (virtiofs)"),
                    #[local_ref]
                    add = dirs_row -> relm4::adw::EntryRow {
                        set_title: "Host Directories",
                    },
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let cpu_row = relm4::adw::SpinRow::with_range(1.0, 32.0, 1.0);
        let ram_row = relm4::adw::SpinRow::with_range(512.0, 32768.0, 512.0);
        let sound_row = relm4::adw::SwitchRow::new();
        let ports_row = relm4::adw::EntryRow::new();
        let loopback_row = relm4::adw::EntryRow::new();
        let dirs_row = relm4::adw::EntryRow::new();

        let model = BubbleSettingsDialog {
            root_dialog: root.clone(),
            vm_name: String::new(),
            cpu_row: cpu_row.clone(),
            ram_row: ram_row.clone(),
            sound_row: sound_row.clone(),
            ports_row: ports_row.clone(),
            loopback_row: loopback_row.clone(),
            dirs_row: dirs_row.clone(),
        };

        let cpu_row = &cpu_row;
        let ram_row = &ram_row;
        let sound_row = &sound_row;
        let ports_row = &ports_row;
        let loopback_row = &loopback_row;
        let dirs_row = &dirs_row;

        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            BubbleSettingsMsg::Load(name) => {
                self.vm_name = name;
                let config = load_config(&self.vm_name);
                self.cpu_row.set_value(config.cpus as f64);
                self.ram_row.set_value(config.ram_mb as f64);
                self.sound_row.set_active(config.sound_forwarding);
                self.ports_row.set_text(&config.tcp_ports);
                self.loopback_row.set_text(&config.map_host_loopback);
                self.dirs_row.set_text(&config.shared_dirs);
            }
            BubbleSettingsMsg::Save => {
                if self.vm_name.is_empty() { return; }
                let config = BubbleConfig {
                    cpus: self.cpu_row.value() as u32,
                    ram_mb: self.ram_row.value() as u32,
                    sound_forwarding: self.sound_row.is_active(),
                    tcp_ports: self.ports_row.text().to_string(),
                    map_host_loopback: self.loopback_row.text().to_string(),
                    shared_dirs: self.dirs_row.text().to_string(),
                };
                save_config(&self.vm_name, &config);
            }
        }
    }
}

struct CreateBubbleDialog {
}

struct WarnCloseDialog {
    root_dialog: relm4::adw::Dialog,
}

#[derive(PartialEq, Debug, Clone)]
enum ImageStatus {
    NotPresent,
    Downloading,
    Present,
}

fn determine_download_status() -> ImageStatus {
    let images_dir = env::current_dir()
        .expect("cwd to be set")
        .join(Path::new(".bubbles/images"));
    fs::create_dir_all(&images_dir).expect("directory to exist or be created");

    let image_exists = images_dir.join(Path::new("debian-13/disk.img")).exists();

    return match image_exists {
        true => ImageStatus::Present,
        false => ImageStatus::NotPresent,
    };
}

pub async fn wait_until_exists(path: &OsStr) {
    loop {
        let process = gtk::gio::Subprocess::newv(
            &[
                OsStr::new("sh"),
                OsStr::new("-c"),
                OsStr::new("stat $0 || (sleep 0.5 && exit 1)"),
                path,
            ],
            SubprocessFlags::empty()
        ).expect("start of process");
        process.wait_future().await.expect("probe to run");
        if process.is_successful() {
            return;
        }
    }
}

pub async fn wait_until_ready(vsock_socket_path: &OsStr) {
    loop {
        let process = gtk::gio::Subprocess::newv(
            &[
                OsStr::new("sh"),
                OsStr::new("-c"),
                OsStr::new("curl -sS --unix-socket $0 http://localhost/ready || (sleep 0.5 && exit 1)"),
                vsock_socket_path,
            ],
            SubprocessFlags::empty()
        ).expect("start of process");
        process.wait_future().await.expect("probe to run");
        if process.is_successful() {
            return;
        }
    }
}

pub async fn request_shutdown(vsock_socket_path: &OsStr) {
    let process = gtk::gio::Subprocess::newv(
        &[
            OsStr::new("curl"),
            OsStr::new("-XPOST"),
            OsStr::new("--unix-socket"),
            vsock_socket_path,
            OsStr::new("http://localhost/shutdown"),
        ],
        SubprocessFlags::empty()
    ).expect("start of process");
    process.wait_future().await.expect("request to be made");
}

pub async fn request_terminal(vsock_socket_path: &OsStr) {
    let process = gtk::gio::Subprocess::newv(
        &[
            OsStr::new("curl"),
            OsStr::new("-XPOST"),
            OsStr::new("--unix-socket"),
            vsock_socket_path,
            OsStr::new("http://localhost/spawn-terminal"),
        ],
        SubprocessFlags::empty()
    ).expect("start of process");
    process.wait_future().await.expect("request to be made");
}

#[derive(PartialEq, Debug, Clone)]
enum WarnCloseDialogMsg {
    Ack,
}

#[relm4::component]
impl SimpleComponent for WarnCloseDialog {
    type Init = ();
    type Input = WarnCloseDialogMsg;
    type Output = AppMsg;

    view! {
        dialog = relm4::adw::Dialog {
            set_size_request: (400, 200),
            #[wrap(Some)]
            set_child = &relm4::adw::StatusPage {
                set_icon_name: Some("computer-fail-symbolic"),
                set_title: "Processes still running",
                set_description: Some("Please stop all running downloads and bubbles, first"),
                #[wrap(Some)]
                set_child = &gtk::Button {
                    set_label: "OK",
                    connect_clicked => WarnCloseDialogMsg::Ack,
                }
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        _sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = WarnCloseDialog { root_dialog: root.clone() };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, _sender: ComponentSender<Self>) {
        match msg {
            WarnCloseDialogMsg::Ack => {
                self.root_dialog.close();
            }
        }
    }
}

#[relm4::component]
impl SimpleComponent for CreateBubbleDialog {
    type Init = ();
    type Input = ();
    type Output = AppMsg;

    view! {
        dialog = relm4::adw::Dialog {
            set_presentation_mode: relm4::adw::DialogPresentationMode::BottomSheet,
            #[wrap(Some)]
            set_child = &relm4::adw::StatusPage {
                set_icon_name: Some("window-new-symbolic"),
                set_title: "Create new Bubble",
                set_description: Some("Enter name and confirm with ENTER"),
                #[wrap(Some)]
                set_child = &gtk::Entry {
                    connect_activate[sender] => move |entry| {
                        let name: String = entry.text().into();
                        sender.output(AppMsg::CreateNewBubble(name)).unwrap();
                        entry.buffer().delete_text(0, None);
                        sender.output(AppMsg::HideBubbleCreationDialog).unwrap();
                    }
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let model = CreateBubbleDialog { };
        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, _msg: Self::Input, _sender: ComponentSender<Self>) {}
}

struct App {
    vms: AsyncFactoryVecDeque<VmEntry>,
    create_bubble_dialog: Controller<CreateBubbleDialog>,
    warn_close_dialog: Controller<WarnCloseDialog>,
    settings_dialog: Controller<BubbleSettingsDialog>,
    currently_creating_bubble: bool,
    image_status: ImageStatus,
    root: relm4::adw::Window,
}

#[derive(PartialEq, Debug, Clone)]
enum VMStatus {
    NotRunning,
    Running,
    InFlux,
}

#[derive(PartialEq, Debug, Clone)]
struct VM {
    name: String,
    status: VMStatus,
}

fn load_vms() -> Vec<VM> {
    let vms_dir = env::current_dir()
        .expect("cwd to be set")
        .join(Path::new(".bubbles/vms"));
    fs::create_dir_all(&vms_dir).expect("directory to exist or be created");
    let mut vms: Vec<VM> = vec![];
    for dir in fs::read_dir(vms_dir).expect("to exist") {
        let dir = dir.expect("to exist");
        let vm_name = dir
            .file_name()
            .into_string()
            .expect("path to be serializable");
        vms.push(VM {
            name: vm_name.clone(),
            status: VMStatus::NotRunning,
        });
    }
    return vms;
}

async fn create_vm(name: String) {
    println!("starting copy");
    let vm_dir_path = &env::current_dir()
        .expect("to be set")
        .join(".bubbles/vms")
        .join(&name);
    tokio::fs::create_dir_all(vm_dir_path).await.expect("directories to be created");
    let image_base_path = env::current_dir()
        .expect("to be set")
        .join(".bubbles/images/debian-13");
    let image_disk_path = image_base_path.join("disk.img");
    let image_linuz_path = image_base_path.join("vmlinuz");
    let image_initrd_path = image_base_path.join("initrd.img");
    tokio::fs::copy(image_disk_path, vm_dir_path.join("disk.img")).await.expect("disk copy to succeed");
    tokio::fs::copy(image_linuz_path, vm_dir_path.join("vmlinuz")).await.expect("vmlinuz copy to succeed");
    tokio::fs::copy(image_initrd_path, vm_dir_path.join("initrd.img")).await.expect("initrd copy to succeed");
    save_config(&name, &BubbleConfig::default());
    println!("done copy");
}

#[derive(Debug)]
enum VmMsg {
    PowerToggle(DynamicIndex),
    StartTerminal(DynamicIndex),
    OpenSettings(DynamicIndex),
}

#[derive(Debug)]
enum VmStateUpdate {
    Update(DynamicIndex, VMStatus),
    OpenSettings(String),
}

#[derive(PartialEq, Debug)]
struct VmEntry {
    value: VM,
}

#[relm4::factory(async)]
impl AsyncFactoryComponent for VmEntry {
    type Init = VM;
    type Input = VmMsg;
    type Output = VmStateUpdate;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        #[root]
        relm4::adw::ActionRow {
            set_title: &self.value.name,
            add_prefix = &gtk::Image {
                set_icon_name: Some("computer-symbolic")
            },
            add_suffix = &gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_spacing: 5,
                append = &gtk::Label {
                    #[watch]
                    set_label: match self.value.status {
                        VMStatus::NotRunning => "Stopped",
                        VMStatus::Running => "Running",
                        VMStatus::InFlux => "Working...",
                    }
                },
                append = &gtk::Button {
                    set_icon_name: "emblem-system-symbolic",
                    set_tooltip_text: Some("Settings"),
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::OpenSettings(index.clone()));
                    }
                },
                append = &gtk::Button {
                    set_icon_name: "system-shutdown-symbolic",
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::PowerToggle(index.clone()));
                    }
                },
                append = &gtk::Button {
                    #[watch]
                    set_sensitive: self.value.status == VMStatus::Running,
                    set_icon_name: "utilities-terminal-symbolic",
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::StartTerminal(index.clone()));
                    }
                },
            }
        }
    }

    async fn init_model(
        value: Self::Init,
        _index: &DynamicIndex,
        _sender: AsyncFactorySender<Self>,
    ) -> Self {
        Self { value }
    }
    async fn update(&mut self, msg: Self::Input, sender: AsyncFactorySender<Self>) {
        let vm_name: String = self.value.name.clone();
        let image_base_path = env::current_dir()
            .expect("to be set")
            .join(".bubbles/vms").join(vm_name.clone());
        let vsock_socket_path = image_base_path.join("vsock");
        match msg {
            VmMsg::OpenSettings(_index) => {
                sender.output(VmStateUpdate::OpenSettings(vm_name)).unwrap();
            },
            VmMsg::PowerToggle(index) => {
                match self.value.status {
                    VMStatus::Running | VMStatus::InFlux => {
                        relm4::spawn_local(async move {
                            request_shutdown(OsStr::new(&vsock_socket_path)).await;
                        });
                    },
                    VMStatus::NotRunning => {
                        sender.output(VmStateUpdate::Update(index.clone(), VMStatus::InFlux)).unwrap();
                        relm4::spawn_local(async move {
                            let config = load_config(&vm_name);
                            let crosvm_socket_path = image_base_path.join("crosvm_socket");
                            let passt_socket_path = Path::new("/tmp").join(format!("passt_socket_{}", vm_name.clone()));
                            let image_disk_path = image_base_path.join("disk.img");
                            let image_linuz_path = image_base_path.join("vmlinuz");
                            let image_initrd_path = image_base_path.join("initrd.img");
                            let socat_process = gtk::gio::Subprocess::newv(
                                &[
                                    OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/socat").as_os_str()),
                                    OsStr::new(&format!("UNIX-LISTEN:{},fork", vsock_socket_path.to_str().expect("string"))),
                                    OsStr::new(&format!("VSOCK-CONNECT:{}:11111", index.current_index() + 10)),
                                ],
                                SubprocessFlags::empty()
                            ).expect("start of socat process");

                            // Sound socket forwarding
                            let sound_socat_process = if config.sound_forwarding {
                                let xdg_runtime = env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR to be set");
                                let pulse_path = format!("{}/pulse/native", xdg_runtime);
                                Some(gtk::gio::Subprocess::newv(
                                    &[
                                        OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/socat").as_os_str()),
                                        OsStr::new("VSOCK-LISTEN:11112,fork"),
                                        OsStr::new(&format!("UNIX-CONNECT:{}", pulse_path)),
                                    ],
                                    SubprocessFlags::empty()
                                ).expect("start of sound socat process"))
                            } else {
                                None
                            };

                            // Build passt args
                            let mut passt_args: Vec<String> = vec![
                                "passt".into(),
                                "-f".into(),
                                "--vhost-user".into(),
                                "--socket".into(),
                                passt_socket_path.to_str().expect("string").into(),
                            ];
                            if !config.tcp_ports.trim().is_empty() {
                                passt_args.push("--tcp-ports".into());
                                passt_args.push(config.tcp_ports.trim().into());
                            }
                            if !config.map_host_loopback.trim().is_empty() {
                                passt_args.push("--map-host-loopback".into());
                                passt_args.push(config.map_host_loopback.trim().into());
                            }
                            let passt_args_os: Vec<&OsStr> = passt_args.iter().map(|s| OsStr::new(s.as_str())).collect();
                            let passt_process = gtk::gio::Subprocess::newv(
                                &passt_args_os,
                                SubprocessFlags::empty()
                            ).expect("start of passt process");
                            wait_until_exists(passt_socket_path.as_os_str()).await;

                            // Build crosvm args
                            let cpus_str = format!("num-cores={}", config.cpus);
                            let ram_str = format!("{}", config.ram_mb);
                            let vsock_str = format!("{}", index.current_index() + 10);
                            let wayland_path = Path::new(&env::var("XDG_RUNTIME_DIR").expect("XDG var to be set"))
                                .join(env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY var to be set"));
                            let vhost_net_str = format!("net,socket={}", passt_socket_path.to_str().expect("string"));
                            let crosvm_bin = Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/crosvm");

                            let mut crosvm_args: Vec<Box<dyn AsRef<OsStr>>> = vec![
                                Box::new(crosvm_bin.clone()),
                                Box::new("run".to_string()),
                                Box::new("--name".to_string()),
                                Box::new(vm_name.clone()),
                                Box::new("--cpus".to_string()),
                                Box::new(cpus_str),
                                Box::new("-m".to_string()),
                                Box::new(ram_str),
                                Box::new("--rwdisk".to_string()),
                                Box::new(image_disk_path.clone()),
                                Box::new("--initrd".to_string()),
                                Box::new(image_initrd_path.clone()),
                                Box::new("--socket".to_string()),
                                Box::new(crosvm_socket_path.clone()),
                                Box::new("--vsock".to_string()),
                                Box::new(vsock_str),
                                Box::new("--gpu".to_string()),
                                Box::new("context-types=cross-domain,displays=[]".to_string()),
                                Box::new("--wayland-sock".to_string()),
                                Box::new(wayland_path),
                                Box::new("--vhost-user".to_string()),
                                Box::new(vhost_net_str),
                                Box::new("-p".to_string()),
                                Box::new("root=/dev/vda2".to_string()),
                            ];

                            // Add shared directories
                            let shared_dirs: Vec<&str> = config.shared_dirs.split(',')
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .collect();
                            for (i, dir) in shared_dirs.iter().enumerate() {
                                let tag = format!("shared{}", i);
                                let shared_arg = format!("{}:{}:type=fs", dir, tag);
                                crosvm_args.push(Box::new("--shared-dir".to_string()));
                                crosvm_args.push(Box::new(shared_arg));
                            }

                            crosvm_args.push(Box::new(image_linuz_path.clone()));

                            let crosvm_args_os: Vec<&OsStr> = crosvm_args.iter().map(|s| (*s).as_ref().as_ref()).collect();
                            let crosvm_process = gtk::gio::Subprocess::newv(
                                &crosvm_args_os,
                                SubprocessFlags::empty()
                            ).expect("start of process");
                            wait_until_ready(vsock_socket_path.as_os_str()).await;
                            sender.output(VmStateUpdate::Update(index.clone(), VMStatus::Running)).unwrap();
                            crosvm_process.wait_future().await.expect("vm to stop");
                            socat_process.send_signal(SIGTERM); // Marker: Incompatible with Windows
                            passt_process.send_signal(SIGTERM);
                            if let Some(ref sound_proc) = sound_socat_process {
                                sound_proc.send_signal(SIGTERM);
                            }
                            socat_process.wait_future().await.expect("socat to stop");
                            passt_process.wait_future().await.expect("passt to stop");
                            if let Some(sound_proc) = sound_socat_process {
                                sound_proc.wait_future().await.expect("sound socat to stop");
                            }
                            sender.output(VmStateUpdate::Update(index, VMStatus::NotRunning)).unwrap();
                        });
                    },
                }
            },
            VmMsg::StartTerminal(_index) => {
                relm4::spawn_local(async move {
                    request_terminal(OsStr::new(&vsock_socket_path)).await;
                });
            }
        }
    }
}

#[derive(Debug)]
enum AppMsg {
    DownloadImage,
    FinishImageDownload,
    ShowBubbleCreationDialog,
    HideBubbleCreationDialog,
    CreateNewBubble(String),
    HandleVMStatusUpdate(DynamicIndex, VMStatus),
    FinishBubbleCreation,
    CloseApplication,
    OpenBubbleSettings(String),
}

#[relm4::component]
impl SimpleComponent for App {
    type Init = ();
    type Input = AppMsg;
    type Output = ();

    view! {
        #[root]
        relm4::adw::Window {
            set_title: Some("Bubbles"),
            set_default_size: (600, 600),

            relm4::adw::ToolbarView {
                add_top_bar = &relm4::adw::HeaderBar {
                    #[wrap(Some)]
                    set_title_widget = &relm4::adw::ViewSwitcher {
                        set_stack: Some(&stack),
                        set_policy: relm4::adw::ViewSwitcherPolicy::Wide
                    },
                    pack_end = &gtk::Button{
                        set_icon_name: "list-add-symbolic",
                        #[watch]
                        set_sensitive: !model.currently_creating_bubble && model.image_status == ImageStatus::Present,
                        set_tooltip_text: Some("Create new bubble"),
                        connect_clicked => AppMsg::ShowBubbleCreationDialog,
                    },
                    pack_end = &gtk::Spinner{
                        #[watch]
                        set_spinning: model.currently_creating_bubble
                    },
                },
                #[wrap(Some)]
                set_content: stack = &relm4::adw::ViewStack {
                    add = &gtk::ListBox {
                        append = &relm4::adw::ActionRow {
                            set_title: "Debian 13 Bubbles Distribution",
                            add_prefix = &gtk::Image {
                                set_icon_name: Some("drive-harddisk-system-symbolic")
                            },
                            add_suffix = &gtk::Box {
                                set_orientation: gtk::Orientation::Horizontal,
                                set_spacing: 5,
                                append = &gtk::Label {
                                    #[watch]
                                    set_label: match model.image_status {
                                        ImageStatus::Present => "Ready",
                                        ImageStatus::NotPresent => "Not downloaded",
                                        ImageStatus::Downloading => "Downloading...",
                                    }
                                },
                                append = &gtk::Button {
                                    #[watch]
                                    set_sensitive: model.image_status != ImageStatus::Downloading,
                                    #[watch]
                                    set_icon_name: match model.image_status {
                                        ImageStatus::Present => "view-refresh-symbolic",
                                        ImageStatus::NotPresent => "folder-download-symbolic",
                                        ImageStatus::Downloading => "image-loading-symbolic",
                                    },
                                    connect_clicked => AppMsg::DownloadImage,
                                }
                            }
                        }
                    } -> {
                        set_title: Some("Images"),
                        set_icon_name: Some("drive-harddisk-system-symbolic")
                    },
                    #[local_ref]
                    add = vms_stack -> gtk::Stack {
                        add_named[Some("create-view")] = &relm4::adw::StatusPage {
                            set_title: "No bubbles here, yet",
                            set_description: Some("Make sure to download an image, then click below."),
                            set_icon_name: Some("computer"),
                            #[wrap(Some)]
                            set_child = &gtk::Button {
                                #[watch]
                                set_sensitive: !model.currently_creating_bubble && model.image_status == ImageStatus::Present,
                                set_css_classes: &["pill", "suggested-action"],
                                set_label: "Create new Bubble",
                                connect_clicked => AppMsg::ShowBubbleCreationDialog
                            }
                        },
                        #[watch]
                        set_visible_child_name: match model.vms.len() {
                            0 => "create-view",
                            _ => "vm-view",
                        },
                    } -> {
                        set_title: Some("Bubbles"),
                        set_icon_name: Some("computer-symbolic"),
                    }
                }
            },

            connect_close_request[sender] => move |_| {
                sender.input(AppMsg::CloseApplication);
                gtk::glib::signal::Propagation::Stop
            }
        },
    }

    fn init(
        _none: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let vms: AsyncFactoryVecDeque<VmEntry> =
            AsyncFactoryVecDeque::builder()
                .launch_default()
                .forward(sender.input_sender(), |output| match output {
                    VmStateUpdate::Update(index, status_update) => AppMsg::HandleVMStatusUpdate(index, status_update),
                    VmStateUpdate::OpenSettings(name) => AppMsg::OpenBubbleSettings(name),
                });
        let create_bubble_dialog = CreateBubbleDialog::builder()
            .launch(())
            .forward(sender.input_sender(), |msg| match msg {
                msg => msg
            });
        let warn_close_dialog = WarnCloseDialog::builder()
            .launch(())
            .forward(sender.input_sender(), |msg| match msg {
                msg => msg
            });
        let settings_dialog = BubbleSettingsDialog::builder()
            .launch(())
            .detach();

        let mut model = App {
            vms,
            create_bubble_dialog,
            warn_close_dialog,
            settings_dialog,
            root: root.clone(),
            currently_creating_bubble: false,
            image_status: determine_download_status(),
        };
        for vm in load_vms() {
            model.vms.guard().push_back(vm);
        }
        let vms_stack = &gtk::Stack::new();
        vms_stack.add_named(model.vms.widget(), Some("vm-view"));

        let widgets = view_output!();

        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            AppMsg::ShowBubbleCreationDialog=>{
                self.create_bubble_dialog.widgets().dialog.present(Some(&self.root));
            }
            AppMsg::HideBubbleCreationDialog=>{
                self.create_bubble_dialog.widgets().dialog.close();
            }
            AppMsg::CreateNewBubble(name) => {
                self.currently_creating_bubble = true;
                spawn(async move {
                    create_vm(name).await;
                    sender.input(AppMsg::FinishBubbleCreation);
                });
            }
            AppMsg::FinishBubbleCreation=>{
                let new_vms = load_vms();
                self.currently_creating_bubble = false;
                self.vms.guard().clear();
                for vm in new_vms {
                    self.vms.guard().push_back(vm);
                }
            }
            AppMsg::DownloadImage => {
                self.image_status = ImageStatus::Downloading;
                relm4::spawn_local(async move {
                    gtk::gio::Subprocess::newv(
                        &[OsStr::new("scripts/download.bash")],
                        SubprocessFlags::empty()
                    ).expect("download").wait_future().await.expect("download to succeed");
                    sender.input(AppMsg::FinishImageDownload);
                });
            }
            AppMsg::FinishImageDownload => {
                self.image_status = determine_download_status();
            }
            AppMsg::HandleVMStatusUpdate(index, status_update) => {
                self.vms.guard().get_mut(index.current_index()).unwrap().value.status = status_update;
            }
            AppMsg::OpenBubbleSettings(name) => {
                self.settings_dialog.sender().send(BubbleSettingsMsg::Load(name)).unwrap();
                self.settings_dialog.widgets().dialog.present(Some(&self.root));
            }
            AppMsg::CloseApplication => {
                let mut vm_running = false;
                for vm in self.vms.guard().iter_mut() {
                    if vm.unwrap().value.status != VMStatus::NotRunning {
                        vm_running = true;
                    }
                }
                if self.image_status == ImageStatus::Downloading || self.currently_creating_bubble || vm_running {
                    self.warn_close_dialog.widgets().dialog.present(Some(&self.root));
                    return
                }

                relm4::main_application().quit();
            }
        }
    }
}

fn main() {
    let app = RelmApp::new("bubbles");
    app.run::<App>(());
}
