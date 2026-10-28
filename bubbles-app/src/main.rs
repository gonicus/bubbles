mod agent;
mod apps;
mod preferences;
mod config;

use relm4::adw::prelude::*;
use gtk::gio::SubprocessFlags;
use gtk::prelude::{BoxExt, ButtonExt, GtkWindowExt};
use relm4::prelude::{AsyncFactoryComponent, AsyncFactoryVecDeque};
use relm4::{
    AsyncFactorySender, Component, ComponentController, ComponentParts, ComponentSender,
    Controller, RelmApp, SimpleComponent, spawn
};
use relm4::factory::DynamicIndex;
use std::{env, fs, path::{Path, PathBuf}, ffi::{OsStr, OsString}};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{BorrowedFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use libc::SIGTERM;

use agent::{agent_request, Agent};
use config::{get_data_dir, vm_dir, GIB};

use apps::{exports, launcher, AppsDialog, AppsMsg};
use preferences::{BubbleSettingsDialog, BubbleSettingsMsg, BubbleSettingsOutput};



pub const APP_ID: &str = "de.gonicus.bubbles";

// static FD numbers
// stdio uses 0, 1, 2; start at 3
const GPU_VHOST_FD: i32 = 3;
const NET_VHOST_FD: i32 = 4;
const KVM_FD: i32 = 5;
const DISK_FD: i32 = 6;
const INITRD_FD: i32 = 7;
const KERNEL_FD: i32 = 8;
const AGENT_DISK_FD: i32 = 9;
const OEM_STRINGS_FD: i32 = 10;

// Mirror relevant flatpak-spawn's sandbox flag numbers
// Newer flatpak portal versions support non-magic-number flags
// TODO: Once newer flatpak versions can be expected to run everywhere (-> is in Debian Stable), re-apply b96b5d8
// OR: Refactor to use call DBUS interface directly
const SANDBOX_DISPLAY: u32 = 1;
const SANDBOX_GPU: u32 = 4;

enum SandboxNet { Denied, Shared }

fn vhost_user_pair() -> (OwnedFd, OwnedFd) {
    let (backend, frontend) = UnixStream::pair().expect("socketpair for the vhost-user link");
    (backend.into(), frontend.into())
}

// Call process using "flatpak-spawn --sandbox" and given permissions
fn spawn_sandboxed(
    flags: &[u32],
    net: SandboxNet,
    fds: Vec<(OwnedFd, i32)>,
    args: &[&OsStr],
) -> gtk::gio::Subprocess {
    let mut argv: Vec<OsString> = vec![
        "flatpak-spawn".into(),
        "--sandbox".into(),
        "--watch-bus".into(),
        "--directory=/".into(),
    ];
    argv.extend(fds.iter().map(|(_, target)| OsString::from(format!("--forward-fd={}", target))));
    if let SandboxNet::Denied = net {
        argv.push("--no-network".into());
    }
    argv.extend(flags.iter().map(|f| OsString::from(format!("--sandbox-flag={}", f))));
    argv.extend(args.iter().map(|a| (*a).to_owned()));

    let launcher = gtk::gio::SubprocessLauncher::new(SubprocessFlags::empty());
    for (fd, target) in fds {
        // SAFETY: take_fd() uses BorrowedFd only to transport an int to C
        launcher.take_fd(fd, unsafe { BorrowedFd::borrow_raw(target) });
    }
    let argv_ref: Vec<&OsStr> = argv.iter().map(OsString::as_os_str).collect();
    launcher.spawn(&argv_ref).expect("start of sandboxed process")
}

fn open_fd(path: &Path, write: bool) -> OwnedFd {
    OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .unwrap_or_else(|e| panic!("opening {}: {}", path.display(), e))
        .into()
}

// In memory rather than on the command line, which every process on the host
// can read through /proc. One string per line.
fn oem_strings_fd(strings: &[String]) -> OwnedFd {
    // SAFETY: a static NUL-terminated name; the fd is fresh and owned by File alone.
    let mut file = unsafe {
        let fd = libc::memfd_create(c"bubbles-oem-strings".as_ptr(), libc::MFD_CLOEXEC);
        assert!(fd >= 0, "memfd for the OEM strings: {}", std::io::Error::last_os_error());
        File::from_raw_fd(fd)
    };
    for string in strings {
        writeln!(file, "{string}").expect("OEM strings to fit in memory");
    }
    file.into()
}

fn wayland_sock_path() -> PathBuf {
    let runtime_dir = env::var("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
    let display = env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY");
    PathBuf::from(runtime_dir).join(display)
}

const AGENT_PORT: u16 = 11111;

// While inside the bubble, the AGENT_PORT is used for the agent, at the host side,
// claim_agent_addr delivers a free random port to be used for agent forwarding
fn claim_agent_addr() -> SocketAddr {
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .expect("a free loopback port for the agent");
    probe.local_addr().expect("the probe socket to have an address")
}

// The agent is passed as a tar on a read-only disk; the unpacking and running
// is configured via SMBIOS and kernel parameters
const AGENT_PAYLOAD_PATH: &str = "/app/share/bubbles/agent.tar";
// Pinned clear of the slots crosvm assigns itself (00:01.0 to 00:08.0)
// Avoid populating lower ends in order to prevent conflict with passt's hardcoded address
const AGENT_DISK_PCI_ADDRESS: &str = "00:10.0";

// /run is noexec in the guest, hence a tmpfs of its own to unpack into.
const AGENT_MOUNT_UNIT: &str = r"[Unit]
ConditionPathExists=!/etc/initrd-release
[Mount]
What=tmpfs
Where=/run/bubbles
Type=tmpfs
Options=mode=0755,nosuid,nodev
";

// Runs on the guest's glibc, built against the runtime's.
const AGENT_SERVICE_UNIT: &str = r"[Unit]
Description=Bubbles agent
ConditionPathExists=!/etc/initrd-release
RequiresMountsFor=/run/bubbles
BindsTo=dev-disk-by\x2did-virtio\x2dbubbles\x2dagent.device
After=dev-disk-by\x2did-virtio\x2dbubbles\x2dagent.device
[Service]
User=user
ExecStartPre=+/bin/sh -c 'tar -xf /dev/disk/by-id/virtio-bubbles-agent -C /run/bubbles'
ExecStart=/run/bubbles/bubbles-agent
Restart=on-failure
# Inherited by everything the agent starts: they run in the user's home,
# reach sommelier's Wayland socket under the runtime dir, and find the user's
# session bus, without which GApplication cannot hand over to a running
# instance and D-Bus activation fails.
WorkingDirectory=~
Environment=XDG_RUNTIME_DIR=/run/user/1000
Environment=DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
LoadCredential=bubbles.agent-token
# A system service never sees the login shell's PATH and XDG_DATA_DIRS. GIO
# lists the applications from these, and the ones it starts inherit them.
Environment=PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/home/user/.nix-profile/bin
Environment=XDG_DATA_DIRS=/home/user/.nix-profile/share:/home/user/.local/share/flatpak/exports/share:/var/lib/flatpak/exports/share:/usr/local/share:/usr/share
# Applications the agent starts run under sommelier, which offers
# Wayland and no X server. Qt5 in particular still defaults to X11.
Environment=WAYLAND_DISPLAY=wayland-0
Environment=XDG_SESSION_TYPE=wayland
Environment=QT_QPA_PLATFORM=wayland
Environment=ELECTRON_OZONE_PLATFORM_HINT=auto
Environment=SDL_VIDEODRIVER=wayland
";

// Sommelier, a user service of the image, prefixes guest windows' app_ids with
// its VM id. Set to the bubble's name, launchers from different bubbles do not
// claim each other's windows; see exports::host_app_id. tmpfiles expands the
// \n, and bubble names hold no % it would expand too.
fn sommelier_vm_id_tmpfiles(bubble: &str) -> String {
    format!(
        "d /run/systemd/user 0755 root root -\n\
         d /run/systemd/user/sommelier.service.d 0755 root root -\n\
         f+ /run/systemd/user/sommelier.service.d/bubbles.conf 0644 root root - \
         [Service]\\nEnvironment=SOMMELIER_VM_IDENTIFIER={bubble}\\n\n"
    )
}

fn credential_oem_string(name: &str, content: &str) -> String {
    format!("io.systemd.credential.binary:{}={}", name, gtk::glib::base64_encode(content.as_bytes()))
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

    if images_dir.join("debian-13/disk.img").exists() {
        ImageStatus::Present
    } else {
        ImageStatus::NotPresent
    }
}

// Launcher ids embed the name as it is; see exports::desktop_file_id.
fn is_valid_bubble_name(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub async fn wait_until_ready(agent: &Agent) {
    loop {
        match tokio::time::timeout(std::time::Duration::from_secs(2), agent_request(agent, reqwest::Method::GET, "/ready", None)).await {
            Ok(Ok((status, _))) if status.is_success() => return,
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
}

async fn agent_post(agent: &Agent, path: &str) -> bool {
    matches!(
        agent_request(agent, reqwest::Method::POST, path, None).await,
        Ok((status, _)) if status.is_success()
    )
}

// Pinned VM image release. Bump both when publishing a new vm-image-* release:
// VM_IMAGE_SHA256 is the sha256 of the release's disk.tar.gz asset.
const VM_IMAGE_TAG: &str = "vm-image-v1.1.0";
const VM_IMAGE_SHA256: &str = "d553da66685a58a83a5f2b70224e36a8bdca0955a5cdeabaeffd129f0034c317";

// Run a subprocess to completion, returning an error instead of panicking so
// download failures can be surfaced without leaving the UI stuck.
async fn run_checked(argv: &[&OsStr], flags: SubprocessFlags) -> Result<(), String> {
    let name = argv.first().map(|a| a.to_string_lossy().into_owned()).unwrap_or_default();
    let proc = gtk::gio::Subprocess::newv(argv, flags)
        .map_err(|e| format!("failed to start {name}: {e}"))?;
    proc.wait_future().await
        .map_err(|e| format!("{name} did not complete: {e}"))?;
    if proc.is_successful() {
        Ok(())
    } else {
        Err(format!("{name} exited with a non-zero status"))
    }
}

async fn download_image() {
    let target_dir = get_data_dir().join("images/debian-13");
    let tarball_path = target_dir.join("disk.tar.gz");
    let checkfile_path = target_dir.join("disk.tar.gz.sha256");
    let raw_path = target_dir.join("disk.img");

    let result: Result<(), String> = async {
        tokio::fs::create_dir_all(&target_dir).await
            .map_err(|e| format!("could not create image directory: {e}"))?;

        let url = format!(
            "https://github.com/gonicus/bubbles/releases/download/{}/disk.tar.gz",
            VM_IMAGE_TAG,
        );

        // Step 1: download the release tarball. curl and tar are provided by the
        // Flatpak runtime; --share=network is already granted, so this runs
        // inside the sandbox without flatpak-spawn.
        run_checked(&[
            OsStr::new("curl"),
            OsStr::new("-L"),
            OsStr::new("--fail"),
            OsStr::new("-o"),
            tarball_path.as_os_str(),
            OsStr::new(url.as_str()),
        ], SubprocessFlags::empty()).await?;

        // Step 2: verify integrity against the pinned checksum before extracting.
        tokio::fs::write(
            &checkfile_path,
            format!("{}  {}\n", VM_IMAGE_SHA256, tarball_path.display()),
        ).await.map_err(|e| format!("could not write checksum file: {e}"))?;
        run_checked(&[
            OsStr::new("sha256sum"),
            OsStr::new("-c"),
            checkfile_path.as_os_str(),
        ], SubprocessFlags::STDOUT_SILENCE).await?;

        // Step 3: extract disk.img, vmlinuz and initrd.img into the target dir.
        run_checked(&[
            OsStr::new("tar"),
            OsStr::new("-xzf"),
            tarball_path.as_os_str(),
            OsStr::new("-C"),
            target_dir.as_os_str(),
        ], SubprocessFlags::empty()).await?;

        Ok(())
    }.await;

    // Best-effort cleanup of intermediates; on failure also drop any partial
    // disk image so it isn't later misread as "Ready".
    tokio::fs::remove_file(&tarball_path).await.ok();
    tokio::fs::remove_file(&checkfile_path).await.ok();
    if let Err(e) = result {
        eprintln!("image download failed: {e}");
        tokio::fs::remove_file(&raw_path).await.ok();
    }
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
                set_description: Some("Enter name and confirm with ENTER (alphanumeric and hyphens only)"),
                #[wrap(Some)]
                set_child = &gtk::Entry {
                    connect_changed => move |entry| {
                        if is_valid_bubble_name(&entry.text()) {
                            entry.remove_css_class("error");
                        } else {
                            entry.add_css_class("error");
                        }
                    },
                    connect_activate[sender] => move |entry| {
                        let name: String = entry.text().into();
                        if !is_valid_bubble_name(&name) {
                            return;
                        }
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
    apps_dialog: Controller<AppsDialog>,
    currently_creating_bubble: bool,
    image_status: ImageStatus,
    root: relm4::adw::Window,
}

#[derive(PartialEq, Debug, Clone)]
enum VMStatus {
    NotRunning,
    /// Ready, with where its agent listens. The address and token are made
    /// at start time, so this is the only place that knows them.
    Running(Agent),
    InFlux,
}

impl VMStatus {
    fn agent(&self) -> Option<&Agent> {
        match self {
            VMStatus::Running(agent) => Some(agent),
            _ => None,
        }
    }
}

#[derive(PartialEq, Debug, Clone)]
struct VM {
    name: String,
    status: VMStatus,
}

fn load_vms() -> Vec<VM> {
    let vms_dir = get_data_dir().join("vms");
    fs::create_dir_all(&vms_dir).expect("directory to exist or be created");
    fs::read_dir(vms_dir)
        .expect("to exist")
        .map(|dir| VM {
            name: dir
                .expect("to exist")
                .file_name()
                .into_string()
                .expect("path to be serializable"),
            status: VMStatus::NotRunning,
        })
        .collect()
}

async fn create_vm(name: String) {
    let vm_dir_path = vm_dir(&name);
    tokio::fs::create_dir_all(&vm_dir_path).await.expect("directories to be created");
    let image_base_path = get_data_dir().join("images/debian-13");
    let image_disk_path = image_base_path.join("disk.img");
    let image_linuz_path = image_base_path.join("vmlinuz");
    let image_initrd_path = image_base_path.join("initrd.img");
    println!("copy disk.img...");
    tokio::fs::copy(image_disk_path, vm_dir_path.join("disk.img")).await.expect("disk copy to succeed");
    println!("copy vmlinuz...");
    tokio::fs::copy(image_linuz_path, vm_dir_path.join("vmlinuz")).await.expect("vmlinuz copy to succeed");
    println!("copy initrd.img...");
    tokio::fs::copy(image_initrd_path, vm_dir_path.join("initrd.img")).await.expect("initrd copy to succeed");

    println!("resize disk.img...");
    let f = tokio::fs::OpenOptions::new().write(true).open(vm_dir_path.join("disk.img")).await
        .expect("open image");
    let current_size = f.metadata().await.expect("stat image").len();
    f.set_len(current_size + 15 * GIB).await.expect("resize image");

    println!("save config...");
    config::save_config(&name, &config::BubbleConfig::default());
    println!("done copy");
}

#[derive(Debug)]
enum VmMsg {
    PowerToggle(DynamicIndex),
    StartTerminal(DynamicIndex),
    OpenSettings(DynamicIndex),
    OpenApplications,
}

#[derive(Debug)]
enum VmStateUpdate {
    Update(DynamicIndex, VMStatus),
    OpenSettings(String),
    OpenApplications(String),
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
            add_suffix = &relm4::adw::Spinner {
                #[watch]
                set_visible: self.value.status == VMStatus::InFlux,
            },
            add_suffix = &gtk::Box {
                set_orientation: gtk::Orientation::Horizontal,
                set_css_classes: &["linked"],
                append = &gtk::Button {
                    #[watch]
                    set_sensitive: self.value.status == VMStatus::NotRunning,
                    set_icon_name: "applications-system-symbolic",
                    set_tooltip_text: Some("Settings"),
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::OpenSettings(index.clone()));
                    }
                },
                append = &gtk::Button {
                    #[watch]
                    set_sensitive: self.value.status != VMStatus::InFlux,
                    #[watch]
                    set_icon_name: match self.value.status {
                        VMStatus::NotRunning => "media-playback-start-symbolic",
                        VMStatus::Running(_) | VMStatus::InFlux => "media-playback-stop-symbolic",
                    },
                    #[watch]
                    set_css_classes: match self.value.status {
                        VMStatus::Running(_) => &["destructive-action"],
                        _ => &[],
                    },
                    set_tooltip_text: Some("Power"),
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::PowerToggle(index.clone()));
                    }
                },
                append = &gtk::Button {
                    #[watch]
                    set_sensitive: self.value.status.agent().is_some(),
                    set_icon_name: "utilities-terminal-symbolic",
                    set_tooltip_text: Some("Start terminal emulator instance"),
                    connect_clicked[sender, index] => move |_| {
                        sender.input(VmMsg::StartTerminal(index.clone()));
                    }
                },
                append = &gtk::Button {
                    #[watch]
                    set_sensitive: self.value.status.agent().is_some(),
                    set_icon_name: "view-grid-symbolic",
                    set_tooltip_text: Some("Applications"),
                    connect_clicked[sender] => move |_| {
                        sender.input(VmMsg::OpenApplications);
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
        let image_base_path = vm_dir(&vm_name);
        match msg {
            VmMsg::OpenSettings(_index) => {
                sender.output(VmStateUpdate::OpenSettings(vm_name)).unwrap();
            },
            VmMsg::OpenApplications => {
                sender.output(VmStateUpdate::OpenApplications(vm_name)).unwrap();
            },
            VmMsg::PowerToggle(index) => {
                match &self.value.status {
                    VMStatus::Running(agent) => {
                        let agent = agent.clone();
                        relm4::spawn_local(async move {
                            if agent_post(&agent, "/shutdown").await {
                                sender.output(VmStateUpdate::Update(index, VMStatus::InFlux)).unwrap();
                            }
                        });
                    },
                    VMStatus::InFlux => {},
                    VMStatus::NotRunning => {
                        let agent = Agent::new(claim_agent_addr());
                        sender.output(VmStateUpdate::Update(index.clone(), VMStatus::InFlux)).unwrap();
                        relm4::spawn_local(async move {
                            let config = config::load_config(&vm_name);
                            let image_disk_path = image_base_path.join("disk.img");
                            let image_linuz_path = image_base_path.join("vmlinuz");
                            let image_initrd_path = image_base_path.join("initrd.img");

                            let (net_backend_fd, net_frontend_fd) = vhost_user_pair();
                            let net_fd_arg = format!("{}", NET_VHOST_FD);
                            let mut passt_args: Vec<&OsStr> = vec![
                                OsStr::new("/app/bin/passt"),
                                OsStr::new("-f"),
                                OsStr::new("--vhost-user"),
                                OsStr::new("--fd"),
                                OsStr::new(&net_fd_arg),
                            ];
                            // Bound to this bubble's own port, which keeps it
                            // clear of the other bubbles and of the host's
                            // services, and off every interface but loopback.
                            let agent_forward = format!(
                                "{}/{}:{}", agent.addr.ip(), agent.addr.port(), AGENT_PORT,
                            );
                            passt_args.push(OsStr::new("--tcp-ports"));
                            passt_args.push(OsStr::new(&agent_forward));
                            let user_forwards = format!("127.0.0.1/{}", config.tcp_ports.join(","));
                            if !config.tcp_ports.is_empty() {
                                passt_args.push(OsStr::new("--tcp-ports"));
                                passt_args.push(OsStr::new(&user_forwards));
                            }
                            passt_args.push(OsStr::new("--map-host-loopback"));
                            if config.map_host_loopback {
                                passt_args.push(OsStr::new("169.254.0.1"));
                            } else {
                                passt_args.push(OsStr::new("none"));
                            }
                            passt_args.push(OsStr::new("--dns-forward"));
                            passt_args.push(OsStr::new("169.254.0.1"));
                            let passt_process = spawn_sandboxed(
                                &[],
                                SandboxNet::Shared,
                                vec![(net_backend_fd, NET_VHOST_FD)],
                                &passt_args,
                            );

                            let wayland_sock = wayland_sock_path();

                            let (gpu_backend_fd, gpu_frontend_fd) = vhost_user_pair();
                            let gpu_fd_arg = format!("--fd={}", GPU_VHOST_FD);
                            let gpu_process = spawn_sandboxed(
                                &[SANDBOX_DISPLAY, SANDBOX_GPU],
                                SandboxNet::Denied,
                                vec![(gpu_backend_fd, GPU_VHOST_FD)],
                                &[
                                    OsStr::new("/app/bin/crosvm"),
                                    OsStr::new("device"),
                                    OsStr::new("gpu"),
                                    OsStr::new(&gpu_fd_arg),
                                    OsStr::new("--wayland-sock"),
                                    wayland_sock.as_os_str(),
                                    OsStr::new("--params"),
                                    OsStr::new(r#"{"context-types":"cross-domain","displays":[]}"#),
                                ],
                            );

                            let kvm_fd = open_fd(Path::new("/dev/kvm"), true);
                            let disk_fd = open_fd(&image_disk_path, true);
                            let initrd_fd = open_fd(&image_initrd_path, false);
                            let kernel_fd = open_fd(&image_linuz_path, false);
                            let agent_disk_fd = open_fd(Path::new(AGENT_PAYLOAD_PATH), false);
                            let oem_strings_fd = oem_strings_fd(&[
                                credential_oem_string("systemd.extra-unit.run-bubbles.mount", AGENT_MOUNT_UNIT),
                                credential_oem_string("systemd.extra-unit.bubbles-agent.service", AGENT_SERVICE_UNIT),
                                credential_oem_string("bubbles.agent-token", &agent.token),
                                credential_oem_string("tmpfiles.extra", &sommelier_vm_id_tmpfiles(&vm_name)),
                            ]);

                            // Pinning pci address to match image's enp0s7
                            let passt_socket_str = format!("net,socket=/proc/self/fd/{},pci-address=00:07.0", NET_VHOST_FD);
                            let gpu_socket_str = format!("gpu,socket=/proc/self/fd/{}", GPU_VHOST_FD);
                            let hypervisor_str = format!("kvm[device=/proc/self/fd/{}]", KVM_FD);
                            let disk_str = format!("/proc/self/fd/{}", DISK_FD);
                            let initrd_str = format!("/proc/self/fd/{}", INITRD_FD);
                            let kernel_str = format!("/proc/self/fd/{}", KERNEL_FD);
                            let cpus_str = format!("num-cores={}", config.cpus);
                            let ram_str = format!("{}", config.ram_mb);
                            let hostname_param = format!("systemd.hostname={}", vm_name);
                            let agent_disk_str = format!(
                                "/proc/self/fd/{},ro=true,id=bubbles-agent,pci-address={}",
                                AGENT_DISK_FD, AGENT_DISK_PCI_ADDRESS,
                            );
                            let smbios_str = format!("oem-strings-file=/proc/self/fd/{}", OEM_STRINGS_FD);
                            let crosvm_args: Vec<&OsStr> = vec![
                                OsStr::new("/app/bin/crosvm"),
                                OsStr::new("run"),
                                OsStr::new("--name"),
                                OsStr::new(&vm_name),
                                OsStr::new("--cpus"),
                                OsStr::new(&cpus_str),
                                OsStr::new("-m"),
                                OsStr::new(&ram_str),
                                OsStr::new("--hypervisor"),
                                OsStr::new(&hypervisor_str),
                                OsStr::new("--rwdisk"),
                                OsStr::new(&disk_str),
                                OsStr::new("--block"),
                                OsStr::new(&agent_disk_str),
                                OsStr::new("--smbios"),
                                OsStr::new(&smbios_str),
                                OsStr::new("--initrd"),
                                OsStr::new(&initrd_str),
                                // Sandboxing implemented using flatpak sandboxing instead
                                OsStr::new("--disable-sandbox"),
                                OsStr::new("--vhost-user"),
                                OsStr::new(&gpu_socket_str),
                                OsStr::new("--vhost-user"),
                                OsStr::new(&passt_socket_str),
                                OsStr::new("-p"),
                                OsStr::new("root=/dev/vda2"),
                                OsStr::new("-p"),
                                OsStr::new(&hostname_param),
                                OsStr::new("-p"),
                                OsStr::new("systemd.wants=bubbles-agent.service"),
                                // The image's own agent and its proxies, for this boot only:
                                // started by an older app, the bubble still gets them.
                                OsStr::new("-p"),
                                OsStr::new("systemd.mask=bubbles.service"),
                                OsStr::new("-p"),
                                OsStr::new("systemd.mask=bubbles-proxy.service"),
                                OsStr::new("-p"),
                                OsStr::new("systemd.mask=bubbles-proxy-2.service"),
                                OsStr::new(&kernel_str),
                            ];
                            let crosvm_process = spawn_sandboxed(
                                &[],
                                SandboxNet::Denied,
                                vec![
                                    (gpu_frontend_fd, GPU_VHOST_FD),
                                    (net_frontend_fd, NET_VHOST_FD),
                                    (kvm_fd, KVM_FD),
                                    (disk_fd, DISK_FD),
                                    (initrd_fd, INITRD_FD),
                                    (kernel_fd, KERNEL_FD),
                                    (agent_disk_fd, AGENT_DISK_FD),
                                    (oem_strings_fd, OEM_STRINGS_FD),
                                ],
                                &crosvm_args,
                            );

                            wait_until_ready(&agent).await;
                            sender.output(VmStateUpdate::Update(index.clone(), VMStatus::Running(agent))).unwrap();
                            crosvm_process.wait_future().await.expect("vm to stop");
                            passt_process.send_signal(SIGTERM); // Marker: Incompatible with Windows
                            gpu_process.send_signal(SIGTERM);
                            passt_process.wait_future().await.expect("passt to stop");
                            gpu_process.wait_future().await.expect("gpu device to stop");
                            sender.output(VmStateUpdate::Update(index, VMStatus::NotRunning)).unwrap();
                        });
                    },
                }
            },
            VmMsg::StartTerminal(_index) => {
                let Some(agent) = self.value.status.agent().cloned() else { return };
                relm4::spawn_local(async move {
                    agent_post(&agent, "/spawn-terminal").await;
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
    OpenBubbleApplications(String),
    DeleteBubble(String),
    StartApplication(launcher::StartRequest),
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
                    pack_end = &relm4::adw::Spinner{
                        #[watch]
                        set_visible: model.currently_creating_bubble
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
                        set_name: Some("images"),
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
                        set_name: Some("bubbles"),
                        set_title: Some("Bubbles"),
                        set_icon_name: Some("computer-symbolic"),
                    },
                    set_visible_child_name: "bubbles",
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
                    VmStateUpdate::OpenApplications(name) => AppMsg::OpenBubbleApplications(name),
                });
        let create_bubble_dialog = CreateBubbleDialog::builder()
            .launch(())
            .forward(sender.input_sender(), std::convert::identity);
        let warn_close_dialog = WarnCloseDialog::builder()
            .launch(())
            .forward(sender.input_sender(), std::convert::identity);
        let settings_dialog = BubbleSettingsDialog::builder()
            .launch(())
            .forward(sender.input_sender(), |output| match output {
                BubbleSettingsOutput::DeleteBubble(name) => AppMsg::DeleteBubble(name),
            });
        let apps_dialog = AppsDialog::builder().launch(()).detach();
        let launch_sender = sender.input_sender().clone();
        launcher::connect(move |request| launch_sender.emit(AppMsg::StartApplication(request)));

        let mut model = App {
            vms,
            create_bubble_dialog,
            warn_close_dialog,
            settings_dialog,
            apps_dialog,
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
                let existing_names: Vec<String> = self.vms.guard()
                    .iter()
                    .filter_map(|entry| entry.map(|e| e.value.name.clone()))
                    .collect();

                if existing_names.contains(&name) {
                    println!("vm already exists. Skipping");
                    return;
                }
                self.currently_creating_bubble = true;
                spawn(async move {
                    create_vm(name).await;
                    sender.input(AppMsg::FinishBubbleCreation);
                });
            }
            AppMsg::FinishBubbleCreation=>{
                self.currently_creating_bubble = false;
                let mut guard = self.vms.guard();
                let existing_names: Vec<String> = guard
                    .iter()
                    .filter_map(|entry| entry.map(|e| e.value.name.clone()))
                    .collect();
                for vm in load_vms() {
                    if !existing_names.contains(&vm.name) {
                        guard.push_back(vm);
                    }
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
                let agent = status_update.agent().cloned();
                let mut guard = self.vms.guard();
                let vm = &mut guard.get_mut(index.current_index()).unwrap().value;
                vm.status = status_update;
                self.apps_dialog.emit(AppsMsg::AgentChanged(vm.name.clone(), agent));
            }
            AppMsg::OpenBubbleSettings(name) => {
                self.settings_dialog.sender().send(BubbleSettingsMsg::Load(name)).unwrap();
                self.settings_dialog.widgets().dialog.present(Some(&self.root));
            }
            AppMsg::OpenBubbleApplications(name) => {
                let agent = self.agent_of(&name);
                self.apps_dialog.emit(AppsMsg::Load(name, agent));
                self.apps_dialog.widgets().dialog.present(Some(&self.root));
            }
            AppMsg::DeleteBubble(name) => {
                relm4::spawn_local(apps::uninstall_launchers(exports::load(&name)));
                let _ = fs::remove_dir_all(vm_dir(&name));
                let position = self.vms.iter()
                    .position(|entry| entry.is_some_and(|entry| entry.value.name == name));
                if let Some(index) = position {
                    self.vms.guard().remove(index);
                }
            }
            AppMsg::CloseApplication => {
                let vm_running = self.vms.iter()
                    .flatten()
                    .any(|entry| entry.value.status != VMStatus::NotRunning);
                if self.image_status == ImageStatus::Downloading || self.currently_creating_bubble || vm_running {
                    self.warn_close_dialog.widgets().dialog.present(Some(&self.root));
                    return
                }

                relm4::main_application().quit();
            }
            AppMsg::StartApplication(request) => {
                let agent = self.agent_of(&request.bubble);
                launcher::answer(request, agent);
            }
        }
    }
}

impl App {
    fn agent_of(&self, bubble: &str) -> Option<Agent> {
        self.vms
            .iter()
            .flatten()
            .find(|entry| entry.value.name == bubble)
            .and_then(|entry| entry.value.status.agent().cloned())
    }
}

fn main() {
    let app = RelmApp::new(APP_ID);
    launcher::setup(&relm4::main_application());
    app.run::<App>(());
}
