//! Writes the generated schema and types under `generated/` (`generated/` in zirc's own
//! repository): see [`zirc_core::generated`]. Under `bazel run`, into the source tree;
//! otherwise under the current directory.

use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    let workspace = std::env::var_os("BUILD_WORKSPACE_DIRECTORY")
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    // Under oss/ in zirc's own repository; at the root of the public one, which has no oss/.
    let oss = workspace.join("oss");
    let root = if oss.is_dir() { oss } else { workspace }.join("generated");
    for (path, contents) in zirc_core::generated::files() {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().expect("under generated/"))?;
        std::fs::write(&path, contents)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
