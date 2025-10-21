use adw::prelude::ActionRowExt;
use gtk::prelude::*;

#[derive(PartialEq, Debug)]
pub struct NixStore {
    path: String,
    name: String,
}

pub fn load_nixstores() -> Vec<NixStore> {
    return vec![];
}

pub fn build_nixstore_list(nixstores: Vec<NixStore>) -> gtk::Widget {
    if nixstores.len() == 0 {
        let statuspage = adw::StatusPage::builder()
            .title("No Nix stores here, yet")
            .icon_name("package-x-generic-symbolic")
            .build();
        let create_button = &gtk::Button::builder()
            .css_classes(["pill", "suggested-action"])
            .label("Create Nix Store")
            .build();
        create_button.connect_clicked(|button| button.set_label("not implemented yet"));
        statuspage.set_child(Some(create_button));
        return statuspage.into();
    }
    let nixstore_list = gtk::ListBox::builder().build();
    for nixstore in nixstores {
        let entry = adw::ActionRow::builder()
            .title(nixstore.name)
            .subtitle(nixstore.path)
            .build();
        entry.add_prefix(&gtk::Image::from_icon_name(
            "drive-harddisk-system-symbolic",
        ));
        nixstore_list.append(&entry);
    }
    return nixstore_list.into();
}
