use std::sync::Arc;

use petramond::modding::client::files::folders::{install_chooser, FolderRequest};

pub fn install() {
    install_chooser(Some(Arc::new(pick)));
}

fn pick(request: FolderRequest) {
    let _ = std::thread::Builder::new()
        .name("folder-picker".into())
        .spawn(move || {
            let mut dialog = rfd::AsyncFileDialog::new().set_title(&request.title);
            if let Some(start) = &request.start {
                dialog = dialog.set_directory(start);
            }
            let picked = pollster::block_on(dialog.pick_folder());
            request
                .done
                .answer(picked.map(|folder| folder.path().to_path_buf()));
        });
}
