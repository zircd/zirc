//! Signs a release's checksums with zirc's release key, as the updater checks them: see
//! [`zirc_core::update`].
//!
//! ```text
//! release_sign sign <file>...   reads the private key (PKCS#8, in hex) on stdin and writes
//!                               each file's signature, in hex, to <file>.sig
//! release_sign generate <public-key-file>
//!                               writes a new private key (PKCS#8, in hex) to stdout and its
//!                               public key, in hex, to <public-key-file>
//! ```
//!
//! `sign` refuses a key other than the one the updater trusts ([`update::RELEASE_KEY`], or
//! [`update::TEST_KEY`]'s in tests), so a release is never signed with a key no updater takes.
//! Neither prints a private key anywhere but `generate`'s stdout.

use std::io::Read as _;
use std::process::ExitCode;

use zirc_core::update::{self, Trust};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((command, files)) if command == "sign" && !files.is_empty() => sign(files),
        Some((command, [public])) if command == "generate" => generate(public),
        _ => Err(
            "usage: release_sign sign <file>... | release_sign generate <public-key-file>"
                .to_owned(),
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(why) => {
            eprintln!("release_sign: {why}");
            ExitCode::FAILURE
        }
    }
}

fn sign(files: &[String]) -> Result<(), String> {
    let mut private = String::new();
    std::io::stdin()
        .read_to_string(&mut private)
        .map_err(|err| format!("reading the key on stdin: {err}"))?;
    let trusted = Trust::from_env().map_err(|err| err.to_string())?.key();
    for file in files {
        let bytes = std::fs::read(file).map_err(|err| format!("{file}: {err}"))?;
        let (signature, public) =
            update::sign(&private, &bytes).map_err(|err| format!("the key on stdin: {err}"))?;
        if public != trusted {
            return Err(format!(
                "the key on stdin isn't the one the updater trusts ({trusted}), but {public}'s"
            ));
        }
        let sig = format!("{file}.sig");
        std::fs::write(&sig, format!("{signature}\n")).map_err(|err| format!("{sig}: {err}"))?;
        println!("signed {file}");
    }
    Ok(())
}

fn generate(public: &str) -> Result<(), String> {
    let (private, key) = update::generate().map_err(|err| err.to_string())?;
    std::fs::write(public, format!("{key}\n")).map_err(|err| format!("{public}: {err}"))?;
    println!("{private}");
    Ok(())
}
