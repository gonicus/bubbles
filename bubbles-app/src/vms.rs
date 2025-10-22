use std::{env, ffi::OsStr, fs, path::Path};

use adw::prelude::ActionRowExt;
use gtk::{
    gio::{Subprocess, SubprocessFlags}, glib, prelude::*
};

#[derive(PartialEq, Debug, Clone)]
pub enum VMStatus {
    NotRunning,
    Running,
    InFlux,
}

#[derive(PartialEq, Debug)]
pub struct VM {
    name: String,
    status: VMStatus,
}

pub fn load_vms() -> Vec<VM> {
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
            status: determine_running_status(vm_name),
        });
    }
    return vms;
}

pub async fn wait_until_exists(path: &OsStr) {
    loop {
        let process = Subprocess::newv(
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
        let process = Subprocess::newv(
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

pub fn set_running_status_on_button(
    power_button: &gtk::Button,
    terminal_button: &gtk::Button,
    label: &gtk::Label,
    vm_status: VMStatus,
) {
    if vm_status == VMStatus::Running {
        label.set_label("Running");
        power_button.set_sensitive(true);
        power_button.set_icon_name("system-shutdown-symbolic");
        power_button.set_tooltip_text(Some("Stop"));
        terminal_button.set_sensitive(true);
    } else if vm_status == VMStatus::NotRunning {
        label.set_label("Not Running");
        power_button.set_sensitive(true);
        power_button.set_icon_name("system-shutdown-symbolic");
        power_button.set_tooltip_text(Some("Start"));
        terminal_button.set_sensitive(false);
    } else if vm_status == VMStatus::InFlux {
        label.set_label("...");
        power_button.set_sensitive(false);
        power_button.set_icon_name("image-loading-symbolic");
        power_button.set_tooltip_text(Some(""));
        terminal_button.set_sensitive(false);
    }
}

pub fn determine_running_status(vm_name: String) -> VMStatus {
    let vm_dir = env::current_dir()
        .expect("cwd to be set")
        .join(Path::new(".bubbles/vms"))
        .join(vm_name);

    let socket_exists = vm_dir
        .join(Path::new("crosvm_socket"))
        .exists();

    return match socket_exists {
        true => VMStatus::Running,
        false => VMStatus::NotRunning,
    };
}

pub fn build_vms_list(vms: Vec<VM>) -> gtk::Widget {
    if vms.len() == 0 {
        let statuspage = adw::StatusPage::builder()
            .title("No Bubbles here, yet")
            .icon_name("computer")
            .build();
        let create_button = &gtk::Button::builder()
            .css_classes(["pill", "suggested-action"])
            .label("Create Bubble 'development'")
            .build();
        create_button.connect_clicked(|button| {
            // TODO Do not run in main thread, refresh UI after success
            create_vm();
            button.set_label("Restart, pls!");
        });
        statuspage.set_child(Some(create_button));
        return statuspage.into();
    }
    let vm_list = gtk::ListBox::builder().build();
    for vm in vms {
        let entry = adw::ActionRow::builder().title(vm.name.clone()).build();
        entry.add_prefix(&gtk::Image::from_icon_name("computer-symbolic"));
        let interaction_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let power_button = gtk::Button::builder()
            .icon_name("system-shutdown-symbolic")
            .build();
        let terminal_button = gtk::Button::builder()
            .icon_name("utilities-terminal-symbolic")
            .build();
        let vm_name_term = vm.name.clone();
        let vm_name_power = vm.name.clone();
        let status_label = &gtk::Label::new(Some(""));
        interaction_box.append(status_label);
        interaction_box.append(&power_button);
        interaction_box.append(&terminal_button);
        set_running_status_on_button(
            &power_button,
            &terminal_button,
            &status_label,
            determine_running_status(vm.name),
        );
        entry.add_suffix(&interaction_box);
        terminal_button.connect_clicked(move |_button| {
            glib::spawn_future_local(glib::clone!(
                #[strong]
                vm_name_term,
                async move {
                    let image_base_path = env::current_dir()
                        .expect("to be set")
                        .join(".bubbles/vms").join(vm_name_term.clone());
                    let vsock_socket_path = image_base_path.join("vsock");
                    Subprocess::newv(
                        &[
                            OsStr::new("curl"),
                            OsStr::new("-XPOST"),
                            OsStr::new("--unix-socket"),
                            vsock_socket_path.as_os_str(),
                            OsStr::new("http://localhost/spawn-terminal"),
                        ],
                        SubprocessFlags::empty()
                    ).expect("start of process").wait_future().await.expect("curl to connect");
                }
            ));
        });
        let (power_button_sender, power_button_receiver) = async_channel::bounded(1);
        power_button.connect_clicked(move |_button| {
            glib::spawn_future_local(glib::clone!(
                #[strong]
                power_button_sender,
                #[strong]
                vm_name_power,
                async move {
                    if determine_running_status(vm_name_power.clone()) == VMStatus::NotRunning {
                        power_button_sender.send(VMStatus::InFlux).await.expect("channel to be open");
                        let image_base_path = env::current_dir()
                            .expect("to be set")
                            .join(".bubbles/vms").join(vm_name_power.clone());
                        let crosvm_socket_path = image_base_path.join("crosvm_socket");
                        let vsock_socket_path = image_base_path.join("vsock");
                        let passt_socket_path = Path::new("/tmp").join(format!("passt_socket_{}", vm_name_power.clone()));
                        let image_disk_path = image_base_path.join("disk.img");
                        let image_linuz_path = image_base_path.join("vmlinuz");
                        let image_initrd_path = image_base_path.join("initrd.img");
                        Subprocess::newv(
                            &[
                                OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/socat").as_os_str()),
                                OsStr::new(&format!("UNIX-LISTEN:{},fork", vsock_socket_path.to_str().expect("string"))),
                                OsStr::new("VSOCK-CONNECT:3:11111"),
                            ],
                            SubprocessFlags::empty()
                        ).expect("start of socat process");
                        Subprocess::newv(
                            &[
                                OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/passt").as_os_str()),
                                OsStr::new("-f"),
                                OsStr::new("--vhost-user"),
                                OsStr::new("--socket"),
                                OsStr::new(passt_socket_path.as_os_str()),
                            ],
                            SubprocessFlags::empty()
                        ).expect("start of passt process");
                        wait_until_exists(passt_socket_path.as_os_str()).await;
                        let crosvm_process = Subprocess::newv(
                            &[
                                OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/crosvm").as_os_str()),
                                OsStr::new("run"),
                                OsStr::new("--name"),
                                OsStr::new(&vm_name_power),
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
                                OsStr::new("3"),
                                OsStr::new("--gpu"),
                                OsStr::new("context-types=cross-domain,displays=[]"),
                                OsStr::new("--wayland-sock"),
                                OsStr::new(Path::new(&env::var("XDG_RUNTIME_DIR").expect("XDG var to be set")).join(env::var("WAYLAND_DISPLAY").expect("WAYLAND_DISPLAY var to be set")).as_os_str()),
                                OsStr::new("--vhost-user"),
                                OsStr::new(&format!("net,socket={}", passt_socket_path.to_str().expect("string"))),
                                OsStr::new("-p"),
                                OsStr::new("root=/dev/vda2"),
                                image_linuz_path.as_os_str(),
                            ],
                            SubprocessFlags::empty()
                        ).expect("start of process");
                        wait_until_ready(vsock_socket_path.as_os_str()).await;
                        power_button_sender.send(VMStatus::Running).await.expect("channel to be open");
                        crosvm_process.wait_future().await.expect("vm to stop");
                        power_button_sender.send(VMStatus::NotRunning).await.expect("channel to be open");
                    } else {
                        power_button_sender.send(VMStatus::InFlux).await.expect("channel to be open");
                        let socket_path = env::current_dir()
                            .expect("to be set")
                            .join(".bubbles/vms").join(vm_name_power.clone()).join("crosvm_socket");
                        Subprocess::newv(
                            &[
                                OsStr::new(Path::new(&env::var("HOME").expect("HOME var to be set")).join("bubbles/crosvm").as_os_str()),
                                OsStr::new("stop"),
                                socket_path.as_os_str(),
                            ],
                            SubprocessFlags::empty()
                        ).expect("start of process").wait_future().await.expect("vm to stop");
                        power_button_sender.send(VMStatus::NotRunning).await.expect("channel to be open");
                    }
                }
            ));
        });
        glib::spawn_future_local(glib::clone!(
            #[weak]
            power_button,
            #[weak]
            terminal_button,
            #[weak]
            status_label,
            async move {
                while let Ok(status) = power_button_receiver.recv().await {
                    set_running_status_on_button(&power_button, &terminal_button, &status_label, status);
                }
            }
        ));
        vm_list.append(&entry);
    }
    return vm_list.into();
}

fn create_vm() {
    println!("starting copy");
    let vm_dir_path = &env::current_dir()
        .expect("to be set")
        .join(".bubbles/vms/development");
    fs::create_dir_all(vm_dir_path).expect("directories to be created");
    let image_base_path = env::current_dir()
        .expect("to be set")
        .join(".bubbles/images/debian-13");
    let image_disk_path = image_base_path.join("disk.img");
    let image_linuz_path = image_base_path.join("vmlinuz");
    let image_initrd_path = image_base_path.join("initrd.img");
    fs::copy(image_disk_path, vm_dir_path.join("disk.img")).expect("disk copy to succeed");
    fs::copy(image_linuz_path, vm_dir_path.join("vmlinuz")).expect("vmlinuz copy to succeed");
    fs::copy(image_initrd_path, vm_dir_path.join("initrd.img")).expect("initrd copy to succeed");
    println!("done copy");
}
