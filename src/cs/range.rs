extern crate rustc_hir;
extern crate rustc_middle;
extern crate rustc_span;
use rustc_middle::mir::{Body, Location, START_BLOCK, TerminatorKind};
use rustc_span::Span;

use std::cmp::Ordering;

use std::fmt;

use std::collections::{HashMap, HashSet};

/// A position in a file: (line num, column num)
#[derive(Eq, Clone, Copy, Hash, Debug)]
pub struct PosInFile(pub u32, pub u32);

impl PartialEq for PosInFile {
    fn eq(&self, other: &PosInFile) -> bool {
        self.0 == other.0 && self.1 == other.1
    }
}

impl PartialOrd for PosInFile {
    fn partial_cmp(&self, other: &PosInFile) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for PosInFile {
    fn cmp(&self, other: &PosInFile) -> Ordering {
        if self.0 != other.0 {
            self.0.cmp(&other.0)
        } else {
            self.1.cmp(&other.1)
        }
    }
}
/// A range in a file: (begin pos, end pos).
#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug)]
pub struct RangeInFile(pub PosInFile, pub PosInFile);

impl RangeInFile {
    fn union_in_place(&mut self, other: &RangeInFile) -> bool {
        if other.1 < self.0 || self.1 < other.0 {
            false
        } else if self.0 <= other.1 && other.1 <= self.1 {
            if other.0 < self.0 {
                self.0 = other.0;
            }
            true
        } else if self.1 < other.1 {
            if other.0 < self.0 {
                self.0 = other.0;
            }
            self.1 = other.1;
            true
        } else {
            false
        }
    }
}

impl fmt::Display for RangeInFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}:{}", (self.0).0, (self.0).1, (self.1).0, (self.1).1)
    }
}
/// The lifetime of a variable may span multiple ranges in a file.
#[derive(Default, Debug)]
struct RangesInFile {
    ranges: HashSet<RangeInFile>,
}

impl RangesInFile {
    fn new(ranges: HashSet<RangeInFile>) -> Self {
        Self {
            ranges
        }
    }
    #[allow(dead_code)]
    fn add(&mut self, range: RangeInFile) {
        self.ranges.insert(range);
    }
    #[allow(dead_code)]
    fn merge(self) -> Vec<RangeInFile> {
        let mut result: Vec<RangeInFile> = Vec::new();
        let mut worklist: Vec<RangeInFile> = self.ranges.into_iter().collect();
        while let Some(mut cur) = worklist.pop() {
            let old_len = worklist.len();
            worklist.retain(|r| 
                if cur.union_in_place(r) {
                    false
                } else {
                    true
                }
            );
            if old_len != worklist.len() {
               worklist.push(cur);
            } else {
                result.push(cur);
            }
        }
        result
    }
}
/// The lifetime of a variable may span across files.
#[derive(Default, Debug)]
pub struct RangesAcrossFiles {
    ranges: HashMap<String, HashSet<RangeInFile>>,
}

impl RangesAcrossFiles {
    #[allow(dead_code)]
    pub fn add_locs(&mut self, locs: &HashSet<Location>, body: &Body) {
        for loc in locs {
            if let Some((filename, range)) = parse_span(&get_span(loc, body)) {
                self.ranges.entry(filename).or_insert_with(HashSet::new).insert(range);
            }
        }
    }
    #[allow(dead_code)]
    pub fn merge(self) -> HashMap<String, Vec<RangeInFile>> {
        self.ranges.into_iter().map(|(filename, file_ranges)| 
            (filename, RangesInFile::new(file_ranges).merge())
        ).collect()
    }
}


