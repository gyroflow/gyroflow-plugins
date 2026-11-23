use std::path::PathBuf;
use std::sync::{OnceLock, atomic::{AtomicU8, Ordering}};

pub const PRESET_KEY: &str = "openfx_inspector_language";
pub const CHOICES: [&str; 3] = ["Use default", "English", "简体中文"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InspectorLanguage {
    #[default]
    English,
    Chinese,
}

pub fn default_preset_path() -> PathBuf {
    gyroflow_core::settings::data_dir().join("lens_profiles").join("default.gyroflow")
}

impl InspectorLanguage {
    /// The language saved with "Save as default", `None` when it follows the host's
    pub fn from_preset(preset: &serde_json::Value) -> Option<Self> {
        match preset.get(PRESET_KEY).and_then(|value| value.as_str()) {
            Some("zh_CN") => Some(Self::Chinese),
            Some("en") => Some(Self::English),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self { Self::English => "en", Self::Chinese => "zh_CN" }
    }

    /// The language a Language selection shows: "Use default" (0) is the saved default, else the language of DaVinci Resolve
    pub fn resolve(selection: i32) -> Self {
        match selection {
            1 => Self::English,
            2 => Self::Chinese,
            _ => match saved_default().load(Ordering::Relaxed) {
                SAVED_ENGLISH => Self::English,
                SAVED_CHINESE => Self::Chinese,
                _ => *host_language(),
            },
        }
    }

    /// The language a selection saves as the default: `None` for "Use default", which keeps following the host's
    pub fn saved_for(selection: i32) -> Option<Self> {
        match selection { 1 => Some(Self::English), 2 => Some(Self::Chinese), _ => None }
    }

    pub fn remember_default(saved: Option<Self>) {
        let value = match saved { Some(Self::English) => SAVED_ENGLISH, Some(Self::Chinese) => SAVED_CHINESE, None => SAVED_NONE };
        saved_default().store(value, Ordering::Relaxed);
    }

    /// Accept either language so existing Inspector values can also be translated on a switch.
    pub fn text(self, text: &str) -> &str {
        for &(english, chinese) in TRANSLATIONS {
            if text == english || text == chinese {
                return if self == Self::Chinese { chinese } else { english };
            }
        }
        text
    }
}

const SAVED_NONE: u8 = 0;
const SAVED_ENGLISH: u8 = 1;
const SAVED_CHINESE: u8 = 2;

fn saved_default() -> &'static AtomicU8 {
    static DEFAULT: OnceLock<AtomicU8> = OnceLock::new();
    DEFAULT.get_or_init(|| {
        let saved = std::fs::read(default_preset_path()).ok()
            .and_then(|data| serde_json::from_slice::<serde_json::Value>(&data).ok())
            .and_then(|preset| InspectorLanguage::from_preset(&preset));
        AtomicU8::new(match saved { Some(InspectorLanguage::English) => SAVED_ENGLISH, Some(InspectorLanguage::Chinese) => SAVED_CHINESE, None => SAVED_NONE })
    })
}

/// DaVinci Resolve doesn't tell plugins its UI language, nor keep it in a settings file it writes as text; its debug
/// log of the running session says it at startup ("UI language is set to "zh_CN"."). English when it can't be read
fn host_language() -> &'static InspectorLanguage {
    static LANGUAGE: OnceLock<InspectorLanguage> = OnceLock::new();
    LANGUAGE.get_or_init(|| {
        let language = resolve_log_path()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|log| language_from_resolve_log(&String::from_utf8_lossy(&log)));
        log::info!("DaVinci Resolve UI language: {language:?}");
        language.unwrap_or_default()
    })
}

fn resolve_log_path() -> Option<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Blackmagic Design").join("DaVinci Resolve").join("Support")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Blackmagic Design/DaVinci Resolve")
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".local/share/DaVinciResolve")
    };
    Some(base.join("logs").join("ResolveDebug.txt"))
}

