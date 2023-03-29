use clap::Clap as _;
use rayon::prelude::*;
use std::io::Write as _;

#[derive(clap::Clap, Debug)]
struct Opts {
    /// additional C compiler arguments
    #[clap(
        long,
        allow_hyphen_values = true,
        multiple_occurrences = true,
        number_of_values = 1
    )]
    ccadd: Vec<String>,

    /// remove C compiler arguments
    #[clap(
        long,
        allow_hyphen_values = true,
        multiple_occurrences = true,
        number_of_values = 1
    )]
    ccdel: Vec<String>,

    /// override C compiler
    #[clap(long)]
    use_cc: Option<String>,

    /// override C++ compiler
    #[clap(long)]
    use_cxx: Option<String>,

    /// extracts the toolchain includes and adds them and `-nostdinc` to the
    /// command
    #[clap(long)]
    resolve_toolchain_includes: bool,

    /// Path to patched compilation database
    #[clap(long, short)]
    out: String,

    /// Path to compilation database (e.g. compile_commands.json)
    cdb: String,
}

#[derive(serde::Serialize, serde::Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CdbEntry {
    directory: String,
    file: String,
    command: String,
}

enum Language {
    C,
    Cxx,
}

fn file_to_language(file: &str) -> Option<Language> {
    let file = std::path::Path::new(file);
    match file.extension().map(|v| v.to_str().unwrap()) {
        Some("c") => Some(Language::C),
        Some("cpp") | Some("cc") => Some(Language::Cxx),
        _ => None,
    }
}

#[derive(Default)]
struct ToolchainInfoCache {
    hm: std::collections::HashMap<Vec<String>, Vec<String>>,
}

impl ToolchainInfoCache {
    fn add_toolchain_includes_(command: &mut Vec<String>, includes: &[String]) {
        for include in includes {
            command.insert(1, format!("-isystem{include}"));
        }
    }

    pub fn add_toolchain_includes(
        &mut self,
        file: &str,
        command: &mut Vec<String>,
    ) -> Result<(), anyhow::Error> {
        static DEL_ARGS_MAYBETWO: &[&str] = &[
            "-I",
            "-L",
            "-imacros",
            "-isystem",
            "-include",
            "-D",
            "-W",
            "-o",
            "-c",
            "-E",
            "-fmacro-prefix-map",
            "-O",
        ];

        // make new args, ignoring the ones which don't matter for the cpp
        let mut args = Vec::with_capacity(command.len());
        let mut nskip = 0;

        args.push(command[0].to_string());
        for arg in command[1..].iter() {
            if nskip > 0 {
                nskip -= 1;
                continue;
            }

            let mut found = false;
            for delarg in DEL_ARGS_MAYBETWO {
                if let Some(s) = arg.strip_prefix(delarg) {
                    if s.is_empty() {
                        nskip = 1;
                    }
                    found = true;
                    break;
                }
            }
            if found {
                continue;
            }

            // don't push positional args because they are sources
            if arg.starts_with('-') {
                args.push(arg.to_string());
            }
        }
        args[1..].sort();

        // since we're using /dev/null we have to set the language
        if !args.iter().any(|s| s.starts_with("-x")) {
            match file_to_language(file) {
                Some(Language::C) => args.push("-xc".to_string()),
                Some(Language::Cxx) => args.push("-xc++".to_string()),
                _ => return Ok(()),
            }
        }

        ["-P", "-E", "-Wp,-v", "/dev/null"]
            .iter()
            .for_each(|s| args.push(s.to_string()));

        // check cache
        if let Some(includes) = self.hm.get(&args) {
            Self::add_toolchain_includes_(command, includes);
            return Ok(());
        }

        let mut cmd = std::process::Command::new(&args[0]);
        cmd.args(&args[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped());
        let output = cmd.spawn()?.wait_with_output()?;
        if !output.status.success() {
            let stderr = std::str::from_utf8(&output.stderr);
            return match stderr {
                Ok(stderr) => Err(anyhow::anyhow!(
                    "compiler failed with {}. args: {:?}\nstderr:\n {}",
                    output.status.code().unwrap(),
                    args,
                    stderr
                )),
                Err(_) => Err(anyhow::anyhow!(
                    "compiler failed with {}. args: {:?}\nstderr:\n {:?}",
                    output.status.code().unwrap(),
                    args,
                    &output.stderr
                )),
            };
        }

        // the includes start with a space, extract them
        let output = std::str::from_utf8(&output.stderr)?.trim();
        let includes: Vec<_> = output
            .lines()
            .filter_map(|s| s.strip_prefix(' ').map(|s| s.to_string()))
            .collect();

        Self::add_toolchain_includes_(command, &includes);

        self.hm.insert(args, includes);
        Ok(())
    }
}

/// escape command for compilation database
/// - only " and \ are special
/// - add quotes if there are spaces or escape sequences
fn cdb_escape(input: &str) -> String {
    lazy_static::lazy_static! {
        static ref ESCAPE_PATTERN: regex::Regex = regex::Regex::new(r#"([\\"])"#).unwrap();
    }

    if input.is_empty() {
        return "\"\"".to_owned();
    }

    let output = ESCAPE_PATTERN.replace_all(input, "\\$1").to_string();
    if output != input || output.contains(' ') {
        return format!("\"{output}\"");
    }

    output
}

fn main() -> Result<(), anyhow::Error> {
    let opts: Opts = Opts::parse();

    let mut cdb: Vec<CdbEntry> = serde_json::from_str(&std::fs::read_to_string(&opts.cdb)?)?;
    thread_local!(static TIC: std::cell::RefCell<ToolchainInfoCache> =
        std::cell::RefCell::new(ToolchainInfoCache::default())
    );

    cdb.par_iter_mut()
        .map(|entry| {
            let mut command = shellwords::split(&entry.command).expect("can't split command");

            match file_to_language(&entry.file) {
                Some(Language::C) => {
                    if let Some(s) = &opts.use_cc {
                        command[0] = s.to_string();
                    }
                }
                Some(Language::Cxx) => {
                    if let Some(s) = &opts.use_cxx {
                        command[0] = s.to_string();
                    }
                }
                _ => (),
            }

            if opts.resolve_toolchain_includes {
                TIC.with(|tic| {
                    tic.borrow_mut()
                        .add_toolchain_includes(&entry.file, &mut command)
                        .expect("can't get toolchain includes")
                });

                if !command.iter().any(|s| s == "-nostdinc") {
                    command.push("-nostdinc".to_string());
                }
            }

            command.extend_from_slice(&opts.ccadd);

            entry.command = command
                .iter()
                .map(|arg| cdb_escape(arg))
                .filter(|arg| !opts.ccdel.contains(arg))
                .collect::<Vec<_>>()
                .join(" ");
        })
        .for_each(|_| {});

    let mut out = std::fs::File::create(&opts.out)?;
    out.write_all(serde_json::to_string(&cdb)?.as_bytes())?;

    Ok(())
}
