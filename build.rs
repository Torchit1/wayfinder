fn main() {
    // app + tray icon (assets/wayfinder.ico, built by assets/make_logo.py)
    embed_resource::compile("wayfinder.rc", embed_resource::NONE).manifest_optional().unwrap();
}
