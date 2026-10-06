//! Parsing Options.
//! `--detector-kind {kind}` or `-k`, currently support only deadlock
//! `--blacklist-mode` or `-b`, sets backlist than the default whitelist.
//! `--crate-name-list [crate1,crate2]` or `-l`, white or black lists of crates decided by `-b`.
//! if `-l` not specified, then do not white-or-black list the crates.
//! `--exit-code-on-bug`, exit with a non-zero code when at least one bug is reported.
//! `--format {json|sarif}`, report output format; `json` (default) prints the
//! native report records, `sarif` prints a SARIF 2.1.0 log instead.
use clap::{Arg, ArgAction, Command};
use std::error::Error;

#[derive(Debug)]
pub enum CrateNameList {
    White(Vec<String>),
    Black(Vec<String>),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Native report records (the historical output).
    #[default]
    Json,
    /// One SARIF 2.1.0 log per analyzed crate, on the same log channel.
    Sarif,
}

impl Default for CrateNameList {
    fn default() -> Self {
        CrateNameList::White(Vec::new())
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum DetectorKind {
    All,
    Deadlock,
    AtomicityViolation,
    Memory,
    Panic,
    // More to be supported.
}

fn make_options_parser() -> Command {
    Command::new("LOCKBUD")
        .no_binary_name(true)
        .version("v0.2.0")
        .arg(
            Arg::new("kind")
                .short('k')
                .long("detector-kind")
                .action(ArgAction::Set)
                .value_parser(["deadlock", "atomicity_violation", "memory", "all", "panic"])
                .default_value("deadlock")
                .help("The detector kind"),
        )
        .arg(
            Arg::new("black")
                .short('b')
                .long("blacklist-mode")
                .action(ArgAction::SetTrue)
                .help("set `crates` as blacklist than whitelist"),
        )
        .arg(
            Arg::new("crates")
                .short('l')
                .long("crate-name-list")
                .action(ArgAction::Set)
                .num_args(1)
                .help("The crate names seperated by ,"),
        )
        .arg(
            Arg::new("exit_code_on_bug")
                .long("exit-code-on-bug")
                .action(ArgAction::SetTrue)
                .help("Exit with a non-zero code when at least one bug is reported"),
        )
        .arg(
            Arg::new("format")
                .long("format")
                .action(ArgAction::Set)
                .value_parser(["json", "sarif"])
                .default_value("json")
                .help("Report output format: json (native records) or sarif (SARIF 2.1.0 log)"),
        )
}

#[derive(Debug)]
pub struct Options {
    pub detector_kind: DetectorKind,
    pub crate_name_list: CrateNameList,
    /// Exit the compiler with a non-zero code when at least one bug is
    /// reported, so CI pipelines can fail on detections.
    pub exit_code_on_bug: bool,
    pub format: OutputFormat,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            detector_kind: DetectorKind::Deadlock,
            crate_name_list: CrateNameList::Black(Vec::new()),
            exit_code_on_bug: false,
            format: OutputFormat::Json,
        }
    }
}

impl Options {
    pub fn parse_from_str(s: &str) -> Result<Self, Box<dyn Error>> {
        let flags = shellwords::split(s)?;
        Self::parse_from_args(&flags)
    }

