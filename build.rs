use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=MIHOMO_EMBED_PATH");

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    let generated = out_dir.join("embedded_mihomo.rs");

    let source = env::var("MIHOMO_EMBED_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let content = if let Some(source) = source {
        let source_path = PathBuf::from(source);
        if !source_path.exists() {
            panic!(
                "MIHOMO_EMBED_PATH points to a missing file: {}",
                source_path.display()
            );
        }

        println!("cargo:rerun-if-changed={}", source_path.display());
        let embedded_path = out_dir.join("mihomo-embedded");
        fs::copy(&source_path, &embedded_path).expect("failed to copy embedded mihomo binary");

        format!(
            "pub const EMBEDDED_MIHOMO: Option<&'static [u8]> = Some(include_bytes!(r#\"{}\"#));\n",
            embedded_path.display()
        )
    } else {
        "pub const EMBEDDED_MIHOMO: Option<&'static [u8]> = None;\n".to_string()
    };

    fs::write(generated, content).expect("failed to write embedded_mihomo.rs");
}
