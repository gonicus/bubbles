use relm4::adw::prelude::*;
use gtk::prelude::{ButtonExt, EditableExt};
use relm4::factory::{DynamicIndex, FactoryVecDeque};
use relm4::prelude::FactoryComponent;
use relm4::{ComponentParts, ComponentSender, FactorySender, SimpleComponent};
use std::fs;

use crate::{BubbleConfig, load_config, save_config};

fn host_cpu_count() -> u32 {
    (unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) } as u32).max(1)
}

fn host_ram_mb() -> u32 {
    let content = fs::read_to_string("/proc/meminfo").unwrap_or_default();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb: u64 = rest.trim().split_whitespace().next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(32768 * 1024);
            return (kb / 1024) as u32;
        }
    }
    32768
}

// --- Port Entry Factory Component ---

fn parse_port_or_range(s: &str, min: u16) -> bool {
    let s = s.trim();
    if let Ok(p) = s.parse::<u16>() {
        return p >= min;
    }
    if let Some((a, b)) = s.split_once('-') {
        if let (Ok(lo), Ok(hi)) = (a.trim().parse::<u16>(), b.trim().parse::<u16>()) {
            return lo >= min && hi >= lo;
        }
    }
    false
}

fn is_valid_port_entry(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() { return true; }
    if let Some((src, dst)) = s.split_once(':') {
        return parse_port_or_range(src, 1024) && parse_port_or_range(dst, 1);
    }
    parse_port_or_range(s, 1024)
}

#[derive(Debug)]
struct PortEntry {
    text: String,
    valid: bool,
}

#[derive(Debug)]
enum PortEntryMsg {
    TextChanged(String),
}

#[derive(Debug)]
enum PortEntryOutput {
    Remove(DynamicIndex),
}

#[relm4::factory]
impl FactoryComponent for PortEntry {
    type Init = String;
    type Input = PortEntryMsg;
    type Output = PortEntryOutput;
    type CommandOutput = ();
    type ParentWidget = gtk::ListBox;

    view! {
        #[root]
        relm4::adw::EntryRow {
            set_title: "Port, range, or mapping (e.g. 8080, 8080-8090, 2222:22)",
            set_text: &self.text,
            #[watch]
            set_css_classes: if self.valid { &[] } else { &["error"] },
            add_suffix = &gtk::Button {
                set_icon_name: "user-trash-symbolic",
                set_valign: gtk::Align::Center,
                connect_clicked[sender, index] => move |_| {
                    sender.output(PortEntryOutput::Remove(index.clone())).unwrap();
                }
            },
            connect_changed[sender] => move |entry| {
                sender.input(PortEntryMsg::TextChanged(entry.text().to_string()));
            },
        }
    }

    fn init_model(text: Self::Init, _index: &DynamicIndex, _sender: FactorySender<Self>) -> Self {
        let valid = is_valid_port_entry(&text);
        Self { text, valid }
    }

    fn update(&mut self, msg: Self::Input, _sender: FactorySender<Self>) {
        match msg {
            PortEntryMsg::TextChanged(text) => {
                self.valid = is_valid_port_entry(&text);
                self.text = text;
            }
        }
    }
}

// --- Bubble Settings Dialog ---

pub struct BubbleSettingsDialog {
    vm_name: String,
    cpu_row: relm4::adw::SpinRow,
    ram_row: relm4::adw::SpinRow,
    loopback_row: relm4::adw::SwitchRow,
    ports: FactoryVecDeque<PortEntry>,
}

#[derive(Debug)]
pub enum BubbleSettingsMsg {
    Load(String),
    Save,
    AddPort,
    RemovePort(DynamicIndex),
}

#[allow(unused)]
#[relm4::component(pub)]
impl SimpleComponent for BubbleSettingsDialog {
    type Init = ();
    type Input = BubbleSettingsMsg;
    type Output = ();

    view! {
        dialog = relm4::adw::PreferencesDialog {
            set_title: "Bubble Settings",
            set_content_height: 550,
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
                    set_title: "Network",
                    #[local_ref]
                    add = loopback_row -> relm4::adw::SwitchRow {
                        set_title: "Map Host Loopback",
                        set_subtitle: "Make host services reachable at 169.254.0.1",
                    },
                },
                add = &relm4::adw::PreferencesGroup {
                    set_title: "TCP Port Forwards",
                    set_description: Some("Applied on next startup"),
                    #[wrap(Some)]
                    set_header_suffix = &gtk::Button {
                        set_icon_name: "list-add-symbolic",
                        set_valign: gtk::Align::Center,
                        connect_clicked => BubbleSettingsMsg::AddPort,
                    },
                    #[local_ref]
                    add = ports_listbox -> gtk::ListBox {},
                },
            },
        }
    }

    fn init(
        _init: Self::Init,
        root: Self::Root,
        sender: ComponentSender<Self>,
    ) -> ComponentParts<Self> {
        let cpu_row = relm4::adw::SpinRow::with_range(1.0, host_cpu_count() as f64, 1.0);
        let ram_row = relm4::adw::SpinRow::with_range(512.0, host_ram_mb() as f64, 512.0);
        let loopback_row = relm4::adw::SwitchRow::new();

        let ports: FactoryVecDeque<PortEntry> = FactoryVecDeque::builder()
            .launch_default()
            .forward(sender.input_sender(), |output| match output {
                PortEntryOutput::Remove(index) => BubbleSettingsMsg::RemovePort(index),
            });

        let ports_listbox_widget = ports.widget().clone();

        let model = BubbleSettingsDialog {
            vm_name: String::new(),
            cpu_row: cpu_row.clone(),
            ram_row: ram_row.clone(),
            loopback_row: loopback_row.clone(),
            ports,
        };

        let cpu_row = &cpu_row;
        let ram_row = &ram_row;
        let loopback_row = &loopback_row;
        let ports_listbox = &ports_listbox_widget;

        let widgets = view_output!();
        ComponentParts { model, widgets }
    }

    fn update(&mut self, msg: Self::Input, sender: ComponentSender<Self>) {
        match msg {
            BubbleSettingsMsg::Load(name) => {
                self.vm_name = name;
                let config = load_config(&self.vm_name);
                self.cpu_row.set_value(config.cpus as f64);
                self.ram_row.set_value(config.ram_mb as f64);
                self.loopback_row.set_active(config.map_host_loopback);

                let mut ports_guard = self.ports.guard();
                ports_guard.clear();
                for port in &config.tcp_ports {
                    ports_guard.push_back(port.clone());
                }
                drop(ports_guard);

            }
            BubbleSettingsMsg::Save => {
                if self.vm_name.is_empty() { return; }
                let tcp_ports: Vec<String> = self.ports.iter()
                    .map(|entry| entry.text.trim().to_string())
                    .filter(|s| !s.is_empty() && is_valid_port_entry(s))
                    .collect();
                let config = BubbleConfig {
                    cpus: self.cpu_row.value() as u32,
                    ram_mb: self.ram_row.value() as u32,
                    tcp_ports,
                    map_host_loopback: self.loopback_row.is_active(),
                };
                save_config(&self.vm_name, &config);
            }
            BubbleSettingsMsg::AddPort => {
                self.ports.guard().push_back(String::new());
            }
            BubbleSettingsMsg::RemovePort(index) => {
                self.ports.guard().remove(index.current_index());
            }
        }
    }
}
