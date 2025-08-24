use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use gyroflow_plugin_base::parking_lot::Mutex;
use gyroflow_plugin_base::rfd;

const FAILED_MSG: &str = "This feature relies on external scripting and is only available in paid Resolve Studio. You have to allow executing scripts:\n
Set \"Preferences -> General -> External scripting using\" to \"Local\".\n\n
It must be the currently displayed video on the timeline.\n
It is also impossible to query file path on a compound clip.\n\nIn any case, you can just select the video or project file using the \"Browse\" button.";

fn replace_frame_count(input: &str) -> String {
    use regex::Regex;
    let re = Regex::new(r"\[(\d+)-(\d+)\]").unwrap();

    re.replace_all(input, |caps: &regex::Captures| {
        format!("{}", &caps[1])
    }).to_string()
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct CurrentFileInfo {
    pub file_path: String,
    pub project_path: Option<String>,
    pub fps: f64,
    pub duration_s: f64,
    pub frame_count: usize,
    pub width: usize,
    pub height: usize,
    pub pixel_aspect_ratio: String
}
impl CurrentFileInfo {
    pub fn get_fuscript() -> Option<std::path::PathBuf> {
        if cfg!(target_os = "windows") {
            Some(std::path::Path::new("fuscript.exe").to_path_buf())
        } else if cfg!(target_os = "macos") {
            Some(std::path::Path::new("../Libraries/Fusion/fuscript").to_path_buf())
        } else if cfg!(target_os = "linux") {
            let p1 = std::path::Path::new("../libs/Fusion/fuscript");
            let p2 = std::path::Path::new("./libs/Fusion/fuscript");
            if p1.exists() { return Some(p1.to_path_buf()); }
            if p2.exists() { return Some(p2.to_path_buf()); }
            None
        } else {
            None
        }
    }
    pub fn is_available() -> bool {
        Self::get_fuscript().map(|x| x.exists()).unwrap_or_default()
    }
    pub fn query(current_file_info: Arc<Mutex<Option<Self>>>, current_file_info_pending: Arc<AtomicBool>) {
        std::thread::spawn(move || {
            let mut cmd = std::process::Command::new(Self::get_fuscript().unwrap());
            #[cfg(target_os = "windows")]
            { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); } // CREATE_NO_WINDOW

            let script = "p = Resolve():GetProjectManager():GetCurrentProject():GetCurrentTimeline():GetCurrentVideoItem():GetMediaPoolItem():GetClipProperty();
                              print(p['FPS']);print(p['Frames']);print(p['Duration']);print(p['PAR']);print(p['Resolution']);print(p['File Path']);";
            if let Ok(out) = cmd.args(["-q", "-l", "lua", "-x", &script]).output() {
                let stdout = String::from_utf8(out.stdout).unwrap_or_default();
                let stderr = String::from_utf8(out.stderr).unwrap_or_default();
                // There is a weird bug in DaVinci Resolve fuscript that it complains about
                // missing python2 even regardless of explicitly specified `-l lua` argument.
                // The error message itself is a subject to localization, so it can't be hardcoded in whole.
                // See https://github.com/gyroflow/gyroflow-plugins/issues/24
                fn is_missing_python2(line: &str) -> bool {
                    line.starts_with("sh:") && line.contains("python2:")
                }
                let errors = stderr.trim().lines()
                        .filter(|line| !is_missing_python2(line))
                        .collect::<Vec<_>>();
                let lines = stdout.trim().lines().collect::<Vec<_>>();
                if errors.is_empty() && lines.len() == 6 {
                    let fps = lines[0].parse::<f64>().unwrap_or_default();
                    let frame_count = lines[1].parse::<usize>().unwrap_or_default();
                    let duration_s = Self::parse_duration(lines[2], fps);
                    let par = lines[3];
                    let resolution = lines[4].split("x").filter_map(|x| x.parse::<usize>().ok()).collect::<Vec<_>>();
                    let file_path = replace_frame_count(lines[5]);
                    if fps > 0.0 && frame_count > 0 && duration_s > 0.0 && !file_path.is_empty() {
                        let info = Self {
                            file_path: file_path.to_string(),
                            fps,
                            duration_s,
                            frame_count,
                            width: *resolution.get(0).unwrap_or(&0),
                            height: *resolution.get(1).unwrap_or(&0),
                            pixel_aspect_ratio: par.to_string(),
                            project_path: gyroflow_plugin_base::GyroflowPluginBase::get_project_path(&file_path)
                        };
                        log::debug!("{info:#?}");
                        *current_file_info.lock() = Some(info);
                        current_file_info_pending.store(true, SeqCst);

                        // Trigger render
                        let script = "c = Resolve():GetProjectManager():GetCurrentProject():GetCurrentTimeline():GetCurrentVideoItem();
                                          c:SetProperty('FlipX', c:GetProperty('FlipX'))";
                        let _ = cmd.args(["-x", &script]).spawn();
                    }
                } else {
                    log::debug!("fuscript stdout: {stdout}");
                    log::debug!("fuscript stderr: {stderr}");
                    rfd::MessageDialog::new()
                        .set_title("Failed to query current video file path.")
                        .set_description(FAILED_MSG)
                        .set_level(rfd::MessageLevel::Warning)
                        .show();
                }
            }
        });
    }

    fn parse_duration(v: &str, fps: f64) -> f64 {
        let parts = v.replace(";", ":").split(':').filter_map(|x| x.parse::<f64>().ok()).collect::<Vec<_>>();
        if parts.len() == 4 {
            parts[0] * 60.0 * 60.0 + // h
            parts[1] * 60.0 + // m
            parts[2] + // s
            parts[3] / fps.max(1.0)
        } else {
            0.0
        }
    }
}
