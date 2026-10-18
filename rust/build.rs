fn main() {
    // tauri_build embeds icons/icon.ico as the exe's icon resource but does not
    // tell cargo to watch it: without this a changed icon leaves the old one in
    // the exe (scripts/verify/exe-icon.py checks it after every GUI build).
    #[cfg(feature = "gui")]
    {
        println!("cargo:rerun-if-changed=icons");
        tauri_build::build();
    }
}
