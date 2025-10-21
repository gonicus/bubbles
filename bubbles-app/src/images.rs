use adw::prelude::ActionRowExt;
use gtk::{
    gio::{Subprocess, SubprocessFlags},
    glib::{self},
    prelude::*,
};
use std::{env, ffi::OsStr, fs, path::Path};

#[derive(PartialEq, Debug, Clone)]
pub enum ImageStatus {
    NotPresent,
    Downloading,
    Present,
}

#[derive(PartialEq, Debug, Clone)]
pub struct Image {
    name: String,
    status: ImageStatus,
}

pub fn set_download_status_on_button(
    button: &gtk::Button,
    label: &gtk::Label,
    image_status: ImageStatus,
) {
    if image_status == ImageStatus::Present {
        label.set_label("Ready");
        button.set_sensitive(true);
        button.set_icon_name("view-refresh-symbolic");
    } else if image_status == ImageStatus::Downloading {
        label.set_label("Loading...");
        button.set_sensitive(false);
        button.set_icon_name("image-loading-symbolic");
    } else if image_status == ImageStatus::NotPresent {
        label.set_label("Not downloaded");
        button.set_sensitive(true);
        button.set_icon_name("folder-download-symbolic");
    }
}

pub fn determine_download_status() -> ImageStatus {
    let images_dir = env::current_dir()
        .expect("cwd to be set")
        .join(Path::new(".bubbles/images"));
    fs::create_dir_all(&images_dir).expect("directory to exist or be created");

    let image_exists = images_dir.join(Path::new("debian-13")).exists();

    return match image_exists {
        true => ImageStatus::Present,
        false => ImageStatus::NotPresent,
    };
}

pub fn build_image_list(images: Vec<Image>) -> gtk::ListBox {
    let image_list = gtk::ListBox::builder().build();
    for image in images {
        let entry = adw::ActionRow::builder().title(image.name).build();
        entry.add_prefix(&gtk::Image::from_icon_name(
            "drive-harddisk-system-symbolic",
        ));
        let download_box = gtk::CenterBox::new();
        let download_button = &gtk::Button::builder().build();
        let download_label = &gtk::Label::new(Some(""));
        set_download_status_on_button(
            &download_button,
            &download_label,
            determine_download_status(),
        );
        download_box.set_center_widget(Some(download_label));
        download_box.set_end_widget(Some(download_button));
        entry.add_suffix(&download_box);
        let (sender, receiver) = async_channel::bounded(1);
        download_button.connect_clicked(move |_button| {
            glib::spawn_future_local(glib::clone!(
                #[strong]
                sender,
                async move {
                    sender.send(ImageStatus::Downloading).await.expect("channel to be open");
                    Subprocess::newv(
                        &[OsStr::new("scripts/download.bash")],
                        SubprocessFlags::empty()
                    ).expect("download").wait_future().await.expect("download to succeed");
                    sender.send(determine_download_status()).await.expect("channel to be open");
                }
            ));
        });
        glib::spawn_future_local(glib::clone!(
            #[weak]
            download_button,
            #[weak]
            download_label,
            async move {
                while let Ok(image_status) = receiver.recv().await {
                    set_download_status_on_button(&download_button, &download_label, image_status);
                }
            }
        ));
        image_list.append(&entry);
    }
    return image_list;
}

pub fn load_images() -> Vec<Image> {
    let images = vec![Image {
        name: "Debian 13 Bubble Distribution".to_string(),
        status: determine_download_status(),
    }];
    return images;
}
