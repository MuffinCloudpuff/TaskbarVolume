fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() == Some(std::ffi::OsStr::new("windows")) {
        let mut resources = winres::WindowsResource::new();
        resources.set_icon("assets/taskbar-volume.ico");
        resources.compile().expect("could not embed the application icon");
    }
}
