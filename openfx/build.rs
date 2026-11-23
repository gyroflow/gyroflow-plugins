
fn main() {
    for var in ["OMNI_RELEASE", "GITHUB_REF", "GITHUB_RUN_NUMBER"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    #[cfg(target_os = "windows")] {
        let mut version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let mut version_short = version.clone().split('-').next().unwrap().to_string();

        // OMNI_RELEASE=1 marks a local release build (Omni has no CI), the same as a tagged CI build.
        let is_release = std::env::var("GITHUB_REF").map(|x| x.contains("tags")).unwrap_or_default()
            || std::env::var("OMNI_RELEASE").map(|x| x == "1").unwrap_or_default();
        if is_release {
            version.push_str(".0");
            version_short.push_str(".0");
        } else if let Ok(github_run_number) = std::env::var("GITHUB_RUN_NUMBER") {
            version.push_str(&format!(".{}", github_run_number));
            version_short.push_str(&format!(".{}", github_run_number));
        } else {
            version.push_str("-dev");
            version_short.push_str(".1");
        }

        let mut res = winres::WindowsResource::new();
        res.set("ProductName", "Gyroflow (Omni)");
        res.set("FileDescription", "Gyroflow (Omni) OpenFX plugin");
        res.set("FileVersion", &version_short);
        res.set("ProductVersion", &version);
        res.compile().unwrap();
    }
}
