use relm4::adw::prelude::*;
use gtk::gio::SubprocessFlags;
use gtk::prelude::{BoxExt, ButtonExt, GtkWindowExt};
use relm4::factory::DynamicIndex;
use relm4::prelude::{AsyncFactoryComponent, AsyncFactoryVecDeque};
use relm4::{
    AsyncFactorySender, Component, ComponentController, ComponentParts, ComponentSender, Controller, RelmApp, SimpleComponent, spawn
};
use std::{env, fs, path::{Path, PathBuf}, ffi::{OsStr, OsString}};
use libc::SIGTERM;
use tokio::io::{AsyncWriteExt, AsyncReadExt};

fn get_data_dir() -> PathBuf {
    let base = env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").expect("HOME")).join(".local/share"));
    base.join("bubbles")
}

fn is_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

fn make_host_args(args: &[&OsStr]) -> Vec<OsString> {
    if is_flatpak() {
        let uid = unsafe { libc::getuid() };
        let mut v: Vec<OsString> = vec![
            "flatpak-spawn".into(),
            "--host".into(),
            format!("--env=XDG_RUNTIME_DIR=/run/user/{}", uid).into(),
        ];
        v.extend(args.iter().map(|a| (*a).to_owned()));
        v
    } else {
        args.iter().map(|a| (*a).to_owned()).collect()
    }
}

fn flatpak_host_bin(name: &str) -> PathBuf {
    // /.flatpak-info is always readable inside the sandbox and contains
    // app-path=<host path> for the actual installation (user or system).
    if let Ok(content) = fs::read_to_string("/.flatpak-info") {
        for line in content.lines() {
            if let Some(path) = line.strip_prefix("app-path=") {
                return PathBuf::from(path).join("bin").join(name);
            }
        }
    }
    // Fallback for non-sandbox use
    PathBuf::from(name)
}

fn wayland_sock_path() -> PathBuf {
    if is_flatpak() {
        let uid = unsafe { libc::getuid() };
        let display = env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY");
        PathBuf::from(format!("/run/user/{}/{}", uid, display))
    } else {
        let runtime_dir = env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        let display = env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY");
        PathBuf::from(runtime_dir).join(display)
    }
}

async fn unix_http(socket: &Path, method: &str, path: &str) -> std::io::Result<String> {
    let mut stream = tokio::net::UnixStream::connect(socket).await?;
    // Content-Length: 0 included for POST correctness; harmless on GET
    let req = format!(
        "{} {} HTTP/1.0\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n",
        method, path
    );
    stream.write_all(req.as_bytes()).await?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
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
    let images_dir = get_data_dir().join("images");
    fs::create_dir_all(&images_dir).expect("directory to exist or be created");

    let image_exists = images_dir.join(Path::new("debian-13")).exists();

    return match image_exists {
        true => ImageStatus::Present,
        false => ImageStatus::NotPresent,
    };
}

