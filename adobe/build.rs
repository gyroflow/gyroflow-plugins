
use pipl::*;

const PF_PLUG_IN_VERSION: u16 = 13;
const PF_PLUG_IN_SUBVERS: u16 = 28;

fn main() {
    #[cfg(target_os = "windows")] {
        let mut version = std::env::var("CARGO_PKG_VERSION").unwrap();

        if std::env::var("GITHUB_REF").map(|x| x.contains("tags")).unwrap_or_default() {
            version.push_str(".0");
        } else if let Ok(github_run_number) = std::env::var("GITHUB_RUN_NUMBER") {
            version.push_str(&format!(".{}", github_run_number));
        } else {
            version.push_str("-dev");
        }

        unsafe { std::env::set_var("CARGO_PKG_VERSION", version) };
    }

    pipl::plugin_build(vec![
        Property::Kind(PIPLType::AEEffect),
        Property::Name("Gyroflow"),
        Property::Category("Gyroflow"),

        #[cfg(target_os = "windows")]
        Property::CodeWin64X86("EffectMain"),
        #[cfg(target_os = "macos")]
        Property::CodeMacIntel64("EffectMain"),
        #[cfg(target_os = "macos")]
        Property::CodeMacARM64("EffectMain"),

        Property::AE_PiPL_Version { major: 2, minor: 0 },
        Property::AE_Effect_Spec_Version { major: PF_PLUG_IN_VERSION, minor: PF_PLUG_IN_SUBVERS },
        Property::AE_Effect_Version {
            version: 0,
            subversion: 1,
            bugversion: 0,
            stage: Stage::Develop,
            build: 0
        },
        Property::AE_Effect_Info_Flags(0),
        Property::AE_Effect_Global_OutFlags(
			OutFlags::CustomUI |
			OutFlags::IDoDialog |
			OutFlags::PixIndependent |
			OutFlags::DeepColorAware |
			//OutFlags::SendUpdateParamsUI |
            OutFlags::IExpandBuffer |
			OutFlags::NonParamVary
		),
        Property::AE_Effect_Global_OutFlags_2(
			OutFlags2::FloatColorAware |
			OutFlags2::SupportsSmartRender |
			OutFlags2::SupportsThreadedRendering |
		    OutFlags2::SupportsGetFlattenedSequenceData |
			OutFlags2::SupportsGpuRenderF32 |
            OutFlags2::ParamGroupStartCollapsedFlag
		),
        Property::AE_Effect_Match_Name("Gyroflow"),
        Property::AE_Reserved_Info(0),
        Property::AE_Effect_Support_URL("https://docs.gyroflow.xyz"),
    ])
}
