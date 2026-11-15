fn main() {
    // Embeds the icon, application manifest and version info.
    embed_resource::compile("assets/winproc-gui.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
    println!("cargo:rerun-if-changed=assets/winproc-gui.rc");
    println!("cargo:rerun-if-changed=assets/winproc-gui.ico");
    println!("cargo:rerun-if-changed=assets/winproc-gui.exe.manifest");
}
