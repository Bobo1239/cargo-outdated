/// Displays information about project dependency versions
///
/// USAGE:
///     cargo outdated [FLAGS] [OPTIONS]
///
/// FLAGS:
///         --all-features           Check outdated packages with all features enabled
///         --frozen                 Require Cargo.lock and cache are up to date
///     -h, --help                   Prints help information
///         --locked                 Require Cargo.lock is up to date
///         --no-default-features    Do not include the `default` feature
///     -q, --quiet                  Coloring: auto, always, never
///     -R, --root-deps-only         Only check root dependencies (Equivalent to --depth=1)
///     -V, --version                Prints version information
///     -v, --verbose                Use verbose output
///
/// OPTIONS:
///         --color <color>           Coloring: auto, always, never [default: auto]
///                                   [values: auto, always, never]
///     -d, --depth <NUM>             How deep in the dependency chain to search
///                                   (Defaults to all dependencies when omitted)
///         --exit-code <NUM>         The exit code to return on new versions found [default: 0]
///         --features <FEATURE>      Space-separated list of features
///     -m, --manifest-path <PATH>    An absolute path to the Cargo.toml file to use
///                                   (Defaults to Cargo.toml in project root)
///     -p, --packages <PKG>...       Package to inspect for updates
///     -r, --root <ROOT>             Package to treat as the root package
#[macro_use]
extern crate clap;
extern crate toml;
extern crate tempdir;
extern crate tabwriter;
extern crate serde;
#[macro_use]
extern crate serde_derive;
extern crate cargo;
extern crate env_logger;
extern crate semver;
extern crate term;

#[macro_use]
mod macros;
mod cargo_ops;
use cargo_ops::{ElaborateWorkspace, TempProject};

use std::path::Path;

use cargo::core::Workspace;
use cargo::core::shell::{ColorConfig, Verbosity};
use cargo::util::important_paths::find_root_manifest_for_wd;
use cargo::util::{CargoResult, CliError, Config};
use clap::{App, AppSettings, Arg, ArgMatches, SubCommand};

/// Options from CLI arguments
#[derive(Deserialize, Debug)]
pub struct Options {
    flag_color: Option<String>,
    flag_features: Vec<String>,
    flag_all_features: bool,
    flag_no_default_features: bool,
    flag_manifest_path: Option<String>,
    flag_quiet: Option<bool>,
    flag_verbose: u32,
    flag_frozen: bool,
    flag_locked: bool,
    flag_exit_code: i32,
    flag_packages: Vec<String>,
    flag_root: Option<String>,
    flag_depth: i32,
}

impl Options {
    fn from_matches(m: &ArgMatches) -> Options {
        Options {
            flag_color: m.value_of("color").map(String::from),
            flag_features: m.values_of("features")
                .map(|vals| vals.into_iter().map(String::from).collect())
                .unwrap_or_default(),
            flag_all_features: m.is_present("all-features"),
            flag_no_default_features: m.is_present("no-default-features"),
            flag_manifest_path: m.value_of("manifest-path").map(String::from),
            flag_quiet: if m.is_present("quiet") {
                Some(true)
            } else {
                None
            },
            flag_verbose: m.occurrences_of("verbose") as u32,
            flag_frozen: m.is_present("frozen"),
            flag_locked: m.is_present("locked"),
            flag_exit_code: m.value_of("exit-code")
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| 0_i32),
            flag_packages: m.values_of("packages")
                .map(|vals| vals.into_iter().map(String::from).collect())
                .unwrap_or_default(),
            flag_root: m.value_of("root").map(String::from),
            flag_depth: if m.is_present("root-deps-only") {
                1
            } else {
                m.value_of("depth")
                    .as_ref()
                    .and_then(|v| v.parse::<i32>().ok())
                    .unwrap_or_else(|| -1_i32)
            },
        }
    }
}