#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_ranges_in_file() {
        let rg1 = RangeInFile(PosInFile(4, 13), PosInFile(7, 6));
        let rg2 = RangeInFile(PosInFile(8, 5), PosInFile(8, 23));
        let rg3 = RangeInFile(PosInFile(4, 9), PosInFile(4, 10));
        let rg4 = RangeInFile(PosInFile(9, 1), PosInFile(9, 2));
        let mut ranges_in_file: RangesInFile = Default::default();
        ranges_in_file.add(rg1);
        ranges_in_file.add(rg2);
        ranges_in_file.add(rg3);
        ranges_in_file.add(rg4);
        let ranges = ranges_in_file.merge();
        println!("{:#?}", ranges);
    }

    #[test]
    fn test_parse_span_str() {
        assert_eq!(parse_span_str("src/main.rs:8:14: 8:20"), RangeInFile(PosInFile(8, 14), PosInFile(8, 20)));
    }
    
    #[test]
    fn test_ranges_in_file_many() {
        let mut ranges_in_file: RangesInFile = Default::default();
        let spans_str = ["src/main.rs:8:14: 8:20","src/main.rs:8:18: 8:19","src/main.rs:5:25: 5:26","src/main.rs:6:17: 6:32","src/main.rs:5:39: 5:40","src/main.rs:8:20: 8:21","src/main.rs:4:9: 4:10","src/main.rs:8:5: 8:23","src/main.rs:5:9: 5:16","src/main.rs:5:25: 5:26","src/main.rs:4:19: 4:23","src/main.rs:5:20: 5:40","src/main.rs:5:39: 5:40","src/main.rs:8:14: 8:20","src/main.rs:5:14: 5:15","src/main.rs:9:1: 9:2","src/main.rs:5:40: 5:41","src/main.rs:8:18: 8:19","src/main.rs:5:9: 5:16","src/main.rs:5:14: 5:15","src/main.rs:5:39: 5:40","src/main.rs:8:19: 8:20","src/main.rs:5:39: 5:40","src/main.rs:4:13: 7:6"];
        for span_str in &spans_str {
            let range = parse_span_str(span_str);
            ranges_in_file.add(range);
        }
        println!("{:#?}", ranges_in_file.merge());
    }
}

#[allow(dead_code)]
pub fn merge_spans(locs: &HashSet<Location>, body: &Body) {
    let mut spans: HashSet<Span> = HashSet::new();
    for loc in locs {
        spans.insert(get_span(loc, body));
    }
    println!("{:#?}", spans);
}

// e.g.
// src/main.rs:4:13: 7:6
#[allow(dead_code)]
fn parse_span_str(span_str: &str) -> RangeInFile {
    let labels: Vec<&str> = span_str.split(":").collect();
    assert!(labels.len() == 5);
    let _filename = labels[0];
    let line_0: u32 = labels[1].parse().unwrap();
    let col_0: u32 = labels[2].parse().unwrap();
    let line_1: u32 = labels[3][1..].parse().unwrap();
    let col_1: u32 = labels[4].parse().unwrap();
    RangeInFile(PosInFile(line_0, col_0), PosInFile(line_1, col_1))
}

pub fn parse_span(span: &Span) -> Option<(String, RangeInFile)> {
    let span_str = format!("{:?}", span);
    let labels: Vec<&str> = span_str.split(":").collect();
    if labels.len() != 5 {
        return None;
    }
    let filename = labels[0];
    let abs_file = std::fs::canonicalize(&filename).unwrap();
   
    let line_0: u32 = labels[1].parse().unwrap();
    let col_0: u32 = labels[2].parse().unwrap();
    let line_1: u32 = labels[3][1..].parse().unwrap();
    let last_part_end = labels[4].find(" ").unwrap();
    let col_1: u32 = labels[4][..last_part_end].parse().unwrap();
    Some((abs_file.into_os_string().into_string().unwrap(), RangeInFile(PosInFile(line_0, col_0), PosInFile(line_1, col_1))))
}

#[allow(dead_code)]
fn get_span(loc: &Location, body: &Body) -> Span {
    let bb_data = &body.basic_blocks()[loc.block];
    if loc.statement_index < bb_data.statements.len() {
        bb_data.statements[loc.statement_index].source_info.span
    } else {
        bb_data.terminator().source_info.span
    }
}
