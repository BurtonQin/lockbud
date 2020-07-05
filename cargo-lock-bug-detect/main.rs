use std::ffi::OsString;
use std::env;
use std::process::Command;
use rustc_version::VersionMeta;

const CARGO_LOCK_BUG_DETECTOR_HELP: &str = r#"Detect double-lock&conflict-lock on MIR
Usage:
    cargo lock-bug-detect [subcommand] [<cargo options>...] [--] [<program/test suite options>...]
Subcommands:
    double-lock              Detect double-lock bugs
    conflict-lock            Detect conflict-lock bugs
    setup                    Only perform automatic setup, but without asking questions (for getting a proper libstd)
Common options:
    -h, --help               Print this message
    -V, --version            Print version info and exit
Other [options] are the same as `cargo check`. Everything after the second "--" verbatim
to the program.
Examples:
    cargo lock-bug-detect double-lock
    cargo lock-bug-detect conflict-lock
"#;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum LockBugDetectCommand {
    DoubleLock,
    ConflictLock,
}

fn show_help() {
    println!("{}", CARGO_LOCK_BUG_DETECTOR_HELP);
}

fn show_version() {
    println!("lock-bug-detect {}", "0.1.0");
    //println!(
    //    "lock-bug-detect {} ({} {})",
    //    env!("CARGO_PKG_VERSION"),
    //    env!("VERGEN_SHA_SHORT"),
    //    env!("VERGEN_COMMIT_DATE")
    //);
}

fn show_error(msg: String) -> ! {
    eprintln!("fatal error: {}", msg);
    std::process::exit(1)
}

fn cargo() -> Command {
    Command::new(env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo")))
}

// Determines whether a `--flag` is present.
fn has_arg_flag(name: &str) -> bool {
    let mut args = std::env::args().take_while(|val| val != "--");
    args.any(|val| val == name)
}

/// Gets the value of a `--flag`.
fn get_arg_flag_value(name: &str) -> Option<String> {
    // Stop searching at `--`.
    let mut args = std::env::args().take_while(|val| val != "--");
    loop {
        let arg = match args.next() {
            Some(arg) => arg,
            None => return None,
        };
        if !arg.starts_with(name) {
            continue;
        }
        // Strip leading `name`.
        let suffix = &arg[name.len()..];
        if suffix.is_empty() {
            // This argument is exactly `name`; the next one is the value.
            return args.next();
        } else if suffix.starts_with('=') {
            // This argument is `name=value`; get the value.
            // Strip leading `=`.
            return Some(suffix[1..].to_owned());
        }
    }
}

fn in_cargo_lock_bug_detect() {
    let (subcommand, skip) = match std::env::args().nth(2).as_deref() {
        Some("double-lock") => (LockBugDetectCommand::DoubleLock, 3),
        Some("conflict-lock") => (LockBugDetectCommand::ConflictLock, 3),
        // Default double-lock
        None => (LockBugDetectCommand::DoubleLock, 2),
        // Invalid command.
        Some(s) => show_error(format!("Unknown command `{}`", s)),
    };
    // Now we run `cargo check $FLAGS $ARGS`, giving the user the
    // change to add additional arguments. `FLAGS` is set to identify
    // this target.  The user gets to control what gets actually passed to lock-bug-detect.
    let mut cmd = cargo();
    cmd.arg("check");
        match subcommand {
        LockBugDetectCommand::DoubleLock => cmd.env("RUST_LOCK_DETECTOR_TYPE", "DoubleLockDetector"),
        LockBugDetectCommand::ConflictLock => cmd.env("RUST_LOCK_DETECTOR_TYPE", "ConflictLockDetector"),
    };
    cmd.env("RUSTC", "/home/boqin/Projects/HackRust/rust-lock-bug-detector/target/release/rust-lock-bug-detector");
    cmd.env("RUST_BACKTRACE", "full");
    let mut args = std::env::args().skip(skip);
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        cmd.arg(arg);
    }
    cmd.env("RUST_LOCK_DETECTOR_BLACK_LISTS", "cc");
    println!("{:?}", cmd);
    let exit_status =
           cmd.spawn().expect("could not run cargo").wait().expect("failed to wait for cargo?");

    if !exit_status.success() {
        std::process::exit(exit_status.code().unwrap_or(-1))
    };
}

fn setup() {
    todo!{};
}

fn main() {
    if has_arg_flag("--help") || has_arg_flag("-h") {
        show_help();
        return;
    }
    if has_arg_flag("--version") || has_arg_flag("-V") {
        show_version();
        return;
    }
    if let Some("lock-bug-detect") = std::env::args().nth(1).as_deref() {
        in_cargo_lock_bug_detect();
    }
}