fn main() {
    env_logger::init().unwrap();

    let config = match Config::default() {
        Ok(cfg) => cfg,
        Err(e) => {
            let mut shell = cargo::shell(Verbosity::Verbose, ColorConfig::Auto);
            cargo::exit_with_error(e.into(), &mut shell)
        }
    };

    let m = App::new("cargo-outdated")
        .author("Kevin K. <kbknapp@gmail.com>")
        .about("Displays information about project dependency versions")
        .version(concat!("v", crate_version!()))
        .bin_name("cargo")
        .settings(&[
            AppSettings::GlobalVersion,
            AppSettings::SubcommandRequired,
        ])
        .subcommand(
            SubCommand::with_name("outdated")
                .about("Displays information about project dependency versions")
                .arg(
                    Arg::with_name("quiet")
                        .long("quiet")
                        .short("q")
                        .help("Coloring: auto, always, never"),
                )
                .arg(
                    Arg::with_name("color")
                        .long("color")
                        .help("Coloring: auto, always, never")
                        .takes_value(true)
                        .number_of_values(1)
                        .possible_values(&["auto", "always", "never"])
                        .default_value("auto"),
                )
                .arg(
                    Arg::with_name("features")
                        .long("features")
                        .help("Space-separated list of features")
                        .takes_value(true)
                        .value_name("FEATURE")
                        .value_delimiter(" ")
                        .conflicts_with_all(&["all-features", "no-default-features"]),
                )
                .arg(
                    Arg::with_name("all-features")
                        .long("all-features")
                        .help("Check outdated packages with all features enabled")
                        .conflicts_with_all(&["features", "no-default-features"]),
                )
                .arg(
                    Arg::with_name("no-default-features")
                        .long("no-default-features")
                        .help("Do not include the `default` feature")
                        .conflicts_with_all(&["features", "all-features"]),
                )
                .arg(
                    Arg::with_name("packages")
                        .long("packages")
                        .short("p")
                        .help("Package to inspect for updates")
                        .takes_value(true)
                        .value_name("PKG")
                        .value_delimiter(" ")
                        .multiple(true),
                )
                .arg(
                    Arg::with_name("root")
                        .long("root")
                        .short("r")
                        .help("Package to treat as the root package")
                        .takes_value(true)
                        .value_name("ROOT")
                        .number_of_values(1),
                )
                .arg(
                    Arg::with_name("verbose")
                        .long("verbose")
                        .short("v")
                        .help("Use verbose output")
                        .multiple(true),
                )
                .arg(
                    Arg::with_name("depth")
                        .long("depth")
                        .short("d")
                        .long_help(
                            "How deep in the dependency chain to search \
                             (Defaults to all dependencies when omitted)",
                        )
                        .takes_value(true)
                        .value_name("NUM")
                        .number_of_values(1),
                )
                .arg(
                    Arg::with_name("exit-code")
                        .long("exit-code")
                        .help("The exit code to return on new versions found")
                        .takes_value(true)
                        .value_name("NUM")
                        .number_of_values(1)
                        .default_value("0"),
                )
                .arg(
                    Arg::with_name("root-deps-only")
                        .long("root-deps-only")
                        .short("R")
                        .help("Only check root dependencies (Equivalent to --depth=1)")
                        .conflicts_with("depth"),
                )
                .arg(
                    Arg::with_name("manifest-path")
                        .long("manifest-path")
                        .short("m")
                        .long_help(
                            "An absolute path to the Cargo.toml file to use \
                             (Defaults to Cargo.toml in project root)",
                        )
                        .takes_value(true)
                        .value_name("PATH")
                        .number_of_values(1)
                        .validator(is_file),
                )
                .arg(
                    Arg::with_name("frozen")
                        .long("frozen")
                        .help("Require Cargo.lock and cache are up to date"),
                )
                .arg(
                    Arg::with_name("locked")
                        .long("locked")
                        .help("Require Cargo.lock is up to date"),
                ),
        )
        .get_matches();
    let m = m.subcommand_matches("outdated")
        .expect("Subcommand outdated not found");
    let options = Options::from_matches(m);
    let exit_code = options.flag_exit_code;
    let result = execute(options, &config);
    match result {
        Err(e) => {
            let cli_error = CliError::new(e, 1);
            cargo::exit_with_error(cli_error, &mut *config.shell())
        }
        Ok(i) => if i > 0 {
            std::process::exit(exit_code);
        } else {
            std::process::exit(0);
        },
    }
}

#[allow(unknown_lints)]
#[allow(needless_pass_by_value)]
pub fn execute(options: Options, config: &Config) -> CargoResult<i32> {
    config.configure(
        options.flag_verbose,
        options.flag_quiet,
        &options.flag_color,
        options.flag_frozen,
        options.flag_locked,
    )?;
    debug!(config, format!("options: {:?}", options));

    verbose!(config, "Parsing...", "current workspace");
    let curr_workspace = {
        let curr_manifest =
            find_root_manifest_for_wd(options.flag_manifest_path.clone(), config.cwd())?;
        Workspace::new(&curr_manifest, config)?
    };
    verbose!(config, "Resolving...", "current workspace");
    let mut ela_curr = ElaborateWorkspace::from_workspace(&curr_workspace, &options)?;

    verbose!(config, "Parsing...", "compat workspace");
    let mut compat_proj = TempProject::from_workspace(&ela_curr, config)?;
    compat_proj.write_manifest_semver()?;
    verbose!(config, "Updating...", "compat workspace");
    compat_proj.cargo_update(config)?;
    verbose!(config, "Resolving...", "compat workspace");
    let ela_compat = ElaborateWorkspace::from_workspace(&compat_proj.workspace, &options)?;

    verbose!(config, "Parsing...", "latest workspace");
    let mut latest_proj = TempProject::from_workspace(&ela_curr, config)?;
    latest_proj.write_manifest_latest()?;
    verbose!(config, "Updating...", "latest workspace");
    latest_proj.cargo_update(config)?;
    verbose!(config, "Resolving...", "latest workspace");
    let ela_latest = ElaborateWorkspace::from_workspace(&latest_proj.workspace, &options)?;

    verbose!(config, "Resolving...", "package status");
    ela_curr
        .resolve_status(&ela_compat, &ela_latest, &options, config)?;

    let count = ela_curr.print_list(&options, config)?;

    Ok(count)
}

#[allow(unknown_lints)]
#[allow(needless_pass_by_value)]
fn is_file(s: String) -> Result<(), String> {
    let p = Path::new(&*s);
    if p.file_name().is_none() {
        return Err(format!("'{}' doesn't appear to be a valid file name", &*s));
    }
    Ok(())
}
