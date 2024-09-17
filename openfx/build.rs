
fn main() {
    #[cfg(target_os = "windows")] {
        let mut version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let mut version_short = version.clone().split('-').next().unwrap().to_string();

        if std::env::var("GITHUB_REF").map(|x| x.contains("tags")).unwrap_or_default() {
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
        res.set("FileVersion", &version_short);
        res.set("ProductVersion", &version);
        res.compile().unwrap();
    }
}
