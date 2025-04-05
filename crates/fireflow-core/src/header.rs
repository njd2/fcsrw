use crate::error::*;
use crate::segment::*;

use regex::Regex;
use serde::Serialize;
use std::fmt;
use std::io::{BufReader, Read};
use std::str;

/// All FCS versions this library supports.
///
/// This appears as the first 6 bytes of any valid FCS file.
#[derive(Debug, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Serialize)]
pub enum Version {
    FCS2_0,
    FCS3_0,
    FCS3_1,
    FCS3_2,
}

pub struct VersionError;

/// Output from parsing the FCS header.
///
/// Includes version and the three main segments (TEXT, DATA, ANALYSIS). For
/// now, OTHER segments are ignored. This may change in the future. Segments may
/// or may not be adjusted using configuration parameters to correct for errors.
///
/// Only valid segments are to be put in this struct (ie begin <= end).
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    pub version: Version,
    pub text: Segment,
    pub data: Segment,
    pub analysis: Segment,
}

pub fn h_read_header<R: Read>(h: &mut BufReader<R>) -> ImpureResult<Header> {
    let mut verbuf = [0; HEADERLEN];
    h.read_exact(&mut verbuf)?;
    if let Ok(hs) = str::from_utf8(&verbuf) {
        let succ = parse_header(hs)?;
        Ok(succ)
    } else {
        Err(Failure::new("HEADER is not valid text".to_string()))?
    }
}

fn parse_header_offset(s: &str, allow_blank: bool) -> Option<u32> {
    if allow_blank && s.trim().is_empty() {
        return Some(0);
    }
    let re = Regex::new(r" *(\d+)").unwrap();
    re.captures(s).map(|c| {
        // ASSUME this won't fail since the regexp has one field
        let [i] = c.extract().1;
        // ASSUME this won't fail since the regexp capture only matches digits
        i.parse().unwrap()
    })
}

fn parse_bounds(s0: &str, s1: &str, allow_blank: bool, id: SegmentId) -> PureMaybe<Segment> {
    let parse_one = |s, which| {
        PureMaybe::from_result_1(
            parse_header_offset(s, allow_blank).ok_or(format!(
                "could not parse {which} offset for {id} segment; value was '{s}'"
            )),
            PureErrorLevel::Error,
        )
    };
    let begin_res = parse_one(s0, "begin");
    let end_res = parse_one(s1, "end");
    begin_res
        .combine(end_res, |b, e| (b, e))
        .and_then(|(b, e)| {
            if let (Some(begin), Some(end)) = (b, e) {
                PureMaybe::from_result_1(
                    // TODO adjust these
                    Segment::try_new(begin, end, 0, 0, id),
                    PureErrorLevel::Error,
                )
            } else {
                PureMaybe::empty()
            }
        })
}

const HEADER_PAT: &str = r"(.{6})    (.{8})(.{8})(.{8})(.{8})(.{8})(.{8})";

fn parse_header(s: &str) -> PureResult<Header> {
    // ASSUME this will always work, if not the regexp is invalid
    let re = Regex::new(HEADER_PAT).unwrap();
    if let Some(cap) = re.captures(s) {
        // ASSUME this will always work since the regexp has 7 fields
        let [v, t0, t1, d0, d1, a0, a1] = cap.extract().1;
        let vers_succ = PureMaybe::from_result_1(
            v.parse::<Version>().map_err(|e| e.to_string()),
            PureErrorLevel::Error,
        );
        let text_succ = parse_bounds(t0, t1, false, SegmentId::PrimaryText);
        let data_succ = parse_bounds(d0, d1, false, SegmentId::Data);
        let anal_succ = parse_bounds(a0, a1, true, SegmentId::Analysis);
        let succ = vers_succ.combine4(text_succ, data_succ, anal_succ, |v, t, d, a| {
            if let (Some(version), Some(text), Some(data), Some(analysis)) = (v, t, d, a) {
                Some(Header {
                    version,
                    text,
                    data,
                    analysis,
                })
            } else {
                None
            }
        });
        PureMaybe::into_result(succ, "could not parse HEADER fields".to_string())
    } else {
        Err(Failure::new("could not parse HEADER".to_string()))
    }
}

const HEADERLEN: usize = 58;

impl str::FromStr for Version {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "FCS2.0" => Ok(Version::FCS2_0),
            "FCS3.0" => Ok(Version::FCS3_0),
            "FCS3.1" => Ok(Version::FCS3_1),
            "FCS3.2" => Ok(Version::FCS3_2),
            _ => Err(VersionError),
        }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Version::FCS2_0 => write!(f, "FCS2.0"),
            Version::FCS3_0 => write!(f, "FCS3.0"),
            Version::FCS3_1 => write!(f, "FCS3.1"),
            Version::FCS3_2 => write!(f, "FCS3.2"),
        }
    }
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "could not parse FCS Version")
    }
}
