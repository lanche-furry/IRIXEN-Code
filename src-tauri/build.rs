fn main() {
    // The Windows .exe icon and Tauri's compiled-in default window icon are
    // produced by this build script (winres + generate_context!). Cargo caches
    // build-script output, so changing src-tauri/icons/* on disk alone does NOT
    // refresh them: an incremental release build happily relinks the old .res
    // and the app keeps showing the previous icon forever.
    //
    // Declaring every icon we ship as a rerun trigger makes the resource
    // compiler re-run whenever `npm run icons` regenerates the set.
    for icon in [
        "icons/icon.ico",
        "icons/icon.icns",
        "icons/icon.png",
        "icons/32x32.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
    ] {
        println!("cargo:rerun-if-changed={icon}");
    }
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");

    tauri_build::build();
}