/// The last UI language a Resolve log reports, any Chinese one shown in Simplified Chinese
fn language_from_resolve_log(log: &str) -> Option<InspectorLanguage> {
    const MARKER: &str = "UI language is set to \"";
    let start = log.rfind(MARKER)? + MARKER.len();
    let code = log[start..].split('"').next()?;
    Some(if code.to_ascii_lowercase().starts_with("zh") { InspectorLanguage::Chinese } else { InspectorLanguage::English })
}

const TRANSLATIONS: &[(&str, &str)] = &[
    ("Language", "界面语言"),
    ("Choose the Inspector language. Use default follows DaVinci Resolve's language; Save as default keeps the chosen language for new instances.", "选择检查器的界面语言。“使用默认”跟随 DaVinci Resolve 的界面语言；点击“保存为默认”可让新实例使用所选语言。"),
    ("Use default", "使用默认"),
    ("Gyroflow project", "Gyroflow 项目"),
    ("Status", "状态"),
    ("Load for current file", "为当前素材加载"),
    ("Try to load project file for current video file, or try to stabilize that video file directly", "尝试加载当前视频对应的项目，也可以直接稳定该视频"),
    ("Browse", "浏览"),
    ("Browse for the Gyroflow project file", "浏览并选择 Gyroflow 项目文件"),
    ("Load preset/lens profile", "加载预设或镜头配置"),
    ("Open a lens profile, including a zoom profile, or a preset", "打开镜头配置（含变焦配置）或预设"),
    ("Lens profile library", "镜头配置库"),
    ("Select a lens profile…", "选择镜头配置…"),
    ("Load a lens profile or preset from the Gyroflow lens_profiles folder. The list is read when DaVinci Resolve starts.", "从 Gyroflow 的 lens_profiles 文件夹加载镜头配置或预设。列表在 DaVinci Resolve 启动时读取。"),
    ("Open Gyroflow", "打开 Gyroflow"),
    ("Open in Gyroflow", "在 Gyroflow 中打开"),
    ("Open project in Gyroflow", "在 Gyroflow 中打开项目"),
    ("Reload project", "重新加载项目"),
    ("Reload currently loaded project", "重新加载当前项目"),
    ("Last saved project", "最近保存的项目"),
    ("Load most recently saved project in the Gyroflow app", "加载 Gyroflow 中最近保存的项目"),
    ("Adjust parameters", "参数调整"),
    ("Smoothness", "平滑度"),
    ("Zoom limit", "缩放上限"),
    ("Lens correction", "镜头校正"),
    ("Horizon lock", "地平线锁定"),
    ("Horizon lock amount", "地平线锁定强度"),
    ("Horizon roll", "地平线旋转"),
    ("Horizon lock roll adjustment", "调整地平线锁定的旋转角度"),
    ("Additional pitch", "额外俯仰"),
    ("Additional pitch rotation", "额外的俯仰旋转角度"),
    ("Additional yaw", "额外偏航"),
    ("Additional yaw rotation", "额外的偏航旋转角度"),
    ("Video rotation", "视频旋转"),
    ("Input rotation", "输入旋转"),
    ("FOV", "视野（FOV）"),
    ("Video speed", "视频速度"),
    ("Use this slider to change video speed or keyframe it, instead of built-in speed changes in the editor", "用此滑块调整视频速度或为速度设置关键帧"),
    ("Zooming", "缩放模式"),
    ("Static zooming uses one crop for the clip. Dynamic zooming adapts the crop with a 4-second smoothing window.", "静态缩放对整段素材使用同一裁切。动态缩放以 4 秒的平滑窗口调整裁切。"),
    ("Static zooming", "静态缩放"),
    ("Dynamic zooming", "动态缩放"),
    ("Integration method", "积分方式"),
    ("IMU integration method", "惯性测量数据的积分方式"),
    ("None", "无"),
    ("Complementary", "互补滤波"),
    ("Simple gyro", "仅陀螺仪"),
    ("Simple gyro + accel", "陀螺仪与加速度计"),
    ("1.0x (Default)", "1.0x（默认）"),
    ("Custom", "自定义"),
    ("Input horizontal stretch", "输入水平拉伸"),
    ("Input vertical stretch", "输入垂直拉伸"),
    ("Initialized from input_horizontal_stretch in the lens profile. Describes the recorded sensor's horizontal axis before video rotation. Used with vertical stretch for anamorphic framing and the crop guide; does not de-squeeze the image.", "初始值来自镜头配置，表示视频旋转前传感器的水平轴拉伸。与垂直拉伸一起用于变形镜头构图和裁切参考线。"),
    ("Initialized from input_vertical_stretch in the lens profile. Describes the recorded sensor's vertical axis before video rotation. Used for anamorphic framing and the crop guide; does not de-squeeze the image.", "初始值来自镜头配置，表示视频旋转前传感器的垂直轴拉伸，用于变形镜头构图和裁切参考线。"),
    ("Custom horizontal stretch", "自定义水平拉伸"),
    ("Custom vertical stretch", "自定义垂直拉伸"),
    ("Custom horizontal stretch (1.0x - 2.0x), used when Input horizontal stretch is set to Custom", "输入水平拉伸设为自定义时使用此值（1.0x–2.0x）"),
    ("Custom vertical stretch (1.0x - 2.0x), used when Input vertical stretch is set to Custom", "输入垂直拉伸设为自定义时使用此值（1.0x–2.0x）"),
    ("Save as default", "保存为默认"),
    ("Save the current smoothness, zooming, zoom limit, lens correction, horizon lock, FOV and Inspector language as the defaults for every newly loaded clip (lens_profiles/default.gyroflow)", "将当前平滑度、缩放、缩放上限、镜头校正、地平线锁定、视野和界面语言保存为默认设置。"),
    ("Keyframes", "关键帧"),
    ("Use Gyroflow's keyframes", "使用 Gyroflow 关键帧"),
    ("Use internal Gyroflow's keyframes, instead of the editor ones.", "使用 Gyroflow 项目中的关键帧。"),
    ("Adjust stabilization to speed", "根据速度调整防抖"),
    ("When you speed ramp the clip, let Gyroflow adjust the stabilization amount to the video speed.", "变速时根据视频速度调整防抖强度。"),
    ("Recalculate keyframes", "重新计算关键帧"),
    ("Recalculate keyframes after adjusting the splines (in Fusion mode)", "在 Fusion 中调整曲线后重新计算关键帧"),
    ("Output size", "输出尺寸"),
    ("Width", "宽度"),
    ("Height", "高度"),
    ("Fit to timeline", "适配时间线"),
    ("Set the output size to the timeline dimensions", "将输出尺寸设为时间线分辨率"),
    ("Swap", "交换"),
    ("Swap width and height", "交换宽度和高度"),
    ("Interpolation", "插值"),
    ("Scaling interpolation method", "图像缩放的插值算法"),
    ("Bilinear", "双线性"),
    ("Bicubic", "双三次"),
    ("Stabilization overview", "防抖预览"),
    ("Zooms out the view to see the stabilization results. Disable this before rendering.", "缩小画面以预览防抖效果。渲染前请关闭。"),
    ("Don't draw outside source clip", "仅绘制源素材范围"),
    ("When clip and timeline aspect ratio don't match, draw the final image inside the source clip, instead of drawing outside it.", "素材和时间线的宽高比不同时，将画面限制在源素材范围内。"),
    ("Embed .gyroflow data in plugin", "在插件中嵌入 .gyroflow 数据"),
    ("If you intend to share the project to someone else, the plugin can embed the Gyroflow project data including gyro data inside the video editor project. This way you don't have to share .gyroflow project files. Enabling this option will make the project bigger.", "将 Gyroflow 项目和陀螺仪数据嵌入剪辑项目，方便分享。启用后项目体积会增大。"),
    ("Info", "信息"),
    ("Loaded project", "已加载的项目"),
    ("Loaded project or video file", "已加载的项目或视频"),
    ("Loaded preset", "已加载的预设"),
    ("Loaded lens profile", "已加载的镜头配置"),
    ("Camera", "相机"),
    ("Camera setting", "相机设置"),
    ("Focal length", "焦距"),
    ("Horizontal squeeze", "水平变形倍率"),
    ("Vertical squeeze", "垂直变形倍率"),
    ("Distortion-Aware Border", "按镜头畸变计算边界"),
    ("Use the lens distortion model for the stabilization border. RS Compen. Only always uses rectangular geometry.", "按照镜头畸变模型计算防抖边界。仅补偿滚动快门时使用矩形边界。"),
    ("Generic profile", "通用镜头配置"),
    ("Use generic profile", "使用通用镜头配置"),
    ("Stabilize with an uncalibrated lens of the focal length below, in place of the clip's lens profile and any profile loaded with Load preset/lens profile. For a lens without a profile, such as one without electronic contacts.", "按下方焦距使用未校准的镜头进行防抖，替代素材的镜头配置以及通过“加载预设/镜头配置”加载的配置。适用于没有镜头配置的镜头，例如无电子触点镜头。"),
    ("Lens focal (mm)", "镜头焦距（毫米）"),
    ("The lens's focal length in mm. Set from the clip's metadata when it has one, while Use generic profile is off.", "镜头焦距（毫米）。未启用通用镜头配置时，若素材元数据包含焦距则自动填入。"),
    ("Sensor width (mm)", "传感器宽度（毫米）"),
    ("The width of the sensor area the clip records, in mm (36 = full frame, about 23.5 = APS-C / Super 35). The clip's own value is used when it reports one (Sony, SIGMA fp CinemaDNG), and set here while Use generic profile is off.", "素材记录的传感器区域宽度（毫米，36 = 全画幅，约 23.5 = APS-C / Super 35）。素材提供该值时（索尼、SIGMA fp CinemaDNG）使用素材的值，并在未启用通用镜头配置时自动填入。"),
    ("Typical distortion for focal", "按焦距使用典型畸变"),
    ("The barrel distortion typical of the focal length (from Gyroflow's lens database), for RAW. Off: use Corner distortion, 0 for video the camera already corrected.", "使用该焦距的典型桶形畸变（来自 Gyroflow 镜头数据库），适用于 RAW。关闭时使用边角畸变；对于机内已校正畸变的视频，设为 0。"),
    ("Corner distortion (%)", "边角畸变（%）"),
    ("With Typical distortion off: the frame corner against an ideal lens. Negative = barrel, positive = pincushion, 0 = none.", "关闭典型畸变时：画面边角相对理想镜头的畸变。负值为桶形，正值为枕形，0 为无畸变。"),
    ("Rolling Shutter", "滚动快门"),
    ("RS Compen. Only", "仅补偿滚动快门"),
    ("RS Compen. On", "启用滚动快门补偿"),
    ("Apply only rolling shutter compensation with rectangular border geometry. Requires RS Compen. On.", "仅进行滚动快门补偿，并使用矩形边界。需要先启用滚动快门补偿。"),
    ("Correct rolling shutter using the sensor readout time.", "使用传感器读出时间校正滚动快门。"),
    ("Rolling shutter (ms)", "读出时间（毫秒）"),
    ("Sensor readout time in milliseconds, initialized from the loaded profile or clip.", "传感器读出时间（毫秒），初始值来自已加载的镜头配置或素材。"),
    ("Project not loaded", "尚未加载项目"),
    ("Failed to load file info!", "无法读取文件信息"),
    ("Calculating...", "正在计算…"),
    ("OK", "正常"),
    ("No gyro data in this file", "此文件没有陀螺仪数据"),
    ("This file has no motion data (a transcode or an export of the original, or a camera without gyro). Use the original camera file, or open it in Gyroflow to load the motion data from another file.", "此文件没有运动数据。请使用相机原始素材，或在 Gyroflow 中从其他文件加载运动数据。"),
    ("Not synced. Open in Gyroflow", "尚未同步，请在 Gyroflow 中打开"),
    ("Gyro data is not synced with the video, open the video in Gyroflow and add sync points (eg. by doing autosync)", "陀螺仪数据与视频尚未同步。请在 Gyroflow 中打开视频并添加同步点。"),
    ("Timeline frame rate doesn't match the clip frame rate! Use the plugin in Fusion instead", "时间线帧率与素材帧率不一致，请在 Fusion 中使用插件。"),
    ("Default settings saved.\nThey will be applied to the next clips loaded.", "默认设置已保存，将应用于之后加载的素材。"),
    ("Failed to save default settings", "无法保存默认设置"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inspector_language_loads_saved_code_and_translates_both_ways() {
        assert_eq!(InspectorLanguage::from_preset(&serde_json::json!({})), None);
        let saved = serde_json::json!({ PRESET_KEY: InspectorLanguage::Chinese.code() });
        assert_eq!(InspectorLanguage::from_preset(&saved), Some(InspectorLanguage::Chinese));
        assert_eq!(InspectorLanguage::saved_for(0), None);
        assert_eq!(InspectorLanguage::saved_for(1), Some(InspectorLanguage::English));
        assert_eq!(InspectorLanguage::resolve(2).text("Save as default"), "保存为默认");
        assert_eq!(InspectorLanguage::resolve(1).text("保存为默认"), "Save as default");
        assert_eq!(InspectorLanguage::Chinese.text("VQF"), "VQF");
    }

    #[test]
    fn resolve_log_gives_the_running_ui_language() {
        let log = "[0x1] | UI | INFO | UI language is set to \"en\".\n[0x2] | UI | INFO | UI language is set to \"zh_CN\".\n";
        assert_eq!(language_from_resolve_log(log), Some(InspectorLanguage::Chinese));
        assert_eq!(language_from_resolve_log("UI language is set to \"ja\"."), Some(InspectorLanguage::English));
        assert_eq!(language_from_resolve_log("nothing here"), None);
    }

    #[test]
    fn every_inspector_text_has_a_chinese_translation() {
        use crate::{ GyroflowPluginBase, ParameterType };
        // Names and ratios read the same in both languages
        let same = |x: &str| x.ends_with('x') || ["VQF", "Mahony", "Madgwick", "Lanczos4", "RobidouxSharp", "Robidoux", "Mitchell", "CatmullRom", "Create camera", "Create camera layer"].contains(&x);
        let mut missing = Vec::new();
        let mut check = |x: &'static str| if !same(x) && InspectorLanguage::Chinese.text(x) == x { missing.push(x); };
        fn walk(x: &ParameterType, check: &mut dyn FnMut(&'static str)) {
            match x {
                ParameterType::HiddenString { .. } => {}
                ParameterType::TextBox { label, hint, .. } | ParameterType::Text { label, hint, .. } |
                ParameterType::Slider { label, hint, .. } | ParameterType::Checkbox { label, hint, .. } |
                ParameterType::Button { label, hint, .. } => { check(label); check(hint); }
                ParameterType::Select { label, hint, options, .. } => { check(label); check(hint); options.iter().for_each(|x| check(x)); }
                ParameterType::Group { label, parameters, .. } => { check(label); parameters.iter().for_each(|x| walk(x, check)); }
            }
        }
        for x in GyroflowPluginBase::get_param_definitions() { walk(&x, &mut check); }
        assert!(missing.is_empty(), "untranslated: {missing:?}");
    }
}
