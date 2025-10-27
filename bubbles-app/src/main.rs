mod images;
mod vms;

use adw::prelude::AdwWindowExt;
use gtk::{
    glib::{self},
    prelude::*,
};

#[derive(PartialEq, Debug)]
struct State {
    images: Vec<images::Image>,
    vms: Vec<vms::VM>,
}

fn load_state() -> State {
    return State {
        images: images::load_images(),
        vms: vms::load_vms(),
    };
}
fn build_window_content() -> gtk::Widget {
    let state = load_state();
    let toolbar_view = adw::ToolbarView::builder().build();
    let view_stack = adw::ViewStack::builder().name("view").build();
    view_stack.add_titled_with_icon(
        &images::build_image_list(state.images),
        Some("images"),
        "Images",
        "drive-harddisk-system-symbolic",
    );
    view_stack.add_titled_with_icon(
        &vms::build_vms_list(state.vms),
        Some("vms"),
        "Bubbles",
        "computer-symbolic",
    );
    let view_switcher = adw::ViewSwitcher::builder()
        .stack(&view_stack)
        .policy(adw::ViewSwitcherPolicy::Wide)
        .build();
    let top_bar = adw::HeaderBar::builder()
        .title_widget(&view_switcher)
        .build();
    toolbar_view.add_top_bar(&top_bar);
    toolbar_view.set_content(Some(&view_stack));
    return toolbar_view.into();
}

fn build_ui(app: &adw::Application) {
    let window = adw::Window::builder()
        .default_width(600)
        .default_height(600)
        .application(app)
        .title("Bubbles")
        .build();

    window.set_content(Some(&build_window_content()));
    window.present();
}

fn main() -> glib::ExitCode {
    let application = adw::Application::builder()
        .application_id("de.gonicus.Bubbles")
        .build();

    application.connect_activate(build_ui);
    application.run()
}