    pub fn parse_from_args(flags: &[String]) -> Result<Self, Box<dyn Error>> {
        let app = make_options_parser();
        let matches = app.try_get_matches_from(flags.iter())?;
        let detector_kind = match matches.get_one::<String>("kind").map(String::as_str) {
            Some("deadlock") => DetectorKind::Deadlock,
            Some("atomicity_violation") => DetectorKind::AtomicityViolation,
            Some("memory") => DetectorKind::Memory,
            Some("all") => DetectorKind::All,
            Some("panic") => DetectorKind::Panic,
            _ => return Err("UnsupportedDetectorKind")?,
        };
        let black = matches.get_flag("black");
        let crate_name_list = matches
            .get_one::<String>("crates")
            .map(|crates| {
                let crates: Vec<String> = crates.split(',').map(|s| s.into()).collect();
                if black {
                    CrateNameList::Black(crates)
                } else {
                    CrateNameList::White(crates)
                }
            })
            .unwrap_or_default();
        let format = match matches.get_one::<String>("format").map(String::as_str) {
            Some("sarif") => OutputFormat::Sarif,
            _ => OutputFormat::Json,
        };
        Ok(Options {
            detector_kind,
            crate_name_list,
            exit_code_on_bug: matches.get_flag("exit_code_on_bug"),
            format,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_from_str_blacklist_ok() {
        let options = Options::parse_from_str("-k deadlock -b -l cc,tokio_util,indicatif").unwrap();
        assert!(matches!(options.detector_kind, DetectorKind::Deadlock));
        assert!(
            matches!(options.crate_name_list, CrateNameList::Black(v) if v == vec!["cc".to_owned(), "tokio_util".to_owned(), "indicatif".to_owned()])
        );
    }

    #[test]
    fn test_parse_from_str_whitelist_ok() {
        let options = Options::parse_from_str("-k deadlock -l cc,tokio_util,indicatif").unwrap();
        assert!(matches!(options.detector_kind, DetectorKind::Deadlock));
        assert!(
            matches!(options.crate_name_list, CrateNameList::White(v) if v == vec!["cc".to_owned(), "tokio_util".to_owned(), "indicatif".to_owned()])
        );
    }

    #[test]
    fn test_parse_from_str_exit_code_on_bug() {
        let options = Options::parse_from_str("-k deadlock --exit-code-on-bug").unwrap();
        assert!(options.exit_code_on_bug);
        let options = Options::parse_from_str("-k deadlock").unwrap();
        assert!(!options.exit_code_on_bug);
    }

    #[test]
    fn test_parse_from_str_format() {
        let options = Options::parse_from_str("-k deadlock --format sarif").unwrap();
        assert_eq!(options.format, OutputFormat::Sarif);
        let options = Options::parse_from_str("-k deadlock").unwrap();
        assert_eq!(options.format, OutputFormat::Json);
        let options = Options::parse_from_str("-k deadlock --format json").unwrap();
        assert_eq!(options.format, OutputFormat::Json);
        assert!(Options::parse_from_str("-k deadlock --format yaml").is_err());
    }

    #[test]
    fn test_parse_from_str_err() {
        let options = Options::parse_from_str("-k unknown -b -l cc,tokio_util,indicatif");
        assert!(options.is_err());
    }

    #[test]
    fn test_parse_from_args_blacklist_ok() {
        let options = Options::parse_from_args(&[
            "-k".to_owned(),
            "deadlock".to_owned(),
            "-b".to_owned(),
            "-l".to_owned(),
            "cc,tokio_util,indicatif".to_owned(),
        ])
        .unwrap();
        assert!(matches!(options.detector_kind, DetectorKind::Deadlock));
        assert!(
            matches!(options.crate_name_list, CrateNameList::Black(v) if v == vec!["cc".to_owned(), "tokio_util".to_owned(), "indicatif".to_owned()])
        );
    }

    #[test]
    fn test_parse_from_args_whitelist_ok() {
        let options = Options::parse_from_args(&[
            "-k".to_owned(),
            "deadlock".to_owned(),
            "-l".to_owned(),
            "cc,tokio_util,indicatif".to_owned(),
        ])
        .unwrap();
        assert!(matches!(options.detector_kind, DetectorKind::Deadlock));
        assert!(
            matches!(options.crate_name_list, CrateNameList::White(v) if v == vec!["cc".to_owned(), "tokio_util".to_owned(), "indicatif".to_owned()])
        );
    }

    #[test]
    fn test_parse_from_args_err() {
        let options = Options::parse_from_args(&[
            "-k".to_owned(),
            "unknown".to_owned(),
            "-b".to_owned(),
            "-l".to_owned(),
            "cc,tokio_util,indicatif".to_owned(),
        ]);
        assert!(options.is_err());
    }
}
