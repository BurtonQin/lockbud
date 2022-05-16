use clap::{Arg, Command};
use std::error::Error;

#[derive(Debug)]
pub enum CrateNameList {
    White(Vec<String>),
    Black(Vec<String>),
}

impl Default for CrateNameList {
    fn default() -> Self {
        CrateNameList::Black(Vec::new())
    }
}

#[derive(Debug)]
pub enum DetectorKind {
    DoubleLock,
    ConflictLock,
}

fn make_options_parser<'help>() -> Command<'help> {
    let parser = Command::new("LOCKBUD")
        .no_binary_name(true)
        .version("v0.2.0")
        .arg(
            Arg::new("kind")
                .short('k')
                .long("detector_kind")
                .possible_values(&["doublelock", "conflictlock"])
                .default_values(&["doublelock"])
                .help("The detector kind"),
        )
        .arg(
            Arg::new("black")
                .short('b')
                .long("blacklist_mode")
                .takes_value(false)
                .help("set `crates` as blacklist than whitelist"),
        )
        .arg(
            Arg::new("crates")
                .short('l')
                .long("crate_name_list")
                .takes_value(true)
                .help("The crate names eperated by ,"),
        )
        .arg(
            Arg::new("depth")
                .short('d')
                .long("callchain_depth")
                .takes_value(true)
                .default_value("4")
                .help("The callchain depth for inter-procedural analysis"),
        )
        .arg(
            Arg::new("iternum")
                .short('n')
                .long("max_iter_num")
                .takes_value(true)
                .default_value("10000")
                .help("Then GenKill iteration inside one function"),
        );
    parser
}

#[derive(Debug)]
pub struct Options {
    pub detector_kind: DetectorKind,
    pub crate_name_list: CrateNameList,
    pub callchain_depth: u32,
    pub max_iter_num: u32,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            detector_kind: DetectorKind::DoubleLock,
            crate_name_list: CrateNameList::Black(Vec::new()),
            callchain_depth: 4,
            max_iter_num: 10000,
        }
    }
}

impl Options {
    pub fn parse_from_str(s: &str) -> Result<Self, Box<dyn Error>> {
        let flags = shellwords::split(s)?;
        let app = make_options_parser();
        let matches = app.try_get_matches_from(flags.iter())?;
        let detector_kind = match matches.value_of("kind") {
            Some("doublelock") => DetectorKind::DoubleLock,
            Some("conflictlock") => DetectorKind::ConflictLock,
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
        let callchain_depth: u32 = matches
            .value_of("depth")
            .map(|s| s.parse::<u32>())
            .unwrap_or(Ok(4))?;
        let max_iter_num: u32 = matches
            .value_of("iternum")
            .map(|s| s.parse::<u32>())
            .unwrap_or(Ok(10000))?;
        Ok(Options {
            detector_kind,
            crate_name_list,
            callchain_depth,
            max_iter_num,
        })
    }
}
