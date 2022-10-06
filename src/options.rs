//! Parsing Options.
//! `--detector-kind {kind}` or `-k`, currently support only deadlock
//! `--blacklist-mode` or `-b`, sets backlist than the default whitelist.
//! `--crate-name-list [crate1,crate2]` or `-l`, white or black lists of crates decided by `-b`.
//! if `-l` not specified, then do not white-or-black list the crates.
use clap::{Arg, Command};
use std::error::Error;

#[derive(Debug)]
pub enum CrateNameList {
    White(Vec<String>),
    Black(Vec<String>),
}

impl Default for CrateNameList {
    fn default() -> Self {
        CrateNameList::White(Vec::new())
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum DetectorKind {
    Deadlock,
    // More to be supported.
}

fn make_options_parser<'help>() -> Command<'help> {
    let parser = Command::new("LOCKBUD")
        .no_binary_name(true)
        .version("v0.2.0")
        .arg(
            Arg::new("kind")
                .short('k')
                .long("detector-kind")
                .possible_values(["deadlock"])
                .default_values(&["deadlock"])
                .help("The detector kind"),
        )
        .arg(
            Arg::new("black")
                .short('b')
                .long("blacklist-mode")
                .takes_value(false)
                .help("set `crates` as blacklist than whitelist"),
        )
        .arg(
            Arg::new("crates")
                .short('l')
                .long("crate-name-list")
                .takes_value(true)
                .help("The crate names seperated by ,"),
        );
    parser
}

#[derive(Debug)]
pub struct Options {
    pub detector_kind: DetectorKind,
    pub crate_name_list: CrateNameList,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            detector_kind: DetectorKind::Deadlock,
            crate_name_list: CrateNameList::Black(Vec::new()),
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
        let detector_kind = match matches.value_of("kind") {
            Some("deadlock") => DetectorKind::Deadlock,
            _ => return Err("UnsupportedDetectorKind")?,
        };
        let black = matches.is_present("black");
        let crate_name_list = matches
            .value_of("crates")
            .map(|crates| {
                let crates: Vec<String> = crates.split(',').map(|s| s.into()).collect();
                if black {
                    CrateNameList::Black(crates)
                } else {
                    CrateNameList::White(crates)
                }
            })
            .unwrap_or_default();
        Ok(Options {
            detector_kind,
            crate_name_list,
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