pub async fn wait_until_exists(path: &Path) {
    loop {
        let exists = if is_flatpak() {
            // /tmp is sandbox-private; check on the host
            let args = make_host_args(&[
                OsStr::new("test"),
                OsStr::new("-e"),
                path.as_os_str(),
            ]);
            let args_ref: Vec<&OsStr> = args.iter().map(OsString::as_os_str).collect();
            let p = gtk::gio::Subprocess::newv(&args_ref, SubprocessFlags::STDERR_SILENCE)
                .expect("spawn host test");
            p.wait_future().await.ok();
            p.is_successful()
        } else {
            path.exists()
        };
        if exists { return; }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
}

pub async fn wait_until_ready(vsock_socket_path: &Path) {
    loop {
        match unix_http(vsock_socket_path, "GET", "/ready").await {
            Ok(response) if response.contains("200") => return,
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

pub async fn request_shutdown(vsock_socket_path: &Path) {
    unix_http(vsock_socket_path, "POST", "/shutdown").await.ok();
}

pub async fn request_terminal(vsock_socket_path: &Path) {
    unix_http(vsock_socket_path, "POST", "/spawn-terminal").await.ok();
}

async fn download_image() {
    let target_dir = get_data_dir().join("images/debian-13");
    tokio::fs::create_dir_all(&target_dir).await.unwrap();

    // Step 1: oras pull (runs inside sandbox — just needs --share=network)
    // In Flatpak: bundled at /app/bin/oras; outside: resolved via PATH
    let oras_bin = if is_flatpak() { "/app/bin/oras" } else { "oras" };
    gtk::gio::Subprocess::newv(&[
        OsStr::new(oras_bin),
        OsStr::new("pull"),
        OsStr::new("ghcr.io/gonicus/bubbles/vm-image:e289a3a5479817c3ffad6bb62d8214e4265e8e4b"),
        OsStr::new("--output"),
        target_dir.as_os_str(),
    ], SubprocessFlags::empty())
        .expect("oras pull to start")
        .wait_future().await.expect("oras pull to complete");

    // Step 2: qemu-img convert
    // In Flatpak: bundled at /app/bin/qemu-img; outside: resolved via PATH
    let qemu_img = if is_flatpak() { "/app/bin/qemu-img" } else { "qemu-img" };
    let qcow2_path = target_dir.join("disk.qcow2");
    let raw_path = target_dir.join("disk.img");
    gtk::gio::Subprocess::newv(&[
        OsStr::new(qemu_img),
        OsStr::new("convert"),
        OsStr::new("-f"), OsStr::new("qcow2"),
        OsStr::new("-O"), OsStr::new("raw"),
        qcow2_path.as_os_str(),
        raw_path.as_os_str(),
    ], SubprocessFlags::empty())
        .expect("qemu-img to start")
        .wait_future().await.expect("qemu-img to complete");

    tokio::fs::remove_file(&qcow2_path).await.ok();

    // Step 3: expand disk (native Rust, no truncate binary needed)
    let f = tokio::fs::OpenOptions::new().write(true).open(&raw_path).await.unwrap();
    let current_size = f.metadata().await.unwrap().len();
    f.set_len(current_size + 15 * 1024 * 1024 * 1024).await.unwrap();
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
    let vms_dir = get_data_dir().join("vms");
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
    let vm_dir_path = get_data_dir().join("vms").join(&name);
    tokio::fs::create_dir_all(&vm_dir_path).await.expect("directories to be created");
    let image_base_path = get_data_dir().join("images/debian-13");
    let image_disk_path = image_base_path.join("disk.img");
    let image_linuz_path = image_base_path.join("vmlinuz");
    let image_initrd_path = image_base_path.join("initrd.img");
    tokio::fs::copy(image_disk_path, vm_dir_path.join("disk.img")).await.expect("disk copy to succeed");
    tokio::fs::copy(image_linuz_path, vm_dir_path.join("vmlinuz")).await.expect("vmlinuz copy to succeed");
    tokio::fs::copy(image_initrd_path, vm_dir_path.join("initrd.img")).await.expect("initrd copy to succeed");
    println!("done copy");
}

#[derive(Debug)]
enum VmMsg {
    PowerToggle(DynamicIndex),
    StartTerminal(DynamicIndex),
}

#[derive(Debug)]
enum VmStateUpdate {
    Update(DynamicIndex, VMStatus)
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
        let image_base_path = get_data_dir().join("vms").join(vm_name.clone());
        let vsock_socket_path = image_base_path.join("vsock");
        match msg {
            VmMsg::PowerToggle(index) => {
                match self.value.status {
                    VMStatus::Running | VMStatus::InFlux => {
                        relm4::spawn_local(async move {
                            request_shutdown(&vsock_socket_path).await;
                        });
                    },
                    VMStatus::NotRunning => {
                        sender.output(VmStateUpdate::Update(index.clone(), VMStatus::InFlux)).unwrap();
                        relm4::spawn_local(async move {
                            let crosvm_socket_path = image_base_path.join("crosvm_socket");
                            let passt_socket_path = Path::new("/tmp").join(format!("passt_socket_{}", vm_name.clone()));
                            let image_disk_path = image_base_path.join("disk.img");
                            let image_linuz_path = image_base_path.join("vmlinuz");
                            let image_initrd_path = image_base_path.join("initrd.img");

                            let socat_bin: OsString = if is_flatpak() {
                                flatpak_host_bin("socat").into_os_string()
                            } else {
                                OsString::from("socat")
                            };
                            let socat_unix = format!("UNIX-LISTEN:{},fork", vsock_socket_path.to_str().expect("string"));
                            let socat_vsock = format!("VSOCK-CONNECT:{}:11111", index.current_index() + 10);
                            let socat_host_args = make_host_args(&[
                                socat_bin.as_os_str(),
                                OsStr::new(&socat_unix),
                                OsStr::new(&socat_vsock),
                            ]);
                            let socat_host_args_ref: Vec<&OsStr> = socat_host_args.iter().map(OsString::as_os_str).collect();
                            let socat_process = gtk::gio::Subprocess::newv(
                                &socat_host_args_ref,
                                SubprocessFlags::empty()
                            ).expect("start of socat process");

                            let passt_host_args = make_host_args(&[
                                OsStr::new("passt"),
                                OsStr::new("-f"),
                                OsStr::new("--vhost-user"),
                                OsStr::new("--socket"),
                                passt_socket_path.as_os_str(),
                            ]);
                            let passt_host_args_ref: Vec<&OsStr> = passt_host_args.iter().map(OsString::as_os_str).collect();
                            let passt_process = gtk::gio::Subprocess::newv(
                                &passt_host_args_ref,
                                SubprocessFlags::empty()
                            ).expect("start of passt process");

                            wait_until_exists(&passt_socket_path).await;

                            let crosvm_bin: OsString = if is_flatpak() {
                                flatpak_host_bin("crosvm").into_os_string()
                            } else {
                                OsString::from("crosvm")
                            };
                            let wayland_sock = wayland_sock_path();
                            let vsock_cid = format!("{}", index.current_index() + 10);
                            let passt_socket_str = format!("net,socket={}", passt_socket_path.to_str().expect("string"));
                            let crosvm_host_args = make_host_args(&[
                                crosvm_bin.as_os_str(),
                                OsStr::new("run"),
                                OsStr::new("--name"),
                                OsStr::new(&vm_name),
                                OsStr::new("--cpus"),
                                OsStr::new("num-cores=4"),
                                OsStr::new("-m"),
                                OsStr::new("7000"),
                                OsStr::new("--rwdisk"),
                                image_disk_path.as_os_str(),
                                OsStr::new("--initrd"),
                                image_initrd_path.as_os_str(),
                                OsStr::new("--socket"),
                                crosvm_socket_path.as_os_str(),
                                OsStr::new("--vsock"),
                                OsStr::new(&vsock_cid),
                                OsStr::new("--gpu"),
                                OsStr::new("context-types=cross-domain,displays=[]"),
                                OsStr::new("--wayland-sock"),
                                wayland_sock.as_os_str(),
                                OsStr::new("--vhost-user"),
                                OsStr::new(&passt_socket_str),
                                OsStr::new("-p"),
                                OsStr::new("root=/dev/vda2"),
                                image_linuz_path.as_os_str(),
                            ]);
                            let crosvm_host_args_ref: Vec<&OsStr> = crosvm_host_args.iter().map(OsString::as_os_str).collect();
                            let crosvm_process = gtk::gio::Subprocess::newv(
                                &crosvm_host_args_ref,
                                SubprocessFlags::empty()
                            ).expect("start of process");

                            wait_until_ready(&vsock_socket_path).await;
                            sender.output(VmStateUpdate::Update(index.clone(), VMStatus::Running)).unwrap();
                            crosvm_process.wait_future().await.expect("vm to stop");
                            socat_process.send_signal(SIGTERM); // Marker: Incompatible with Windows
                            passt_process.send_signal(SIGTERM);
                            socat_process.wait_future().await.expect("socat to stop");
                            passt_process.wait_future().await.expect("passt to stop");
                            sender.output(VmStateUpdate::Update(index, VMStatus::NotRunning)).unwrap();
                        });
                    },
                }
            },
            VmMsg::StartTerminal(_index) => {
                relm4::spawn_local(async move {
                    request_terminal(&vsock_socket_path).await;
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
                    VmStateUpdate::Update(index, status_update  ) => AppMsg::HandleVMStatusUpdate(index, status_update),
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

        let mut model = App {
            vms,
            create_bubble_dialog,
            warn_close_dialog,
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
                    download_image().await;
                    sender.input(AppMsg::FinishImageDownload);
                });
            }
            AppMsg::FinishImageDownload => {
                self.image_status = determine_download_status();
            }
            AppMsg::HandleVMStatusUpdate(index, status_update) => {
                self.vms.guard().get_mut(index.current_index()).unwrap().value.status = status_update;
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
    let app = RelmApp::new("de.gonicus.Bubbles");
    app.run::<App>(());
}
