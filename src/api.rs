use crate::keywords::*;
use crate::numeric::{Endian, IntMath, NumProps, Series};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use itertools::Itertools;
use regex::Regex;
use serde::ser::SerializeStruct;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::iter;
use std::num::{IntErrorKind, ParseFloatError, ParseIntError};
use std::path;
use std::str;
use std::str::FromStr;

fn format_measurement(n: &str, m: &str) -> String {
    format!("$P{}{}", n, m)
}

type ParseResult<T> = Result<T, String>;

#[derive(Debug, Clone, Serialize)]
struct FCSDateTime(DateTime<FixedOffset>);

struct FCSDateTimeError;

impl fmt::Display for FCSDateTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be formatted like 'yyyy-mm-ddThh:mm:ss[TZD]'")
    }
}

impl str::FromStr for FCSDateTime {
    type Err = FCSDateTimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let formats = [
            "%Y-%m-%dT%H:%M:%S%.f",
            "%Y-%m-%dT%H:%M:%S%.f%#z",
            "%Y-%m-%dT%H:%M:%S%.f%:z",
            "%Y-%m-%dT%H:%M:%S%.f%::z",
            "%Y-%m-%dT%H:%M:%S%.f%:::z",
        ];
        for f in formats {
            if let Ok(t) = DateTime::parse_from_str(s, f) {
                return Ok(FCSDateTime(t));
            }
        }
        Err(FCSDateTimeError)
    }
}

impl fmt::Display for FCSDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0.format("%Y-%m-%dT%H:%M:%S%.f%:z"))
    }
}

#[derive(Debug, Clone, Serialize)]
struct FCSTime(NaiveTime);

impl str::FromStr for FCSTime {
    type Err = FCSTimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveTime::parse_from_str(s, "%H:%M:%S")
            .map(FCSTime)
            .or(Err(FCSTimeError))
    }
}

impl fmt::Display for FCSTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0.format("%H:%M:%S"))
    }
}

struct FCSTimeError;

impl fmt::Display for FCSTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'hh:mm:ss'")
    }
}

#[derive(Debug, Clone, Serialize)]
struct FCSTime60(NaiveTime);

impl str::FromStr for FCSTime60 {
    type Err = FCSTime60Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveTime::parse_from_str(s, "%H:%M:%S")
            .or_else(|_| match s.split(":").collect::<Vec<_>>()[..] {
                [s1, s2, s3, s4] => {
                    let hh: u32 = s1.parse().or(Err(FCSTime60Error))?;
                    let mm: u32 = s2.parse().or(Err(FCSTime60Error))?;
                    let ss: u32 = s3.parse().or(Err(FCSTime60Error))?;
                    let tt: u32 = s4.parse().or(Err(FCSTime60Error))?;
                    let nn = tt * 1000000 / 60;
                    NaiveTime::from_hms_micro_opt(hh, mm, ss, nn).ok_or(FCSTime60Error)
                }
                _ => Err(FCSTime60Error),
            })
            .map(FCSTime60)
    }
}

impl fmt::Display for FCSTime60 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let base = self.0.format("%H:%M:%S");
        let cc = self.0.nanosecond() / 10000000 * 60;
        write!(f, "{}.{}", base, cc)
    }
}

struct FCSTime60Error;

impl fmt::Display for FCSTime60Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "must be like 'hh:mm:ss[:tt]' where 'tt' is in 1/60th seconds"
        )
    }
}

#[derive(Debug, Clone, Serialize)]
struct FCSTime100(NaiveTime);

impl str::FromStr for FCSTime100 {
    type Err = FCSTime100Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveTime::parse_from_str(s, "%H:%M:%S")
            .or_else(|_| {
                let re = Regex::new(r"(\d){2}:(\d){2}:(\d){2}.(\d){2}").unwrap();
                let cap = re.captures(s).ok_or(FCSTime100Error)?;
                let [s1, s2, s3, s4] = cap.extract().1;
                let hh: u32 = s1.parse().or(Err(FCSTime100Error))?;
                let mm: u32 = s2.parse().or(Err(FCSTime100Error))?;
                let ss: u32 = s3.parse().or(Err(FCSTime100Error))?;
                let tt: u32 = s4.parse().or(Err(FCSTime100Error))?;
                NaiveTime::from_hms_milli_opt(hh, mm, ss, tt * 10).ok_or(FCSTime100Error)
            })
            .map(FCSTime100)
    }
}

impl fmt::Display for FCSTime100 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let base = self.0.format("%H:%M:%S");
        let cc = self.0.nanosecond() / 10000000;
        write!(f, "{}.{}", base, cc)
    }
}

struct FCSTime100Error;

impl fmt::Display for FCSTime100Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'hh:mm:ss[.cc]'")
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
struct Segment {
    begin: u32,
    end: u32,
}

#[derive(Debug)]
enum SegmentId {
    PrimaryText,
    SupplementalText,
    Analysis,
    Data,
    // TODO add Other (which will be indexed I think)
}

#[derive(Debug)]
enum SegmentErrorKind {
    Range,
    Inverted,
}

impl fmt::Display for SegmentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let x = match self {
            SegmentId::PrimaryText => "TEXT",
            SegmentId::SupplementalText => "STEXT",
            SegmentId::Analysis => "ANALYSIS",
            SegmentId::Data => "DATA",
        };
        write!(f, "{x}")
    }
}

#[derive(Debug)]
struct SegmentError {
    offsets: Segment,
    begin_delta: i32,
    end_delta: i32,
    kind: SegmentErrorKind,
    id: SegmentId,
}

impl fmt::Display for SegmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let offset_text = |x, delta| {
            if delta == 0 {
                format!("{}", x)
            } else {
                format!("{} ({}))", x, delta)
            }
        };
        let begin_text = offset_text(self.offsets.begin, self.begin_delta);
        let end_text = offset_text(self.offsets.end, self.end_delta);
        let kind_text = match &self.kind {
            SegmentErrorKind::Range => "Offset out of range",
            SegmentErrorKind::Inverted => "Begin after end",
        };
        write!(
            f,
            "{kind_text} for {} segment; begin={begin_text}, end={end_text}",
            self.id,
        )
    }
}

impl Segment {
    fn try_new(begin: u32, end: u32, id: SegmentId) -> Result<Segment, String> {
        Self::try_new_adjusted(begin, end, 0, 0, id)
    }

    fn try_new_adjusted(
        begin: u32,
        end: u32,
        begin_delta: i32,
        end_delta: i32,
        id: SegmentId,
    ) -> Result<Segment, String> {
        let x = i64::from(begin) + i64::from(begin_delta);
        let y = i64::from(end) + i64::from(end_delta);
        let err = |kind| {
            Err(SegmentError {
                offsets: Segment { begin, end },
                begin_delta,
                end_delta,
                kind,
                id,
            }
            .to_string())
        };
        match (u32::try_from(x), u32::try_from(y)) {
            (Ok(new_begin), Ok(new_end)) => {
                if new_begin > new_end {
                    err(SegmentErrorKind::Inverted)
                } else {
                    Ok(Segment {
                        begin: new_begin,
                        end: new_end,
                    })
                }
            }
            (_, _) => err(SegmentErrorKind::Range),
        }
    }

    fn try_adjust(
        self,
        begin_delta: i32,
        end_delta: i32,
        id: SegmentId,
    ) -> Result<Segment, String> {
        Self::try_new_adjusted(self.begin, self.end, begin_delta, end_delta, id)
    }

    fn len(&self) -> u32 {
        self.end - self.begin
    }

    fn num_bytes(&self) -> u32 {
        self.len() + 1
    }

    // unset = both offsets are 0
    fn is_unset(&self) -> bool {
        self.begin == 0 && self.end == 0
    }
}

/// FCS version.
///
/// This appears as the first 6 bytes of any valid FCS file.
#[derive(Debug, Clone, Eq, PartialEq, PartialOrd, Ord, Serialize)]
enum Version {
    FCS2_0,
    FCS3_0,
    FCS3_1,
    FCS3_2,
}

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

struct VersionError;

/// Data contained in the FCS header.
#[derive(Debug, Clone, Serialize)]
pub struct Header {
    version: Version,
    text: Segment,
    data: Segment,
    analysis: Segment,
}

/// The four allowed datatypes for FCS data.
///
/// This is shown in the $DATATYPE keyword.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
enum AlphaNumType {
    Ascii,
    Integer,
    Single,
    Double,
}

impl FromStr for AlphaNumType {
    type Err = AlphaNumTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "I" => Ok(AlphaNumType::Integer),
            "F" => Ok(AlphaNumType::Single),
            "D" => Ok(AlphaNumType::Double),
            "A" => Ok(AlphaNumType::Ascii),
            _ => Err(AlphaNumTypeError),
        }
    }
}

impl fmt::Display for AlphaNumType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            AlphaNumType::Ascii => write!(f, "A"),
            AlphaNumType::Integer => write!(f, "I"),
            AlphaNumType::Single => write!(f, "F"),
            AlphaNumType::Double => write!(f, "D"),
        }
    }
}

struct AlphaNumTypeError;

impl fmt::Display for AlphaNumTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be one of 'I', 'F', 'D', or 'A'")
    }
}

/// The three numeric data types for the $PnDATATYPE keyword in 3.2+
#[derive(Debug, Clone, Copy, Serialize)]
enum NumType {
    Integer,
    Single,
    Double,
}

impl FromStr for NumType {
    type Err = NumTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "I" => Ok(NumType::Integer),
            "F" => Ok(NumType::Single),
            "D" => Ok(NumType::Double),
            _ => Err(NumTypeError),
        }
    }
}

impl fmt::Display for NumType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            NumType::Integer => write!(f, "I"),
            NumType::Single => write!(f, "F"),
            NumType::Double => write!(f, "D"),
        }
    }
}

struct NumTypeError;

impl fmt::Display for NumTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be one of 'F', 'D', or 'A'")
    }
}

impl From<NumType> for AlphaNumType {
    fn from(value: NumType) -> Self {
        match value {
            NumType::Integer => AlphaNumType::Integer,
            NumType::Single => AlphaNumType::Single,
            NumType::Double => AlphaNumType::Double,
        }
    }
}

/// A compensation matrix.
///
/// This is held in the $DFCmTOn keywords in 2.0 and $COMP in 3.0.
#[derive(Debug, Clone, Serialize)]
struct Compensation {
    /// Values in the comp matrix in row-major order. Assumed to be the
    /// same width and height as $PAR
    matrix: Vec<Vec<f32>>,
}

impl FromStr for Compensation {
    type Err = FixedSeqError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut xs = s.split(",");
        if let Some(first) = &xs.next().and_then(|x| x.parse::<usize>().ok()) {
            let n = *first;
            let nn = n * n;
            let values: Vec<_> = xs.by_ref().take(nn).collect();
            let remainder = xs.by_ref().count();
            let total = values.len() + remainder;
            if total != nn {
                Err(FixedSeqError::WrongLength {
                    expected: nn,
                    total,
                })
            } else {
                let fvalues: Vec<_> = values
                    .into_iter()
                    .filter_map(|x| x.parse::<f32>().ok())
                    .collect();
                if fvalues.len() != nn {
                    Err(FixedSeqError::BadFloat)
                } else {
                    let matrix = fvalues
                        .into_iter()
                        .chunks(n)
                        .into_iter()
                        .map(|c| c.collect())
                        .collect();
                    Ok(Compensation { matrix })
                }
            }
        } else {
            Err(FixedSeqError::BadLength)
        }
    }
}

impl fmt::Display for Compensation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let n = self.matrix.len();
        let xs = self.matrix.iter().map(|xs| xs.iter().join(",")).join(",");
        write!(f, "{n},{xs}")
    }
}

enum FixedSeqError {
    WrongLength { total: usize, expected: usize },
    BadLength,
    BadFloat,
}

impl fmt::Display for FixedSeqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            FixedSeqError::BadFloat => write!(f, "Float could not be parsed"),
            FixedSeqError::WrongLength { total, expected } => {
                write!(f, "Expected {expected} entries, found {total}")
            }
            FixedSeqError::BadLength => write!(f, "Could not determine length"),
        }
    }
}

/// The spillover matrix in the $SPILLOVER keyword in (3.1+)
#[derive(Debug, Clone, Serialize)]
struct Spillover {
    measurements: Vec<String>,
    /// Values in the spillover matrix in row-major order.
    matrix: Vec<Vec<f32>>,
}

impl FromStr for Spillover {
    type Err = NamedFixedSeqError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        {
            let mut xs = s.split(",");
            if let Some(first) = &xs.next().and_then(|x| x.parse::<usize>().ok()) {
                let n = *first;
                let nn = n * n;
                let expected = n + nn;
                let measurements: Vec<_> = xs.by_ref().take(n).map(String::from).collect();
                let values: Vec<_> = xs.by_ref().take(nn).collect();
                let remainder = xs.by_ref().count();
                let total = measurements.len() + values.len() + remainder;
                if total != expected {
                    Err(NamedFixedSeqError::Seq(FixedSeqError::WrongLength {
                        total,
                        expected,
                    }))
                } else if measurements.iter().unique().count() != n {
                    Err(NamedFixedSeqError::NonUnique)
                } else {
                    let fvalues: Vec<_> = values
                        .into_iter()
                        .filter_map(|x| x.parse::<f32>().ok())
                        .collect();
                    if fvalues.len() != nn {
                        Err(NamedFixedSeqError::Seq(FixedSeqError::BadFloat))
                    } else {
                        let matrix = fvalues
                            .into_iter()
                            .chunks(n)
                            .into_iter()
                            .map(|c| c.collect())
                            .collect();
                        Ok(Spillover {
                            measurements,
                            matrix,
                        })
                    }
                }
            } else {
                Err(NamedFixedSeqError::Seq(FixedSeqError::BadLength))
            }
        }
    }
}

impl fmt::Display for Spillover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let n = self.measurements.len();
        let xs = self.matrix.iter().map(|ys| ys.iter().join(",")).join(",");
        write!(f, "{n},{xs}")
    }
}

enum NamedFixedSeqError {
    Seq(FixedSeqError),
    NonUnique,
}

impl fmt::Display for NamedFixedSeqError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            NamedFixedSeqError::Seq(s) => write!(f, "{}", s),
            NamedFixedSeqError::NonUnique => write!(f, "Names in sequence is not unique"),
        }
    }
}

impl Spillover {
    fn table(&self, delim: &str) -> Vec<String> {
        let header0 = vec![String::from("[-]")];
        let header = header0
            .into_iter()
            .chain(self.measurements.iter().map(String::from))
            .join(delim);
        let lines = vec![header];
        let rows = self.matrix.iter().map(|xs| xs.iter().join(delim));
        lines.into_iter().chain(rows).collect()
    }

    fn print_table(&self, delim: &str) {
        for e in self.table(delim) {
            println!("{}", e);
        }
    }
}

/// The byte order as shown in the $BYTEORD field in 2.0 and 3.0
///
/// This can be either 1,2,3,4 (little endian), 4,3,2,1 (big endian), or some
/// sequence representing byte order. For 2.0 and 3.0, this sequence is
/// technically allowed to vary in length in the case of $DATATYPE=I since
/// integers do not necessarily need to be 32 or 64-bit.
#[derive(Debug, Clone, Serialize)]
enum ByteOrd {
    Endian(Endian),
    Mixed(Vec<u8>),
}

pub struct EndianError;

impl fmt::Display for EndianError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "Endian must be either 1,2,3,4 or 4,3,2,1")
    }
}

impl FromStr for Endian {
    type Err = EndianError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "1,2,3,4" => Ok(Endian::Little),
            "4,3,2,1" => Ok(Endian::Big),
            _ => Err(EndianError),
        }
    }
}

impl fmt::Display for Endian {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let x = match self {
            Endian::Big => "4,3,2,1",
            Endian::Little => "1,2,3,4",
        };
        write!(f, "{x}")
    }
}

enum ParseByteOrdError {
    InvalidOrder,
    InvalidNumbers,
}

impl fmt::Display for ParseByteOrdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ParseByteOrdError::InvalidNumbers => write!(f, "Could not parse numbers in byte order"),
            ParseByteOrdError::InvalidOrder => write!(f, "Byte order must include 1-n uniquely"),
        }
    }
}

impl FromStr for ByteOrd {
    type Err = ParseByteOrdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse() {
            Ok(e) => Ok(ByteOrd::Endian(e)),
            _ => {
                let xs: Vec<_> = s.split(",").collect();
                let nxs = xs.len();
                let xs_num: Vec<u8> = xs.iter().filter_map(|s| s.parse().ok()).unique().collect();
                if let (Some(min), Some(max)) = (xs_num.iter().min(), xs_num.iter().max()) {
                    if *min == 1 && usize::from(*max) == nxs && xs_num.len() == nxs {
                        Ok(ByteOrd::Mixed(xs_num.iter().map(|x| x - 1).collect()))
                    } else {
                        Err(ParseByteOrdError::InvalidOrder)
                    }
                } else {
                    Err(ParseByteOrdError::InvalidNumbers)
                }
            }
        }
    }
}

impl fmt::Display for ByteOrd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ByteOrd::Endian(e) => write!(f, "{}", e),
            ByteOrd::Mixed(xs) => write!(f, "{}", xs.iter().join(",")),
        }
    }
}

impl ByteOrd {
    // This only makes sense for pre 3.1 integer types
    fn num_bytes(&self) -> u8 {
        match self {
            ByteOrd::Endian(_) => 4,
            ByteOrd::Mixed(xs) => xs.len() as u8,
        }
    }
}

/// The $TR field in all FCS versions.
///
/// This is formatted as 'string,f' where 'string' is a measurement name.
#[derive(Debug, Clone, Serialize)]
struct Trigger {
    measurement: String,
    threshold: u32,
}

impl FromStr for Trigger {
    type Err = TriggerError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split(",").collect::<Vec<_>>()[..] {
            [p, n1] => n1
                .parse()
                .map_err(TriggerError::IntFormat)
                .map(|threshold| Trigger {
                    measurement: String::from(p),
                    threshold,
                }),
            _ => Err(TriggerError::WrongFieldNumber),
        }
    }
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{},{}", self.measurement, self.threshold)
    }
}

enum TriggerError {
    WrongFieldNumber,
    IntFormat(std::num::ParseIntError),
}

impl fmt::Display for TriggerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            TriggerError::WrongFieldNumber => write!(f, "must be like 'string,f'"),
            TriggerError::IntFormat(i) => write!(f, "{}", i),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ModifiedDateTime(NaiveDateTime);

impl FromStr for ModifiedDateTime {
    type Err = ModifiedDateTimeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (dt, cc) = NaiveDateTime::parse_and_remainder(s, "%d-%b-%Y %H:%M:%S")
            .or(Err(ModifiedDateTimeError))?;
        if cc.is_empty() {
            Ok(ModifiedDateTime(dt))
        } else if cc.len() == 3 && cc.starts_with(".") {
            let tt: u32 = cc[1..3].parse().or(Err(ModifiedDateTimeError))?;
            dt.with_nanosecond(tt * 10000000)
                .map(ModifiedDateTime)
                .ok_or(ModifiedDateTimeError)
        } else {
            Err(ModifiedDateTimeError)
        }
    }
}

impl fmt::Display for ModifiedDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let dt = self.0.format("%d-%b-%Y %H:%M:%S");
        let cc = self.0.nanosecond() / 10000000;
        write!(f, "{dt}.{cc}")
    }
}

struct ModifiedDateTimeError;

impl fmt::Display for ModifiedDateTimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'dd-mmm-yyyy hh:mm:ss[.cc]'")
    }
}

#[derive(Debug, Clone, Serialize)]
struct FCSDate(NaiveDate);

// the "%b" format is case-insensitive so this should work for "Jan", "JAN",
// "jan", "jaN", etc
const FCS_DATE_FORMAT: &str = "%d-%b-%Y";

impl FromStr for FCSDate {
    type Err = FCSDateError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        NaiveDate::parse_from_str(s, FCS_DATE_FORMAT)
            .or(Err(FCSDateError))
            .map(FCSDate)
    }
}

impl fmt::Display for FCSDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0.format(FCS_DATE_FORMAT))
    }
}

struct FCSDateError;

impl fmt::Display for FCSDateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'dd-mmm-yyyy'")
    }
}

#[derive(Debug, Clone, Serialize)]
struct Timestamps<T> {
    btim: OptionalKw<T>,
    etim: OptionalKw<T>,
    date: OptionalKw<FCSDate>,
}

type Timestamps2_0 = Timestamps<FCSTime>;
type Timestamps3_0 = Timestamps<FCSTime60>;
type Timestamps3_1 = Timestamps<FCSTime100>;

#[derive(Debug, Clone, Serialize)]
struct Datetimes {
    begin: OptionalKw<FCSDateTime>,
    end: OptionalKw<FCSDateTime>,
}

// TODO this is super messy, see 3.2 spec for restrictions on this we may with
// to use further
#[derive(Debug, Clone, PartialEq, Serialize)]
enum Scale {
    Log { decades: f32, offset: f32 },
    Linear,
}

use Scale::*;

impl fmt::Display for Scale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Scale::Log { decades, offset } => write!(f, "{decades},{offset}"),
            Scale::Linear => write!(f, "Lin"),
        }
    }
}

enum ScaleError {
    FloatError(ParseFloatError),
    WrongFormat,
}

impl fmt::Display for ScaleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            ScaleError::FloatError(x) => write!(f, "{}", x),
            ScaleError::WrongFormat => write!(f, "must be like 'f1,f2'"),
        }
    }
}

impl str::FromStr for Scale {
    type Err = ScaleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split(",").collect::<Vec<_>>()[..] {
            [ds, os] => {
                let f1 = ds.parse().map_err(ScaleError::FloatError)?;
                let f2 = os.parse().map_err(ScaleError::FloatError)?;
                match (f1, f2) {
                    (0.0, 0.0) => Ok(Linear),
                    (decades, offset) => Ok(Log { decades, offset }),
                }
            }
            _ => Err(ScaleError::WrongFormat),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
enum Display {
    Lin { lower: f32, upper: f32 },
    Log { offset: f32, decades: f32 },
}

enum DisplayError {
    FloatError(ParseFloatError),
    InvalidType,
    FormatError,
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            DisplayError::FloatError(x) => write!(f, "{}", x),
            DisplayError::InvalidType => write!(f, "Type must be either 'Logarithmic' or 'Linear'"),
            DisplayError::FormatError => write!(f, "must be like 'string,f1,f2'"),
        }
    }
}

impl str::FromStr for Display {
    type Err = DisplayError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split(",").collect::<Vec<_>>()[..] {
            [which, s1, s2] => {
                let f1 = s1.parse().map_err(DisplayError::FloatError)?;
                let f2 = s2.parse().map_err(DisplayError::FloatError)?;
                match which {
                    "Linear" => Ok(Display::Lin {
                        lower: f1,
                        upper: f2,
                    }),
                    "Logarithmic" => Ok(Display::Log {
                        decades: f1,
                        offset: f2,
                    }),
                    _ => Err(DisplayError::InvalidType),
                }
            }
            _ => Err(DisplayError::FormatError),
        }
    }
}

impl fmt::Display for Display {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Display::Lin { lower, upper } => write!(f, "Linear,{lower},{upper}"),
            Display::Log { offset, decades } => write!(f, "Log,{offset},{decades}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Calibration3_1 {
    value: f32,
    unit: String,
}

enum CalibrationError<C> {
    Float(ParseFloatError),
    Range,
    Format(C),
}

struct CalibrationFormat3_1;
struct CalibrationFormat3_2;

impl fmt::Display for CalibrationFormat3_1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'f,string'")
    }
}

impl fmt::Display for CalibrationFormat3_2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be like 'f1,[f2],string'")
    }
}

impl<C: fmt::Display> fmt::Display for CalibrationError<C> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            CalibrationError::Float(x) => write!(f, "{}", x),
            CalibrationError::Range => write!(f, "must be a positive float"),
            CalibrationError::Format(x) => write!(f, "{}", x),
        }
    }
}

impl str::FromStr for Calibration3_1 {
    type Err = CalibrationError<CalibrationFormat3_1>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.split(",").collect::<Vec<_>>()[..] {
            [svalue, unit] => {
                let value = svalue.parse().map_err(CalibrationError::Float)?;
                if value >= 0.0 {
                    Ok(Calibration3_1 {
                        value,
                        unit: String::from(unit),
                    })
                } else {
                    Err(CalibrationError::Range)
                }
            }
            _ => Err(CalibrationError::Format(CalibrationFormat3_1)),
        }
    }
}

impl fmt::Display for Calibration3_1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{},{}", self.value, self.unit)
    }
}

#[derive(Debug, Clone, Serialize)]
struct Calibration3_2 {
    value: f32,
    offset: f32,
    unit: String,
}

impl str::FromStr for Calibration3_2 {
    type Err = CalibrationError<CalibrationFormat3_2>;

    // TODO not dry
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (value, offset, unit) = match s.split(",").collect::<Vec<_>>()[..] {
            [svalue, unit] => {
                let f1 = svalue.parse().map_err(CalibrationError::Float)?;
                Ok((f1, 0.0, String::from(unit)))
            }
            [svalue, soffset, unit] => {
                let f1 = svalue.parse().map_err(CalibrationError::Float)?;
                let f2 = soffset.parse().map_err(CalibrationError::Float)?;
                Ok((f1, f2, String::from(unit)))
            }
            _ => Err(CalibrationError::Format(CalibrationFormat3_2)),
        }?;
        if value >= 0.0 {
            Ok(Calibration3_2 {
                value,
                offset,
                unit,
            })
        } else {
            Err(CalibrationError::Range)
        }
    }
}

impl fmt::Display for Calibration3_2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{},{},{}", self.value, self.offset, self.unit)
    }
}

#[derive(Debug, Clone, Serialize)]
enum MeasurementType {
    ForwardScatter,
    SideScatter,
    RawFluorescence,
    UnmixedFluorescence,
    Mass,
    Time,
    ElectronicVolume,
    Classification,
    Index,
    Other(String),
}

impl str::FromStr for MeasurementType {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Forward Scatter" => Ok(MeasurementType::ForwardScatter),
            "Side Scatter" => Ok(MeasurementType::SideScatter),
            "Raw Fluorescence" => Ok(MeasurementType::RawFluorescence),
            "Unmixed Fluorescence" => Ok(MeasurementType::UnmixedFluorescence),
            "Mass" => Ok(MeasurementType::Mass),
            "Time" => Ok(MeasurementType::Time),
            "Electronic Volume" => Ok(MeasurementType::ElectronicVolume),
            "Index" => Ok(MeasurementType::Index),
            "Classification" => Ok(MeasurementType::Classification),
            s => Ok(MeasurementType::Other(String::from(s))),
        }
    }
}

impl fmt::Display for MeasurementType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            MeasurementType::ForwardScatter => write!(f, "Foward Scatter"),
            MeasurementType::SideScatter => write!(f, "Side Scatter"),
            MeasurementType::RawFluorescence => write!(f, "Raw Fluorescence"),
            MeasurementType::UnmixedFluorescence => write!(f, "Unmixed Fluorescence"),
            MeasurementType::Mass => write!(f, "Mass"),
            MeasurementType::Time => write!(f, "Time"),
            MeasurementType::ElectronicVolume => write!(f, "Electronic Volume"),
            MeasurementType::Classification => write!(f, "Classification"),
            MeasurementType::Index => write!(f, "Index"),
            MeasurementType::Other(s) => write!(f, "{}", s),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
enum Feature {
    Area,
    Width,
    Height,
}

struct FeatureError;

impl fmt::Display for FeatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be one of 'Area', 'Width', or 'Height'")
    }
}

impl str::FromStr for Feature {
    type Err = FeatureError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Area" => Ok(Feature::Area),
            "Width" => Ok(Feature::Width),
            "Height" => Ok(Feature::Height),
            _ => Err(FeatureError),
        }
    }
}

impl fmt::Display for Feature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Feature::Area => write!(f, "Area"),
            Feature::Width => write!(f, "Width"),
            Feature::Height => write!(f, "Height"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum OptionalKw<V> {
    Present(V),
    Absent,
}

use OptionalKw::*;

impl<V> OptionalKw<V> {
    fn as_ref(&self) -> OptionalKw<&V> {
        match self {
            OptionalKw::Present(x) => Present(x),
            Absent => Absent,
        }
    }

    fn into_option(self) -> Option<V> {
        match self {
            OptionalKw::Present(x) => Some(x),
            Absent => None,
        }
    }

    fn from_option(x: Option<V>) -> Self {
        x.map_or_else(|| Absent, |y| OptionalKw::Present(y))
    }
}

impl<V: fmt::Display> OptionalKw<V> {
    fn as_opt_string(&self) -> Option<String> {
        self.as_ref().into_option().map(|x| x.to_string())
    }
}

impl<T: Serialize> Serialize for OptionalKw<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self.as_ref() {
            Present(x) => serializer.serialize_some(x),
            Absent => serializer.serialize_none(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Wavelengths(Vec<u32>);

impl fmt::Display for Wavelengths {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0.iter().join(","))
    }
}

impl str::FromStr for Wavelengths {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut ws = vec![];
        for x in s.split(",") {
            ws.push(x.parse()?);
        }
        Ok(Wavelengths(ws))
    }
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq)]
struct Shortname(String);

struct ShortnameError;

impl fmt::Display for ShortnameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "commas are not allowed")
    }
}

impl fmt::Display for Shortname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

impl str::FromStr for Shortname {
    type Err = ShortnameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains(',') {
            Err(ShortnameError)
        } else {
            Ok(Shortname(String::from(s)))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct InnerMeasurement2_0 {
    scale: OptionalKw<Scale>,         // PnE
    wavelength: OptionalKw<u32>,      // PnL
    shortname: OptionalKw<Shortname>, // PnN
}

#[derive(Debug, Clone, Serialize)]
struct InnerMeasurement3_0 {
    scale: Scale,                     // PnE
    wavelength: OptionalKw<u32>,      // PnL
    shortname: OptionalKw<Shortname>, // PnN
    gain: OptionalKw<f32>,            // PnG
}

#[derive(Debug, Clone, Serialize)]
struct InnerMeasurement3_1 {
    scale: Scale,                         // PnE
    wavelengths: OptionalKw<Wavelengths>, // PnL
    shortname: Shortname,                 // PnN
    gain: OptionalKw<f32>,                // PnG
    calibration: OptionalKw<Calibration3_1>,
    display: OptionalKw<Display>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerMeasurement3_2 {
    scale: Scale,                         // PnE
    wavelengths: OptionalKw<Wavelengths>, // PnL
    shortname: Shortname,                 // PnN
    gain: OptionalKw<f32>,                // PnG
    calibration: OptionalKw<Calibration3_2>,
    display: OptionalKw<Display>,
    analyte: OptionalKw<String>,
    feature: OptionalKw<Feature>,
    measurement_type: OptionalKw<MeasurementType>,
    tag: OptionalKw<String>,
    detector_name: OptionalKw<String>,
    datatype: OptionalKw<NumType>,
}

// TODO this will likely need to be a trait in 4.0
impl InnerMeasurement3_2 {
    fn get_column_type(&self, default: AlphaNumType) -> AlphaNumType {
        self.datatype
            .as_ref()
            .into_option()
            .copied()
            .map(AlphaNumType::from)
            .unwrap_or(default)
    }
}

#[derive(Debug, Clone, Serialize)]
enum Bytes {
    Fixed(u8),
    Variable,
}

enum BytesError {
    Int(ParseIntError),
    Range,
    NotOctet,
}

impl fmt::Display for BytesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            BytesError::Int(i) => write!(f, "{}", i),
            BytesError::Range => write!(f, "bit widths over 64 are not supported"),
            BytesError::NotOctet => write!(f, "bit widths must be octets"),
        }
    }
}

impl FromStr for Bytes {
    type Err = BytesError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "*" => Ok(Bytes::Variable),
            _ => s.parse::<u8>().map_err(BytesError::Int).and_then(|x| {
                if x > 64 {
                    Err(BytesError::Range)
                } else if x % 8 > 1 {
                    Err(BytesError::NotOctet)
                } else {
                    Ok(Bytes::Fixed(x / 8))
                }
            }),
        }
    }
}

impl fmt::Display for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Bytes::Fixed(x) => write!(f, "{}", x),
            Bytes::Variable => write!(f, "*"),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
enum Range {
    // This will actually store PnR - 1; most cytometers will store this as a
    // power of 2, so in the case of a 64 bit channel this will be 2^64 which is
    // one greater than u64::MAX.
    Int(u64),
    // This stores the value of PnR as-is. Sometimes PnR is actually a float
    // for floating point measurements rather than an int.
    Float(f64),
}

enum RangeError {
    Int(ParseIntError),
    Float(ParseFloatError),
}

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            RangeError::Int(x) => write!(f, "{x}"),
            RangeError::Float(x) => write!(f, "{x}"),
        }
    }
}

impl str::FromStr for Range {
    type Err = RangeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.parse::<u64>() {
            Ok(x) => Ok(Range::Int(x - 1)),
            Err(e) => match e.kind() {
                IntErrorKind::InvalidDigit => s
                    .parse::<f64>()
                    .map_or_else(|e| Err(RangeError::Float(e)), |x| Ok(Range::Float(x))),
                IntErrorKind::PosOverflow => Ok(Range::Int(u64::MAX)),
                _ => Err(RangeError::Int(e)),
            },
        }
    }
}

impl fmt::Display for Range {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            Range::Int(x) => write!(f, "{x}"),
            Range::Float(x) => write!(f, "{x}"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct Measurement<X> {
    bytes: Bytes,                      // PnB
    range: Range,                      // PnR
    longname: OptionalKw<String>,      // PnS
    filter: OptionalKw<String>,        // PnF there is a loose standard for this
    power: OptionalKw<u32>,            // PnO
    detector_type: OptionalKw<String>, // PnD
    percent_emitted: OptionalKw<u32>,  // PnP (TODO deprecated in 3.2, factor out)
    detector_voltage: OptionalKw<f32>, // PnV
    nonstandard: HashMap<NonStdKey, String>,
    specific: X,
}

impl<P: VersionedMeasurement> Measurement<P> {
    fn bytes_eq(&self, b: u8) -> bool {
        match self.bytes {
            Bytes::Fixed(x) => x == b,
            _ => false,
        }
    }

    // TODO add measurement index to error message
    fn make_int_parser(&self, o: &ByteOrd, t: usize) -> Result<AnyIntColumn, Vec<String>> {
        match self.bytes {
            Bytes::Fixed(b) => make_int_parser(b, &self.range, o, t),
            _ => Err(vec![String::from("PnB is variable length")]),
        }
    }

    // TODO include nonstandard?
    fn keywords(&self, n: &str) -> Vec<(String, Option<String>)> {
        P::keywords(self, n)
    }

    fn table_header(&self) -> Vec<String> {
        vec![String::from("index")]
            .into_iter()
            .chain(P::keywords(self, "n").into_iter().map(|(k, _)| k))
            .collect()
    }

    fn table_row(&self, n: usize) -> Vec<Option<String>> {
        vec![Some(n.to_string())]
            .into_iter()
            // NOTE; the "n" is a dummy and never used
            .chain(P::keywords(self, "n").into_iter().map(|(_, v)| v))
            .collect()
    }
}

fn make_int_parser(b: u8, r: &Range, o: &ByteOrd, t: usize) -> Result<AnyIntColumn, Vec<String>> {
    match b {
        1 => u8::to_col_parser(r, o, t).map(AnyIntColumn::Uint8),
        2 => u16::to_col_parser(r, o, t).map(AnyIntColumn::Uint16),
        3 => IntFromBytes::<4, 3>::to_col_parser(r, o, t).map(AnyIntColumn::Uint24),
        4 => IntFromBytes::<4, 4>::to_col_parser(r, o, t).map(AnyIntColumn::Uint32),
        5 => IntFromBytes::<8, 5>::to_col_parser(r, o, t).map(AnyIntColumn::Uint40),
        6 => IntFromBytes::<8, 6>::to_col_parser(r, o, t).map(AnyIntColumn::Uint48),
        7 => IntFromBytes::<8, 7>::to_col_parser(r, o, t).map(AnyIntColumn::Uint56),
        8 => IntFromBytes::<8, 8>::to_col_parser(r, o, t).map(AnyIntColumn::Uint64),
        _ => Err(vec![String::from("$PnB has invalid byte length")]),
    }
}

trait Versioned {
    fn fcs_version() -> Version;
}

trait VersionedMeasurement: Sized + Versioned {
    fn lookup_specific(st: &mut KwState, n: usize) -> Option<Self>;

    fn measurement_name(p: &Measurement<Self>) -> Option<&str>;

    fn lookup_measurements(st: &mut KwState, par: usize) -> Option<Vec<Measurement<Self>>> {
        let mut ps = vec![];
        let v = Self::fcs_version();
        for n in 1..(par + 1) {
            let maybe_bytes = st.lookup_meas_bytes(n);
            let maybe_range = st.lookup_meas_range(n);
            let maybe_specific = Self::lookup_specific(st, n);
            if let (Some(bytes), Some(range), Some(specific)) =
                (maybe_bytes, maybe_range, maybe_specific)
            {
                let p = Measurement {
                    bytes,
                    range,
                    longname: st.lookup_meas_longname(n),
                    filter: st.lookup_meas_filter(n),
                    power: st.lookup_meas_power(n),
                    detector_type: st.lookup_meas_detector_type(n),
                    percent_emitted: st.lookup_meas_percent_emitted(n, v == Version::FCS3_2),
                    detector_voltage: st.lookup_meas_detector_voltage(n),
                    specific,
                    nonstandard: st.lookup_meas_nonstandard(n),
                };
                ps.push(p);
            }
        }
        let names: Vec<&str> = ps
            .iter()
            .filter_map(|m| Self::measurement_name(m))
            .collect();
        if let Some(time_name) = &st.conf.time_shortname {
            if !names.iter().copied().contains(time_name.as_str()) {
                st.push_meta_error_or_warning(
                    st.conf.ensure_time,
                    format!("Channel called '{time_name}' not found for time"),
                );
            }
        }
        if names.iter().unique().count() < names.len() {
            st.push_meta_error_str("$PnN are not all unique");
            None
        } else {
            Some(ps)
        }
    }

    fn suffixes_inner(&self) -> Vec<(&'static str, Option<String>)>;

    fn keywords(m: &Measurement<Self>, n: &str) -> Vec<(String, Option<String>)> {
        let fixed = [
            (BYTES_SFX, Some(m.bytes.to_string())),
            (RANGE_SFX, Some(m.range.to_string())),
            (LONGNAME_SFX, m.longname.as_opt_string()),
            (FILTER_SFX, m.filter.as_opt_string()),
            (POWER_SFX, m.power.as_opt_string()),
            (DET_TYPE_SFX, m.detector_type.as_opt_string()),
            (PCNT_EMT_SFX, m.percent_emitted.as_opt_string()),
            (DET_VOLT_SFX, m.detector_voltage.as_opt_string()),
        ];
        fixed
            .into_iter()
            .chain(m.specific.suffixes_inner())
            .map(|(s, v)| (format_measurement(n, s), v))
            .collect()
    }
}

type Measurement2_0 = Measurement<InnerMeasurement2_0>;
type Measurement3_0 = Measurement<InnerMeasurement3_0>;
type Measurement3_1 = Measurement<InnerMeasurement3_1>;
type Measurement3_2 = Measurement<InnerMeasurement3_2>;

impl Versioned for InnerMeasurement2_0 {
    fn fcs_version() -> Version {
        Version::FCS2_0
    }
}

impl Versioned for InnerMeasurement3_0 {
    fn fcs_version() -> Version {
        Version::FCS3_0
    }
}

impl Versioned for InnerMeasurement3_1 {
    fn fcs_version() -> Version {
        Version::FCS3_1
    }
}

impl Versioned for InnerMeasurement3_2 {
    fn fcs_version() -> Version {
        Version::FCS3_2
    }
}

impl VersionedMeasurement for InnerMeasurement2_0 {
    fn measurement_name(p: &Measurement<Self>) -> Option<&str> {
        p.specific
            .shortname
            .as_ref()
            .into_option()
            .map(|s| s.0.as_str())
    }

    fn lookup_specific(st: &mut KwState, n: usize) -> Option<InnerMeasurement2_0> {
        Some(InnerMeasurement2_0 {
            scale: st.lookup_meas_scale_opt(n),
            shortname: st.lookup_meas_shortname_opt(n),
            wavelength: st.lookup_meas_wavelength(n),
        })
    }

    fn suffixes_inner(&self) -> Vec<(&'static str, Option<String>)> {
        [
            (SCALE_SFX, self.scale.as_opt_string()),
            (SHORTNAME_SFX, self.shortname.as_opt_string()),
            (WAVELEN_SFX, self.wavelength.as_opt_string()),
        ]
        .into_iter()
        .collect()
    }
}

impl VersionedMeasurement for InnerMeasurement3_0 {
    fn measurement_name(p: &Measurement<Self>) -> Option<&str> {
        p.specific
            .shortname
            .as_ref()
            .into_option()
            .map(|s| s.0.as_str())
    }

    fn lookup_specific(st: &mut KwState, n: usize) -> Option<InnerMeasurement3_0> {
        let shortname = st.lookup_meas_shortname_opt(n);
        Some(InnerMeasurement3_0 {
            scale: st.lookup_meas_scale_timecheck_opt(n, &shortname)?,
            gain: st.lookup_meas_gain_timecheck_opt(n, &shortname),
            shortname,
            wavelength: st.lookup_meas_wavelength(n),
        })
    }

    fn suffixes_inner(&self) -> Vec<(&'static str, Option<String>)> {
        [
            (SCALE_SFX, Some(self.scale.to_string())),
            (SHORTNAME_SFX, self.shortname.as_opt_string()),
            (WAVELEN_SFX, self.wavelength.as_opt_string()),
            (GAIN_SFX, self.gain.as_opt_string()),
        ]
        .into_iter()
        .collect()
    }
}

impl VersionedMeasurement for InnerMeasurement3_1 {
    fn measurement_name(p: &Measurement<Self>) -> Option<&str> {
        Some(p.specific.shortname.0.as_str())
    }

    fn lookup_specific(st: &mut KwState, n: usize) -> Option<InnerMeasurement3_1> {
        let shortname = st.lookup_meas_shortname_req(n)?;
        Some(InnerMeasurement3_1 {
            scale: st.lookup_meas_scale_timecheck(n, &shortname)?,
            gain: st.lookup_meas_gain_timecheck(n, &shortname),
            shortname,
            wavelengths: st.lookup_meas_wavelengths(n),
            calibration: st.lookup_meas_calibration3_1(n),
            display: st.lookup_meas_display(n),
        })
    }

    fn suffixes_inner(&self) -> Vec<(&'static str, Option<String>)> {
        [
            (SCALE_SFX, Some(self.scale.to_string())),
            (SHORTNAME_SFX, Some(self.shortname.to_string())),
            (WAVELEN_SFX, self.wavelengths.as_opt_string()),
            (GAIN_SFX, self.gain.as_opt_string()),
            (CALIBRATION_SFX, self.calibration.as_opt_string()),
            (DISPLAY_SFX, self.display.as_opt_string()),
        ]
        .into_iter()
        .collect()
    }
}

impl VersionedMeasurement for InnerMeasurement3_2 {
    fn measurement_name(p: &Measurement<Self>) -> Option<&str> {
        Some(p.specific.shortname.0.as_str())
    }

    fn lookup_specific(st: &mut KwState, n: usize) -> Option<InnerMeasurement3_2> {
        let shortname = st.lookup_meas_shortname_req(n)?;
        Some(InnerMeasurement3_2 {
            gain: st.lookup_meas_gain_timecheck(n, &shortname),
            scale: st.lookup_meas_scale_timecheck(n, &shortname)?,
            shortname,
            wavelengths: st.lookup_meas_wavelengths(n),
            calibration: st.lookup_meas_calibration3_2(n),
            display: st.lookup_meas_display(n),
            detector_name: st.lookup_meas_detector(n),
            tag: st.lookup_meas_tag(n),
            // TODO this should be "Time" if time channel
            measurement_type: st.lookup_meas_type(n),
            feature: st.lookup_meas_feature(n),
            analyte: st.lookup_meas_analyte(n),
            datatype: st.lookup_meas_datatype(n),
        })
    }

    fn suffixes_inner(&self) -> Vec<(&'static str, Option<String>)> {
        [
            (SCALE_SFX, Some(self.scale.to_string())),
            (SHORTNAME_SFX, Some(self.shortname.to_string())),
            (WAVELEN_SFX, self.wavelengths.as_opt_string()),
            (GAIN_SFX, self.gain.as_opt_string()),
            (CALIBRATION_SFX, self.calibration.as_opt_string()),
            (DISPLAY_SFX, self.display.as_opt_string()),
            (DET_NAME_SFX, self.detector_name.as_opt_string()),
            (TAG_SFX, self.tag.as_opt_string()),
            (MEAS_TYPE_SFX, self.measurement_type.as_opt_string()),
            (FEATURE_SFX, self.feature.as_opt_string()),
            (ANALYTE_SFX, self.analyte.as_opt_string()),
            (DATATYPE_SFX, self.datatype.as_opt_string()),
        ]
        .into_iter()
        .collect()
    }
}

#[derive(Debug, Clone, Serialize)]
enum Originality {
    Original,
    NonDataModified,
    Appended,
    DataModified,
}

struct OriginalityError;

impl fmt::Display for OriginalityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(
            f,
            "Originality must be one of 'Original', 'NonDataModified', \
                   'Appended', or 'DataModified'"
        )
    }
}

impl str::FromStr for Originality {
    type Err = OriginalityError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Original" => Ok(Originality::Original),
            "NonDataModified" => Ok(Originality::NonDataModified),
            "Appended" => Ok(Originality::Appended),
            "DataModified" => Ok(Originality::DataModified),
            _ => Err(OriginalityError),
        }
    }
}

impl fmt::Display for Originality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let x = match self {
            Originality::Appended => "Appended",
            Originality::Original => "Original",
            Originality::NonDataModified => "NonDataModified",
            Originality::DataModified => "DataModified",
        };
        write!(f, "{x}")
    }
}

#[derive(Debug, Clone, Serialize)]
struct ModificationData {
    last_modifier: OptionalKw<String>,
    last_modified: OptionalKw<ModifiedDateTime>,
    originality: OptionalKw<Originality>,
}

#[derive(Debug, Clone, Serialize)]
struct PlateData {
    plateid: OptionalKw<String>,
    platename: OptionalKw<String>,
    wellid: OptionalKw<String>,
}

#[derive(Debug, Clone, Serialize)]
struct UnstainedCenters(HashMap<String, f32>);

impl FromStr for UnstainedCenters {
    type Err = NamedFixedSeqError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut xs = s.split(",");
        if let Some(n) = xs.next().and_then(|s| s.parse().ok()) {
            let measurements: Vec<_> = xs.by_ref().take(n).map(String::from).collect();
            let values: Vec<_> = xs.by_ref().take(n).collect();
            let remainder = xs.by_ref().count();
            let total = values.len() + measurements.len() + remainder;
            let expected = 2 * n;
            if total != expected {
                let fvalues: Vec<_> = values
                    .into_iter()
                    .filter_map(|s| s.parse::<f32>().ok())
                    .collect();
                if fvalues.len() != n {
                    Err(NamedFixedSeqError::Seq(FixedSeqError::BadFloat))
                } else if measurements.iter().unique().count() != n {
                    Err(NamedFixedSeqError::NonUnique)
                } else {
                    Ok(UnstainedCenters(
                        measurements.into_iter().zip(fvalues).collect(),
                    ))
                }
            } else {
                Err(NamedFixedSeqError::Seq(FixedSeqError::WrongLength {
                    total,
                    expected,
                }))
            }
        } else {
            Err(NamedFixedSeqError::Seq(FixedSeqError::BadLength))
        }
    }
}

impl fmt::Display for UnstainedCenters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let n = self.0.len();
        let (measurements, values): (Vec<_>, Vec<_>) =
            self.0.iter().map(|(k, v)| (k.clone(), *v)).unzip();
        write!(
            f,
            "{n},{},{}",
            measurements.join(","),
            values.iter().join(",")
        )
    }
}

#[derive(Debug, Clone, Serialize)]
struct UnstainedData {
    unstainedcenters: OptionalKw<UnstainedCenters>,
    unstainedinfo: OptionalKw<String>,
}

#[derive(Debug, Clone, Serialize)]
struct CarrierData {
    carrierid: OptionalKw<String>,
    carriertype: OptionalKw<String>,
    locationid: OptionalKw<String>,
}

#[derive(Debug, Clone, Serialize)]
struct Unicode {
    page: u32,
    kws: Vec<String>,
}

enum UnicodeError {
    Empty,
    BadFormat,
}

impl fmt::Display for UnicodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        match self {
            UnicodeError::Empty => write!(f, "No keywords given"),
            UnicodeError::BadFormat => write!(f, "Must be like 'n,string,[[string],...]'"),
        }
    }
}

impl FromStr for Unicode {
    type Err = UnicodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut xs = s.split(",");
        if let Some(page) = xs.next().and_then(|s| s.parse().ok()) {
            let kws: Vec<String> = xs.map(String::from).collect();
            if kws.is_empty() {
                Err(UnicodeError::Empty)
            } else {
                Ok(Unicode { page, kws })
            }
        } else {
            Err(UnicodeError::BadFormat)
        }
    }
}

impl fmt::Display for Unicode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{},{}", self.page, self.kws.iter().join(","))
    }
}

#[derive(Debug, Clone, Serialize)]
struct SupplementalOffsets3_0 {
    analysis: Segment,
    stext: Segment,
}

#[derive(Debug, Clone, Serialize)]
struct SupplementalOffsets3_2 {
    analysis: OptionalKw<Segment>,
    stext: OptionalKw<Segment>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerMetadata2_0 {
    // tot: OptionalKw<u32>,
    mode: Mode,
    byteord: ByteOrd,
    cyt: OptionalKw<String>,
    comp: OptionalKw<Compensation>,
    timestamps: Timestamps2_0, // BTIM/ETIM/DATE
}

#[derive(Debug, Clone, Serialize)]
struct InnerMetadata3_0 {
    // data: Offsets,
    // supplemental: SupplementalOffsets3_0,
    // tot: u32,
    mode: Mode,
    byteord: ByteOrd,
    timestamps: Timestamps3_0, // BTIM/ETIM/DATE
    cyt: OptionalKw<String>,
    comp: OptionalKw<Compensation>,
    cytsn: OptionalKw<String>,
    timestep: OptionalKw<f32>,
    unicode: OptionalKw<Unicode>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerMetadata3_1 {
    // data: Offsets,
    // supplemental: SupplementalOffsets3_0,
    // tot: u32,
    mode: Mode,
    byteord: Endian,
    timestamps: Timestamps3_1, // BTIM/ETIM/DATE
    cyt: OptionalKw<String>,
    spillover: OptionalKw<Spillover>,
    cytsn: OptionalKw<String>,
    timestep: OptionalKw<f32>,
    modification: ModificationData,
    plate: PlateData,
    vol: OptionalKw<f32>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerMetadata3_2 {
    // TODO offsets are not necessary for writing
    // data: Offsets,
    // supplemental: SupplementalOffsets3_2,
    // TODO this can be assumed from full dataframe when we have it
    // tot: u32,
    byteord: Endian,
    timestamps: Timestamps3_1, // BTIM/ETIM/DATE
    datetimes: Datetimes,      // DATETIMESTART/END
    cyt: String,
    spillover: OptionalKw<Spillover>,
    cytsn: OptionalKw<String>,
    timestep: OptionalKw<f32>,
    modification: ModificationData,
    plate: PlateData,
    vol: OptionalKw<f32>,
    carrier: CarrierData,
    unstained: UnstainedData,
    flowrate: OptionalKw<String>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerReadData2_0 {
    tot: OptionalKw<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct InnerReadData3_0 {
    data: Segment,
    supplemental: SupplementalOffsets3_0,
    tot: u32,
}

// struct InnerReadData3_1 {
//     data: Offsets,
//     supplemental: SupplementalOffsets3_0,
//     tot: u32,
// }

#[derive(Debug, Clone, Serialize)]
struct InnerReadData3_2 {
    data: Segment,
    supplemental: SupplementalOffsets3_2,
    tot: u32,
}

#[derive(Debug, Clone, Serialize)]
struct ReadData<X> {
    par: usize,
    nextdata: u32,
    specific: X,
}

trait VersionedReadData: Sized {
    fn lookup_inner(st: &mut KwState) -> Option<Self>;

    fn get_tot(&self) -> Option<u32>;

    fn data_offsets(&self, o: &Segment) -> Segment;

    // fn measurement_name(p: &Measurement<Self>) -> Option<&str>;

    // fn has_linear_scale(&self) -> bool;

    // fn has_gain(&self) -> bool;

    fn lookup(st: &mut KwState, par: usize) -> Option<ReadData<Self>> {
        let r = ReadData {
            par,
            nextdata: st.lookup_nextdata()?,
            specific: Self::lookup_inner(st)?,
        };
        Some(r)
    }
}

impl VersionedReadData for InnerReadData2_0 {
    fn lookup_inner(st: &mut KwState) -> Option<Self> {
        Some(InnerReadData2_0 {
            tot: st.lookup_tot_opt(),
        })
    }

    fn data_offsets(&self, o: &Segment) -> Segment {
        *o
    }

    fn get_tot(&self) -> Option<u32> {
        self.tot.as_ref().into_option().copied()
    }
}

impl VersionedReadData for InnerReadData3_0 {
    fn lookup_inner(st: &mut KwState) -> Option<Self> {
        Some(InnerReadData3_0 {
            data: st.lookup_data_offsets()?,
            supplemental: st.lookup_supplemental3_0()?,
            tot: st.lookup_tot_req()?,
        })
    }

    fn data_offsets(&self, o: &Segment) -> Segment {
        if o.is_unset() {
            self.data
        } else {
            *o
        }
    }

    fn get_tot(&self) -> Option<u32> {
        Some(self.tot)
    }
}

impl VersionedReadData for InnerReadData3_2 {
    fn lookup_inner(st: &mut KwState) -> Option<Self> {
        let r = InnerReadData3_2 {
            data: st.lookup_data_offsets()?,
            supplemental: st.lookup_supplemental3_2(),
            tot: st.lookup_tot_req()?,
        };
        Some(r)
    }

    fn data_offsets(&self, o: &Segment) -> Segment {
        if o.is_unset() {
            self.data
        } else {
            *o
        }
    }

    fn get_tot(&self) -> Option<u32> {
        Some(self.tot)
    }
}

#[derive(Debug, Clone, Serialize)]
struct Metadata<X> {
    // TODO par is redundant when we have a full dataframe
    // TODO nextdata is not relevant for writing
    // par: u32,
    // nextdata: u32,
    datatype: AlphaNumType,
    abrt: OptionalKw<u32>,
    com: OptionalKw<String>,
    cells: OptionalKw<String>,
    exp: OptionalKw<String>,
    fil: OptionalKw<String>,
    inst: OptionalKw<String>,
    lost: OptionalKw<u32>,
    op: OptionalKw<String>,
    proj: OptionalKw<String>,
    smno: OptionalKw<String>,
    src: OptionalKw<String>,
    sys: OptionalKw<String>,
    tr: OptionalKw<Trigger>,
    specific: X,
}

impl<M: VersionedMetadata> Metadata<M> {
    fn keywords(&self, par: usize, tot: usize, len: KwLengths) -> MaybeKeywords {
        M::keywords(self, par, tot, len)
    }
}

type Metadata2_0 = Metadata<InnerMetadata2_0>;
type Metadata3_0 = Metadata<InnerMetadata3_0>;
type Metadata3_1 = Metadata<InnerMetadata3_1>;
type Metadata3_2 = Metadata<InnerMetadata3_2>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum Mode {
    List,
    Uncorrelated,
    Correlated,
}

impl FromStr for Mode {
    type Err = ModeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "C" => Ok(Mode::Correlated),
            "L" => Ok(Mode::List),
            "U" => Ok(Mode::Uncorrelated),
            _ => Err(ModeError),
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let x = match self {
            Mode::Correlated => "C",
            Mode::List => "L",
            Mode::Uncorrelated => "U",
        };
        write!(f, "{}", x)
    }
}

struct ModeError;

impl fmt::Display for ModeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "must be one of 'C', 'L', or 'U'")
    }
}

struct Mode3_2;

impl FromStr for Mode3_2 {
    type Err = Mode3_2Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "L" => Ok(Mode3_2),
            _ => Err(Mode3_2Error),
        }
    }
}

impl fmt::Display for Mode3_2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "L")
    }
}

struct Mode3_2Error;

impl fmt::Display for Mode3_2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "can only be 'L'")
    }
}

/// Represents write-critical keywords in the TEXT segment.
///
/// This includes everything except offsets, $NEXTDATA, $PAR, and $TOT, since
/// these are not necessary for writing a new FCS file and will be calculated on
/// the fly.
#[derive(Debug, Clone, Serialize)]
struct CoreText<M, P> {
    metadata: Metadata<M>,
    measurements: Vec<Measurement<P>>,
    deviant_keywords: HashMap<StdKey, String>,
    nonstandard_keywords: HashMap<NonStdKey, String>,
}

#[derive(Debug, Clone, Serialize)]
struct StdText<M, P, R> {
    // TODO this isn't necessary for writing and is redundant here
    data_offsets: Segment,
    read_data: ReadData<R>,
    core: CoreText<M, P>,
}

#[derive(Debug)]
pub enum AnyStdTEXT {
    FCS2_0(Box<StdText2_0>),
    FCS3_0(Box<StdText3_0>),
    FCS3_1(Box<StdText3_1>),
    FCS3_2(Box<StdText3_2>),
}

impl AnyStdTEXT {
    pub fn print_meas_table(&self, delim: &str) {
        match self {
            AnyStdTEXT::FCS2_0(x) => x.print_meas_table(delim),
            AnyStdTEXT::FCS3_0(x) => x.print_meas_table(delim),
            AnyStdTEXT::FCS3_1(x) => x.print_meas_table(delim),
            AnyStdTEXT::FCS3_2(x) => x.print_meas_table(delim),
        }
    }

    pub fn print_spillover_table(&self, delim: &str) {
        let res = match self {
            AnyStdTEXT::FCS2_0(_) => None,
            AnyStdTEXT::FCS3_0(_) => None,
            AnyStdTEXT::FCS3_1(x) => x
                .core
                .metadata
                .specific
                .spillover
                .as_ref()
                .into_option()
                .map(|s| s.print_table(delim)),
            AnyStdTEXT::FCS3_2(x) => x
                .core
                .metadata
                .specific
                .spillover
                .as_ref()
                .into_option()
                .map(|s| s.print_table(delim)),
        };
        if res.is_none() {
            println!("None")
        }
    }
}

impl Serialize for AnyStdTEXT {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut state = serializer.serialize_struct("AnyStdTEXT", 2)?;
        match self {
            AnyStdTEXT::FCS2_0(x) => {
                state.serialize_field("version", &Version::FCS2_0)?;
                state.serialize_field("data", &x)?;
            }
            AnyStdTEXT::FCS3_0(x) => {
                state.serialize_field("version", &Version::FCS3_0)?;
                state.serialize_field("data", &x)?;
            }
            AnyStdTEXT::FCS3_1(x) => {
                state.serialize_field("version", &Version::FCS3_1)?;
                state.serialize_field("data", &x)?;
            }
            AnyStdTEXT::FCS3_2(x) => {
                state.serialize_field("version", &Version::FCS3_2)?;
                state.serialize_field("data", &x)?;
            }
        }
        state.end()
    }
}

#[derive(Debug)]
// TODO add warnings and such
pub struct ParsedTEXT {
    pub header: Header,
    pub raw: RawTEXT,
    pub standard: AnyStdTEXT,
    data_parser: DataParser,
    deprecated_keys: Vec<StdKey>,
    deprecated_features: Vec<String>,
    meta_warnings: Vec<String>,
    keyword_warnings: Vec<KeyWarning>,
}

type TEXTResult = Result<ParsedTEXT, Box<StdTEXTErrors>>;

impl<M: VersionedMetadata> StdText<M, M::P, M::R> {
    fn get_shortnames(&self) -> Vec<&str> {
        self.core
            .measurements
            .iter()
            .filter_map(|p| M::P::measurement_name(p))
            .collect()
    }

    // TODO char should be validated somehow
    fn text_segment(&self, delim: char, data_len: usize) -> String {
        let ms: Vec<_> = self
            .core
            .measurements
            .iter()
            .enumerate()
            .flat_map(|(i, m)| m.keywords(&(i + 1).to_string()))
            .flat_map(|(k, v)| v.map(|x| (k, x)))
            .collect();
        let meas_len = ms.iter().map(|(k, v)| k.len() + v.len() + 2).sum();
        let len = KwLengths {
            data: data_len,
            measurements: meas_len,
        };
        // TODO properly populate tot/par here
        let mut meta: Vec<(String, String)> = self
            .core
            .metadata
            .keywords(0, 0, len)
            .into_iter()
            .flat_map(|(k, v)| v.map(|x| (String::from(k), x)))
            .chain(ms)
            .collect();

        meta.sort_by(|a, b| a.0.cmp(&b.0));

        let fin = meta
            .into_iter()
            .map(|(k, v)| format!("{}{}{}", k, delim, v))
            .join(&delim.to_string());
        format!("{fin}{delim}")
    }

    fn meas_table(&self, delim: &str) -> Vec<String> {
        let ms = &self.core.measurements;
        if ms.is_empty() {
            return vec![];
        }
        let header = ms[0].table_header().join(delim);
        let rows = self.core.measurements.iter().enumerate().map(|(i, m)| {
            m.table_row(i)
                .into_iter()
                .map(|v| v.unwrap_or(String::from("NA")))
                .join(delim)
        });
        vec![header].into_iter().chain(rows).collect()
    }

    fn print_meas_table(&self, delim: &str) {
        for e in self.meas_table(delim) {
            println!("{}", e);
        }
    }

    fn raw_to_std_text(header: Header, raw: RawTEXT, conf: &StdTextReader) -> TEXTResult {
        let mut st = raw.to_state(conf);
        if let Some(par) = st.lookup_par() {
            // TODO in the case of the measurement dump table command this is
            // the only thing we really need to read, which will make it much
            // easier to bypass other "bad" keywords if all we wish to do is
            // look at the measurement table
            let ms = M::P::lookup_measurements(&mut st, par);
            let rd = M::R::lookup(&mut st, par);
            let md = ms.as_ref().and_then(|xs| M::lookup_metadata(&mut st, xs));
            if let (Some(measurements), Some(read_data), Some(metadata)) = (ms, rd, md) {
                let it = IntermediateTEXT {
                    data_offsets: header.data,
                    read_data,
                    metadata,
                    measurements,
                };
                let mut dp_st = st.into_data_parser_state();
                if let Some(data_parser) = M::build_data_parser(&mut dp_st, &it) {
                    dp_st.into_result(it, data_parser, header, raw)
                } else {
                    Err(Box::new(dp_st.into_errors()))
                }
            } else {
                // ...and chain new state thing down here, so that way the
                // errors have a natural "flow"
                Err(Box::new(st.into_errors()))
            }
        } else {
            Err(Box::new(st.into_errors()))
        }
    }
}

//     fn raw_to_std_text(header: Header, raw: RawTEXT, conf: &StdTextReader) -> TEXTResult {
//         let mut st = raw.to_state(conf);
//         if let Some((it, data_parser)) = st
//             .lookup_par()
//             .and_then(|par| M::P::lookup_measurements(&mut st, par))
//             .and_then(|ms| M::lookup_metadata(&mut st, &ms).map(|md| (ms, md)))
//             .and_then(|(ms, md)| {
//                 M::R::lookup(&mut st, ms.len()).map(|rd| IntermediateTEXT {
//                     data_offsets: header.data,
//                     read_data: rd,
//                     metadata: md,
//                     measurements: ms,
//                 })
//             })
//             .and_then(|it| M::build_data_parser(&mut st, &it).map(|dp| (it, dp)))
//         {
//             st.into_result(it, data_parser, header, raw)
//         } else {
//             Err(Box::new(st.into_errors()))
//         }
//     }
// }

struct IntermediateTEXT<M, P, R> {
    data_offsets: Segment,
    read_data: ReadData<R>,
    metadata: Metadata<M>,
    measurements: Vec<Measurement<P>>,
}

type StdText2_0 = StdText<InnerMetadata2_0, InnerMeasurement2_0, InnerReadData2_0>;
type StdText3_0 = StdText<InnerMetadata3_0, InnerMeasurement3_0, InnerReadData3_0>;
type StdText3_1 = StdText<InnerMetadata3_1, InnerMeasurement3_1, InnerReadData3_0>;
type StdText3_2 = StdText<InnerMetadata3_2, InnerMeasurement3_2, InnerReadData3_2>;

trait OrderedFromBytes<const DTLEN: usize, const OLEN: usize>: NumProps<DTLEN> {
    fn read_from_ordered<R: Read>(h: &mut BufReader<R>, order: &[u8; OLEN]) -> io::Result<Self> {
        let mut tmp = [0; OLEN];
        let mut buf = [0; DTLEN];
        h.read_exact(&mut tmp)?;
        for (i, j) in order.iter().enumerate() {
            buf[usize::from(*j)] = tmp[i];
        }
        Ok(Self::from_little(buf))
    }
}

// TODO where to put this?
fn byteord_to_sized<const LEN: usize>(byteord: &ByteOrd) -> Result<SizedByteOrd<LEN>, String> {
    match byteord {
        ByteOrd::Endian(e) => Ok(SizedByteOrd::Endian(*e)),
        ByteOrd::Mixed(v) => v[..]
            .try_into()
            .map(|order: [u8; LEN]| SizedByteOrd::Order(order))
            .or(Err(format!(
                "$BYTEORD is mixed but length is {} and not {LEN}",
                v.len()
            ))),
    }
}

trait IntFromBytes<const DTLEN: usize, const INTLEN: usize>:
    NumProps<DTLEN> + OrderedFromBytes<DTLEN, INTLEN> + Ord + IntMath
{
    fn byteord_to_sized(byteord: &ByteOrd) -> Result<SizedByteOrd<INTLEN>, String> {
        byteord_to_sized(byteord)
    }

    fn range_to_bitmask(range: &Range) -> Option<Self> {
        match range {
            Range::Float(_) => None,
            Range::Int(i) => Some(Self::next_power_2(Self::from_u64(*i))),
        }
    }

    fn to_col_parser(
        range: &Range,
        byteord: &ByteOrd,
        total_events: usize,
    ) -> Result<IntColumnParser<Self, INTLEN>, Vec<String>> {
        // TODO be more specific, which means we need the measurement index
        let b =
            Self::range_to_bitmask(range).ok_or(String::from("PnR is float for an integer column"));
        let s = Self::byteord_to_sized(byteord);
        let data = vec![Self::zero(); total_events];
        match (b, s) {
            (Ok(bitmask), Ok(size)) => Ok(IntColumnParser {
                bitmask,
                size,
                data,
            }),
            (Err(x), Err(y)) => Err(vec![x, y]),
            (Err(x), _) => Err(vec![x]),
            (_, Err(y)) => Err(vec![y]),
        }
    }

    fn read_int_masked<R: Read>(
        h: &mut BufReader<R>,
        byteord: &SizedByteOrd<INTLEN>,
        bitmask: Self,
    ) -> io::Result<Self> {
        Self::read_int(h, byteord).map(|x| x.min(bitmask))
    }

    fn read_int<R: Read>(h: &mut BufReader<R>, byteord: &SizedByteOrd<INTLEN>) -> io::Result<Self> {
        // This lovely code will read data that is not a power-of-two
        // bytes long. Start by reading n bytes into a vector, which can
        // take a varying size. Then copy this into the power of 2 buffer
        // and reset all the unused cells to 0. This copy has to go to one
        // or the other end of the buffer depending on endianness.
        //
        // ASSUME for u8 and u16 that these will get heavily optimized away
        // since 'order' is totally meaningless for u8 and the only two possible
        // 'orders' for u16 are big and little.
        let mut tmp = [0; INTLEN];
        let mut buf = [0; DTLEN];
        match byteord {
            SizedByteOrd::Endian(Endian::Big) => {
                let b = DTLEN - INTLEN;
                h.read_exact(&mut tmp)?;
                buf[b..].copy_from_slice(&tmp[b..]);
                Ok(Self::from_big(buf))
            }
            SizedByteOrd::Endian(Endian::Little) => {
                h.read_exact(&mut tmp)?;
                buf[..INTLEN].copy_from_slice(&tmp[..INTLEN]);
                Ok(Self::from_little(buf))
            }
            SizedByteOrd::Order(order) => Self::read_from_ordered(h, order),
        }
    }

    fn assign<R: Read>(
        h: &mut BufReader<R>,
        d: &mut IntColumnParser<Self, INTLEN>,
        row: usize,
    ) -> io::Result<()> {
        d.data[row] = Self::read_int_masked(h, &d.size, d.bitmask)?;
        Ok(())
    }
}

trait FloatFromBytes<const LEN: usize>:
    NumProps<LEN> + OrderedFromBytes<LEN, LEN> + Clone + NumProps<LEN>
{
    fn to_float_byteord(byteord: &ByteOrd) -> Result<SizedByteOrd<LEN>, String> {
        byteord_to_sized(byteord)
    }

    fn make_column_parser(endian: Endian, total_events: usize) -> FloatColumn<Self> {
        FloatColumn {
            data: vec![Self::zero(); total_events],
            endian,
        }
    }

    fn make_matrix_parser(
        byteord: &ByteOrd,
        par: usize,
        total_events: usize,
    ) -> Result<FloatParser<LEN>, String> {
        Self::to_float_byteord(byteord).map(|byteord| FloatParser {
            nrows: total_events,
            ncols: par,
            byteord,
        })
    }

    fn read_float<R: Read>(h: &mut BufReader<R>, byteord: &SizedByteOrd<LEN>) -> io::Result<Self> {
        let mut buf = [0; LEN];
        match byteord {
            SizedByteOrd::Endian(Endian::Big) => {
                h.read_exact(&mut buf)?;
                Ok(Self::from_big(buf))
            }
            SizedByteOrd::Endian(Endian::Little) => {
                h.read_exact(&mut buf)?;
                Ok(Self::from_little(buf))
            }
            SizedByteOrd::Order(order) => Self::read_from_ordered(h, order),
        }
    }

    fn assign_column<R: Read>(
        h: &mut BufReader<R>,
        column: &mut FloatColumn<Self>,
        row: usize,
    ) -> io::Result<()> {
        // TODO endian wrap thing seems unnecessary
        column.data[row] = Self::read_float(h, &SizedByteOrd::Endian(column.endian))?;
        Ok(())
    }

    fn assign_matrix<R: Read + Seek>(
        h: &mut BufReader<R>,
        d: &FloatParser<LEN>,
        column: &mut [Self],
        row: usize,
    ) -> io::Result<()> {
        column[row] = Self::read_float(h, &d.byteord)?;
        Ok(())
    }

    fn parse_matrix<R: Read + Seek>(
        h: &mut BufReader<R>,
        p: FloatParser<LEN>,
    ) -> io::Result<Vec<Series>> {
        let mut columns: Vec<_> = iter::repeat_with(|| vec![Self::zero(); p.nrows])
            .take(p.ncols)
            .collect();
        for r in 0..p.nrows {
            for c in columns.iter_mut() {
                Self::assign_matrix(h, &p, c, r)?;
            }
        }
        Ok(columns.into_iter().map(Self::into_series).collect())
    }
}

impl OrderedFromBytes<1, 1> for u8 {}
impl OrderedFromBytes<2, 2> for u16 {}
impl OrderedFromBytes<4, 3> for u32 {}
impl OrderedFromBytes<4, 4> for u32 {}
impl OrderedFromBytes<8, 5> for u64 {}
impl OrderedFromBytes<8, 6> for u64 {}
impl OrderedFromBytes<8, 7> for u64 {}
impl OrderedFromBytes<8, 8> for u64 {}
impl OrderedFromBytes<4, 4> for f32 {}
impl OrderedFromBytes<8, 8> for f64 {}

impl FloatFromBytes<4> for f32 {}
impl FloatFromBytes<8> for f64 {}

impl IntFromBytes<1, 1> for u8 {}
impl IntFromBytes<2, 2> for u16 {}
impl IntFromBytes<4, 3> for u32 {}
impl IntFromBytes<4, 4> for u32 {}
impl IntFromBytes<8, 5> for u64 {}
impl IntFromBytes<8, 6> for u64 {}
impl IntFromBytes<8, 7> for u64 {}
impl IntFromBytes<8, 8> for u64 {}

#[derive(Debug)]
struct FloatParser<const LEN: usize> {
    nrows: usize,
    ncols: usize,
    byteord: SizedByteOrd<LEN>,
}

#[derive(Debug)]
struct AsciiColumn {
    data: Vec<f64>,
    width: u8,
}

#[derive(Debug)]
struct FloatColumn<T> {
    data: Vec<T>,
    endian: Endian,
}

#[derive(Debug)]
enum MixedColumnType {
    Ascii(AsciiColumn),
    Uint(AnyIntColumn),
    Single(FloatColumn<f32>),
    Double(FloatColumn<f64>),
}

impl MixedColumnType {
    fn into_series(self) -> Series {
        match self {
            MixedColumnType::Ascii(x) => f64::into_series(x.data),
            MixedColumnType::Single(x) => f32::into_series(x.data),
            MixedColumnType::Double(x) => f64::into_series(x.data),
            MixedColumnType::Uint(x) => x.into_series(),
        }
    }
}

#[derive(Debug)]
struct MixedParser {
    nrows: usize,
    columns: Vec<MixedColumnType>,
}

#[derive(Debug)]
enum SizedByteOrd<const LEN: usize> {
    Endian(Endian),
    Order([u8; LEN]),
}

#[derive(Debug)]
struct IntColumnParser<B, const LEN: usize> {
    bitmask: B,
    size: SizedByteOrd<LEN>,
    data: Vec<B>,
}

#[derive(Debug)]
enum AnyIntColumn {
    Uint8(IntColumnParser<u8, 1>),
    Uint16(IntColumnParser<u16, 2>),
    Uint24(IntColumnParser<u32, 3>),
    Uint32(IntColumnParser<u32, 4>),
    Uint40(IntColumnParser<u64, 5>),
    Uint48(IntColumnParser<u64, 6>),
    Uint56(IntColumnParser<u64, 7>),
    Uint64(IntColumnParser<u64, 8>),
}

impl AnyIntColumn {
    fn into_series(self) -> Series {
        match self {
            AnyIntColumn::Uint8(y) => u8::into_series(y.data),
            AnyIntColumn::Uint16(y) => u16::into_series(y.data),
            AnyIntColumn::Uint24(y) => u32::into_series(y.data),
            AnyIntColumn::Uint32(y) => u32::into_series(y.data),
            AnyIntColumn::Uint40(y) => u64::into_series(y.data),
            AnyIntColumn::Uint48(y) => u64::into_series(y.data),
            AnyIntColumn::Uint56(y) => u64::into_series(y.data),
            AnyIntColumn::Uint64(y) => u64::into_series(y.data),
        }
    }

    fn assign<R: Read>(&mut self, h: &mut BufReader<R>, r: usize) -> io::Result<()> {
        match self {
            AnyIntColumn::Uint8(d) => u8::assign(h, d, r)?,
            AnyIntColumn::Uint16(d) => u16::assign(h, d, r)?,
            AnyIntColumn::Uint24(d) => u32::assign(h, d, r)?,
            AnyIntColumn::Uint32(d) => u32::assign(h, d, r)?,
            AnyIntColumn::Uint40(d) => u64::assign(h, d, r)?,
            AnyIntColumn::Uint48(d) => u64::assign(h, d, r)?,
            AnyIntColumn::Uint56(d) => u64::assign(h, d, r)?,
            AnyIntColumn::Uint64(d) => u64::assign(h, d, r)?,
        }
        Ok(())
    }
}

// Integers are complicated because in each version we need to at least deal
// with the possibility that each column has a different bitmask. In addition,
// 3.1+ allows for different widths (even though this likely is used seldom
// if ever) so each series can potentially be a different type. Finally,
// BYTEORD further complicates this because unlike floats which can only have
// widths of 4 or 8 bytes, integers can have any number of bytes up to their
// next power of 2 data type. For example, some cytometers store their values
// in 3-byte segments, which would need to be stored in u32 but are read as
// triples, which in theory could be any byte order.
//
// There may be some small optimizations we can make for the "typical" cases
// where the entire file is u32 with big/little BYTEORD and only a handful
// of different bitmasks. For now, the increased complexity of dealing with this
// is likely no worth it.
#[derive(Debug)]
struct IntParser {
    nrows: usize,
    columns: Vec<AnyIntColumn>,
}

#[derive(Debug)]
struct FixedAsciiParser {
    columns: Vec<u8>,
    nrows: usize,
}

#[derive(Debug)]
struct DelimAsciiParser {
    ncols: usize,
    nrows: Option<usize>,
    nbytes: usize,
}

#[derive(Debug)]
enum ColumnParser {
    // DATATYPE=A where all PnB = *
    DelimitedAscii(DelimAsciiParser),
    // DATATYPE=A where all PnB = number
    FixedWidthAscii(FixedAsciiParser),
    // DATATYPE=F (with no overrides in 3.2+)
    Single(FloatParser<4>),
    // DATATYPE=D (with no overrides in 3.2+)
    Double(FloatParser<8>),
    // DATATYPE=I this is complicated so see struct definition
    Int(IntParser),
    // Mixed column types (3.2+)
    Mixed(MixedParser),
}

#[derive(Debug)]
struct DataParser {
    column_parser: ColumnParser,
    begin: u64,
}

type ParsedData = Vec<Series>;

fn format_parsed_data(res: &FCSSuccess, delim: &str) -> Vec<String> {
    let shortnames = match &res.std {
        AnyStdTEXT::FCS2_0(x) => x.get_shortnames(),
        AnyStdTEXT::FCS3_0(x) => x.get_shortnames(),
        AnyStdTEXT::FCS3_1(x) => x.get_shortnames(),
        AnyStdTEXT::FCS3_2(x) => x.get_shortnames(),
    };
    if res.data.is_empty() {
        return vec![];
    }
    let mut buf = vec![];
    let mut lines = vec![];
    let nrows = res.data[0].len();
    let ncols = res.data.len();
    // ASSUME names is the same length as columns
    lines.push(shortnames.join(delim));
    for r in 0..nrows {
        buf.clear();
        for c in 0..ncols {
            buf.push(res.data[c].format(r));
        }
        lines.push(buf.join(delim));
    }
    lines
}

pub fn print_parsed_data(res: &FCSSuccess, delim: &str) {
    for x in format_parsed_data(res, delim) {
        println!("{}", x);
    }
}

fn ascii_to_float_io(buf: Vec<u8>) -> io::Result<f64> {
    String::from_utf8(buf)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
        .and_then(|s| parse_f64_io(&s))
}

fn parse_f64_io(s: &str) -> io::Result<f64> {
    s.parse::<f64>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

fn read_data_delim_ascii<R: Read>(
    h: &mut BufReader<R>,
    p: DelimAsciiParser,
) -> io::Result<ParsedData> {
    let mut buf = Vec::new();
    let mut row = 0;
    let mut col = 0;
    let mut last_was_delim = false;
    // Delimiters are tab, newline, carriage return, space, or comma. Any
    // consecutive delimiter counts as one, and delimiters can be mixed.
    let is_delim = |byte| byte == 9 || byte == 10 || byte == 13 || byte == 32 || byte == 44;
    // FCS 2.0 files have an optional $TOT field, which complicates this a bit
    if let Some(nrows) = p.nrows {
        let mut data: Vec<_> = iter::repeat_with(|| vec![0.0; nrows])
            .take(p.ncols)
            .collect();
        for b in h.bytes().take(p.nbytes) {
            let byte = b?;
            // exit if we encounter more rows than expected.
            if row == nrows {
                let msg = format!("Exceeded expected number of rows: {nrows}");
                return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
            }
            if is_delim(byte) {
                if !last_was_delim {
                    last_was_delim = true;
                    // TODO this will spaz out if we end up reading more
                    // rows than expected
                    data[col][row] = ascii_to_float_io(buf.clone())?;
                    buf.clear();
                    if col == p.ncols - 1 {
                        col = 0;
                        row += 1;
                    } else {
                        col += 1;
                    }
                }
            } else {
                buf.push(byte);
            }
        }
        // The spec isn't clear if the last value should be a delim or
        // not, so flush the buffer if it has anything in it since we
        // only try to parse if we hit a delim above.
        if !buf.is_empty() {
            data[col][row] = ascii_to_float_io(buf.clone())?;
        }
        if !(col == 0 && row == nrows) {
            let msg = format!(
                "Parsing ended in column {col} and row {row}, \
                               where expected number of rows is {nrows}"
            );
            return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
        }
        Ok(data.into_iter().map(f64::into_series).collect())
    } else {
        let mut data: Vec<_> = iter::repeat_with(Vec::new).take(p.ncols).collect();
        for b in h.bytes().take(p.nbytes) {
            let byte = b?;
            // Delimiters are tab, newline, carriage return, space, or
            // comma. Any consecutive delimiter counts as one, and
            // delimiters can be mixed.
            if is_delim(byte) {
                if !last_was_delim {
                    last_was_delim = true;
                    data[col].push(ascii_to_float_io(buf.clone())?);
                    buf.clear();
                    if col == p.ncols - 1 {
                        col = 0;
                    } else {
                        col += 1;
                    }
                }
            } else {
                buf.push(byte);
            }
        }
        // The spec isn't clear if the last value should be a delim or
        // not, so flush the buffer if it has anything in it since we
        // only try to parse if we hit a delim above.
        if !buf.is_empty() {
            data[col][row] = ascii_to_float_io(buf.clone())?;
        }
        // Scream if not all columns are equal in length
        if data.iter().map(|c| c.len()).unique().count() > 1 {
            let msg = "Not all columns are equal length";
            return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
        }
        Ok(data.into_iter().map(f64::into_series).collect())
    }
}

fn read_data_ascii_fixed<R: Read>(
    h: &mut BufReader<R>,
    parser: &FixedAsciiParser,
) -> io::Result<ParsedData> {
    let ncols = parser.columns.len();
    let mut data: Vec<_> = iter::repeat_with(|| vec![0.0; parser.nrows])
        .take(ncols)
        .collect();
    let mut buf = String::new();
    for r in 0..parser.nrows {
        for (c, width) in parser.columns.iter().enumerate() {
            buf.clear();
            h.take(u64::from(*width)).read_to_string(&mut buf)?;
            data[c][r] = parse_f64_io(&buf)?;
        }
    }
    Ok(data.into_iter().map(f64::into_series).collect())
}

fn read_data_mixed<R: Read>(h: &mut BufReader<R>, parser: MixedParser) -> io::Result<ParsedData> {
    let mut p = parser;
    let mut strbuf = String::new();
    for r in 0..p.nrows {
        for c in p.columns.iter_mut() {
            match c {
                MixedColumnType::Single(t) => f32::assign_column(h, t, r)?,
                MixedColumnType::Double(t) => f64::assign_column(h, t, r)?,
                MixedColumnType::Uint(u) => u.assign(h, r)?,
                MixedColumnType::Ascii(d) => {
                    strbuf.clear();
                    h.take(u64::from(d.width)).read_to_string(&mut strbuf)?;
                    d.data[r] = parse_f64_io(&strbuf)?;
                }
            }
        }
    }
    Ok(p.columns.into_iter().map(|c| c.into_series()).collect())
}

fn read_data_int<R: Read>(h: &mut BufReader<R>, parser: IntParser) -> io::Result<ParsedData> {
    let mut p = parser;
    for r in 0..p.nrows {
        for c in p.columns.iter_mut() {
            c.assign(h, r)?;
        }
    }
    Ok(p.columns.into_iter().map(|c| c.into_series()).collect())
}

fn read_data<R: Read + Seek>(h: &mut BufReader<R>, parser: DataParser) -> io::Result<ParsedData> {
    h.seek(SeekFrom::Start(parser.begin))?;
    match parser.column_parser {
        ColumnParser::DelimitedAscii(p) => read_data_delim_ascii(h, p),
        ColumnParser::FixedWidthAscii(p) => read_data_ascii_fixed(h, &p),
        ColumnParser::Single(p) => f32::parse_matrix(h, p),
        ColumnParser::Double(p) => f64::parse_matrix(h, p),
        ColumnParser::Mixed(p) => read_data_mixed(h, p),
        ColumnParser::Int(p) => read_data_int(h, p),
    }
}

enum EventWidth {
    Finite(Vec<u8>),
    Variable,
    Error(Vec<usize>, Vec<usize>),
}

type MaybeKeyword = (&'static str, Option<String>);

type MaybeKeywords = Vec<MaybeKeyword>;

/// Used to hold critical lengths when calculating the pad for $BEGIN/ENDDATA.
struct KwLengths {
    /// Length of the entire DATA segment when written.
    data: usize,
    /// Length of all the measurement keywords in the TEXT segment.
    ///
    /// This is computed as the sum of all string lengths of each key and
    /// value plus 2*P (number of measurements) which captures the length
    /// of the two delimiters b/t the key and value and the key and previous
    /// value.
    measurements: usize,
}

fn sum_keywords(kws: &[MaybeKeyword]) -> usize {
    kws.iter()
        .map(|(k, v)| v.as_ref().map(|y| y.len() + k.len() + 2).unwrap_or(0))
        .sum()
}

fn n_digits(x: f64) -> f64 {
    // ASSUME this is effectively only going to be used on the u32 range
    // starting at 1; keep in f64 space to minimize casts
    f64::log10(x).floor() + 1.0
}

fn compute_data_offsets(textlen: u32, datalen: u32) -> (u32, u32) {
    let d = f64::from(datalen);
    let t = f64::from(textlen);
    let mut datastart = t;
    let mut dataend = datastart + d;
    let mut ndigits_start = n_digits(datastart);
    let mut ndigits_end = n_digits(dataend);
    let mut tmp_start;
    let mut tmp_end;
    loop {
        datastart = ndigits_start + ndigits_end + t;
        dataend = datastart + d;
        tmp_start = n_digits(datastart);
        tmp_end = n_digits(dataend);
        if tmp_start == ndigits_start && tmp_end == ndigits_end {
            return (datastart as u32, dataend as u32);
        } else {
            ndigits_start = tmp_start;
            ndigits_end = tmp_end;
        }
    }
}

// ASSUME header is always this length
const HEADER_LEN: usize = 58;

// length of BEGIN/ENDDATA keywords (without values), + 4 to account for
// delimiters
const DATALEN_NO_VAL: usize = BEGINDATA.len() + ENDDATA.len() + 4;

fn make_data_offset_keywords(other_textlen: usize, datalen: usize) -> [MaybeKeyword; 2] {
    // add everything up, + 1 at the end to account for the delimiter at
    // the end of TEXT
    let textlen = HEADER_LEN + DATALEN_NO_VAL + other_textlen + 1;
    let (datastart, dataend) = compute_data_offsets(textlen as u32, datalen as u32);
    [
        (BEGINDATA, Some(datastart.to_string())),
        (ENDDATA, Some(dataend.to_string())),
    ]
}

trait VersionedMetadata: Sized {
    type P: VersionedMeasurement;
    type R: VersionedReadData;

    fn into_any_text(s: Box<StdText<Self, Self::P, Self::R>>) -> AnyStdTEXT;

    fn get_byteord(&self) -> ByteOrd;

    fn event_width(ms: &[Measurement<Self::P>]) -> EventWidth {
        let (fixed, variable_indices): (Vec<_>, Vec<_>) = ms
            .iter()
            .enumerate()
            .map(|(i, p)| match p.bytes {
                Bytes::Fixed(b) => Ok((i, b)),
                Bytes::Variable => Err(i),
            })
            .partition_result();
        let (fixed_indices, fixed_bytes): (Vec<_>, Vec<_>) = fixed.into_iter().unzip();
        if variable_indices.is_empty() {
            EventWidth::Finite(fixed_bytes)
        } else if fixed_indices.is_empty() {
            EventWidth::Variable
        } else {
            EventWidth::Error(fixed_indices, variable_indices)
        }
    }

    fn total_events(
        st: &mut DataParserState,
        it: &IntermediateTEXT<Self, Self::P, Self::R>,
        event_width: u32,
    ) -> Option<usize> {
        let nbytes = it
            .read_data
            .specific
            .data_offsets(&it.data_offsets)
            .num_bytes();
        let remainder = nbytes % event_width;
        let res = nbytes / event_width;
        let total_events = if nbytes % event_width > 0 {
            let msg = format!(
                "Events are {event_width} bytes wide, but this does not evenly \
                 divide DATA segment which is {nbytes} bytes long \
                 (remainder of {remainder})"
            );
            if st.conf.raw.enfore_data_width_divisibility {
                st.push_meta_error(msg);
                None
            } else {
                st.push_meta_warning(msg);
                Some(res)
            }
        } else {
            Some(res)
        };
        total_events.and_then(|x| {
            if let Some(tot) = it.read_data.specific.get_tot() {
                if x != tot {
                    let msg = format!(
                        "$TOT field is {tot} but number of events \
                         that evenly fit into DATA is {x}"
                    );
                    if st.conf.raw.enfore_matching_tot {
                        st.push_meta_error(msg);
                    } else {
                        st.push_meta_warning(msg);
                    }
                }
            }
            usize::try_from(x).ok()
        })
    }

    fn build_int_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        total_events: usize,
    ) -> Option<IntParser>;

    fn build_mixed_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        dt: &AlphaNumType,
        total_events: usize,
    ) -> Option<Option<MixedParser>>;

    fn build_float_parser(
        &self,
        st: &mut DataParserState,
        is_double: bool,
        par: usize,
        total_events: usize,
        ps: &[Measurement<Self::P>],
    ) -> Option<ColumnParser> {
        let (bytes, dt) = if is_double { (8, "D") } else { (4, "F") };
        let remainder: Vec<_> = ps.iter().filter(|p| p.bytes_eq(bytes)).collect();
        if remainder.is_empty() {
            let byteord = &self.get_byteord();
            let res = if is_double {
                f64::make_matrix_parser(byteord, par, total_events).map(ColumnParser::Double)
            } else {
                f32::make_matrix_parser(byteord, par, total_events).map(ColumnParser::Single)
            };
            match res {
                Ok(x) => Some(x),
                Err(e) => {
                    st.push_meta_error(e);
                    None
                }
            }
        } else {
            for e in remainder.iter().enumerate().map(|(i, p)| {
                format!(
                    "Measurment {} uses {} bytes but DATATYPE={}",
                    i, p.bytes, dt
                )
            }) {
                st.push_meta_error(e);
            }
            None
        }
    }

    fn build_fixed_width_parser(
        st: &mut DataParserState,
        it: &IntermediateTEXT<Self, Self::P, Self::R>,
        total_events: usize,
        measurement_widths: Vec<u8>,
    ) -> Option<ColumnParser> {
        // TODO fix cast?
        let par = it.read_data.par;
        let ps = &it.measurements;
        let dt = &it.metadata.datatype;
        let specific = &it.metadata.specific;
        if let Some(mixed) = Self::build_mixed_parser(specific, st, ps, dt, total_events) {
            mixed.map(ColumnParser::Mixed)
        } else {
            match dt {
                AlphaNumType::Single => {
                    specific.build_float_parser(st, false, par, total_events, ps)
                }
                AlphaNumType::Double => {
                    specific.build_float_parser(st, true, par, total_events, ps)
                }
                AlphaNumType::Integer => specific
                    .build_int_parser(st, ps, total_events)
                    .map(ColumnParser::Int),
                AlphaNumType::Ascii => Some(ColumnParser::FixedWidthAscii(FixedAsciiParser {
                    columns: measurement_widths,
                    nrows: total_events,
                })),
            }
        }
    }

    fn build_delim_ascii_parser(
        it: &IntermediateTEXT<Self, Self::P, Self::R>,
        tot: Option<usize>,
    ) -> ColumnParser {
        let nbytes = it
            .read_data
            .specific
            .data_offsets(&it.data_offsets)
            .num_bytes();
        ColumnParser::DelimitedAscii(DelimAsciiParser {
            ncols: it.read_data.par,
            nrows: tot,
            nbytes: nbytes as usize,
        })
    }

    fn build_column_parser(
        st: &mut DataParserState,
        it: &IntermediateTEXT<Self, Self::P, Self::R>,
    ) -> Option<ColumnParser> {
        // In order to make a data parser, the $DATATYPE, $BYTEORD, $PnB, and
        // $PnDATATYPE (if present) all need to be a specific relationship to
        // each other, each of which corresponds to the options below.
        if it.metadata.datatype == AlphaNumType::Ascii && Self::P::fcs_version() >= Version::FCS3_1
        {
            st.push_meta_deprecated_str("$DATATYPE=A has been deprecated since FCS 3.1");
        }
        match (Self::event_width(&it.measurements), it.metadata.datatype) {
            // Numeric/Ascii (fixed width)
            (EventWidth::Finite(measurement_widths), _) => {
                let event_width = measurement_widths.iter().map(|x| u32::from(*x)).sum();
                Self::total_events(st, it, event_width).and_then(|total_events| {
                    Self::build_fixed_width_parser(st, it, total_events, measurement_widths)
                })
            }
            // Ascii (variable width)
            (EventWidth::Variable, AlphaNumType::Ascii) => {
                let tot = it.read_data.specific.get_tot();
                Some(Self::build_delim_ascii_parser(it, tot.map(|x| x as usize)))
            }
            // nonsense...scream at user
            (EventWidth::Error(fixed, variable), _) => {
                st.push_meta_error_str("$PnBs are a mix of numeric and variable");
                for f in fixed {
                    st.push_meta_error(format!("$PnB for measurement {f} is numeric"));
                }
                for v in variable {
                    st.push_meta_error(format!("$PnB for measurement {v} is variable"));
                }
                None
            }
            (EventWidth::Variable, dt) => {
                st.push_meta_error(format!("$DATATYPE is {dt} but all $PnB are '*'"));
                None
            }
        }
    }

    fn build_data_parser(
        st: &mut DataParserState,
        it: &IntermediateTEXT<Self, Self::P, Self::R>,
    ) -> Option<DataParser> {
        Self::build_column_parser(st, it).map(|column_parser| DataParser {
            column_parser,
            begin: u64::from(it.read_data.specific.data_offsets(&it.data_offsets).begin),
        })
    }

    fn lookup_specific(st: &mut KwState, par: usize, names: &HashSet<&str>) -> Option<Self>;

    fn lookup_metadata(st: &mut KwState, ms: &[Measurement<Self::P>]) -> Option<Metadata<Self>> {
        let names: HashSet<_> = ms
            .iter()
            .filter_map(|m| Self::P::measurement_name(m))
            .collect();
        let par = ms.len();
        let maybe_datatype = st.lookup_datatype();
        let maybe_specific = Self::lookup_specific(st, par, &names);
        if let (Some(datatype), Some(specific)) = (maybe_datatype, maybe_specific) {
            Some(Metadata {
                datatype,
                abrt: st.lookup_abrt(),
                com: st.lookup_com(),
                cells: st.lookup_cells(),
                exp: st.lookup_exp(),
                fil: st.lookup_fil(),
                inst: st.lookup_inst(),
                lost: st.lookup_lost(),
                op: st.lookup_op(),
                proj: st.lookup_proj(),
                smno: st.lookup_smno(),
                src: st.lookup_src(),
                sys: st.lookup_sys(),
                tr: st.lookup_trigger_checked(&names),
                specific,
            })
        } else {
            None
        }
    }

    fn keywords_inner(&self, other_textlen: usize, data_len: usize) -> MaybeKeywords;

    fn keywords(m: &Metadata<Self>, par: usize, tot: usize, len: KwLengths) -> MaybeKeywords {
        let fixed = [
            (PAR, Some(par.to_string())),
            (TOT, Some(tot.to_string())),
            (NEXTDATA, Some("0".to_string())),
            (DATATYPE, Some(m.datatype.to_string())),
            (ABRT, m.abrt.as_opt_string()),
            (COM, m.com.as_opt_string()),
            (CELLS, m.cells.as_opt_string()),
            (EXP, m.exp.as_opt_string()),
            (FIL, m.fil.as_opt_string()),
            (INST, m.inst.as_opt_string()),
            (LOST, m.lost.as_opt_string()),
            (OP, m.op.as_opt_string()),
            (PROJ, m.proj.as_opt_string()),
            (SMNO, m.smno.as_opt_string()),
            (SRC, m.src.as_opt_string()),
            (SYS, m.sys.as_opt_string()),
            (TR, m.tr.as_opt_string()),
        ];
        let fixed_len = sum_keywords(&fixed) + len.measurements;
        fixed
            .into_iter()
            .chain(m.specific.keywords_inner(fixed_len, len.data))
            .collect()
    }
}

fn build_int_parser_2_0<P: VersionedMeasurement>(
    st: &mut DataParserState,
    byteord: &ByteOrd,
    ps: &[Measurement<P>],
    total_events: usize,
) -> Option<IntParser> {
    let nbytes = byteord.num_bytes();
    let remainder: Vec<_> = ps.iter().filter(|p| !p.bytes_eq(nbytes)).collect();
    if remainder.is_empty() {
        let (columns, fail): (Vec<_>, Vec<_>) = ps
            .iter()
            .map(|p| p.make_int_parser(byteord, total_events))
            .partition_result();
        let errors: Vec<_> = fail.into_iter().flatten().collect();
        if errors.is_empty() {
            Some(IntParser {
                columns,
                nrows: total_events,
            })
        } else {
            for e in errors.into_iter() {
                st.push_meta_error(e);
            }
            None
        }
    } else {
        for e in remainder.iter().enumerate().map(|(i, p)| {
            format!(
                "Measurement {} uses {} bytes when DATATYPE=I \
                         and BYTEORD implies {} bytes",
                i, p.bytes, nbytes
            )
        }) {
            st.push_meta_error(e);
        }
        None
    }
}

impl VersionedMetadata for InnerMetadata2_0 {
    type P = InnerMeasurement2_0;
    type R = InnerReadData2_0;

    fn into_any_text(t: Box<StdText2_0>) -> AnyStdTEXT {
        AnyStdTEXT::FCS2_0(t)
    }

    // fn get_data_offsets(s: &StdText<Self, Self::P, Self::R>) -> Offsets {
    //     s.data_offsets
    // }

    fn get_byteord(&self) -> ByteOrd {
        self.byteord.clone()
    }

    fn build_int_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        total_events: usize,
    ) -> Option<IntParser> {
        build_int_parser_2_0(st, &self.byteord, ps, total_events)
    }

    fn build_mixed_parser(
        &self,
        _: &mut DataParserState,
        _: &[Measurement<Self::P>],
        _: &AlphaNumType,
        _: usize,
    ) -> Option<Option<MixedParser>> {
        None
    }

    fn lookup_specific(
        st: &mut KwState,
        par: usize,
        _: &HashSet<&str>,
    ) -> Option<InnerMetadata2_0> {
        let maybe_mode = st.lookup_mode();
        let maybe_byteord = st.lookup_byteord();
        if let (Some(mode), Some(byteord)) = (maybe_mode, maybe_byteord) {
            Some(InnerMetadata2_0 {
                mode,
                byteord,
                cyt: st.lookup_cyt_opt(),
                comp: st.lookup_compensation_2_0(par),
                timestamps: st.lookup_timestamps2_0(),
            })
        } else {
            None
        }
    }

    fn keywords_inner(&self, _: usize, _: usize) -> MaybeKeywords {
        [
            (MODE, Some(self.mode.to_string())),
            (BYTEORD, Some(self.byteord.to_string())),
            (CYT, self.cyt.as_opt_string()),
            (COMP, self.comp.as_opt_string()),
            (BTIM, self.timestamps.btim.as_opt_string()),
            (ETIM, self.timestamps.etim.as_opt_string()),
            (DATE, self.timestamps.date.as_opt_string()),
        ]
        .into_iter()
        .collect()
    }
}

impl VersionedMetadata for InnerMetadata3_0 {
    type P = InnerMeasurement3_0;
    type R = InnerReadData3_0;

    fn into_any_text(t: Box<StdText3_0>) -> AnyStdTEXT {
        AnyStdTEXT::FCS3_0(t)
    }

    fn get_byteord(&self) -> ByteOrd {
        self.byteord.clone()
    }

    fn build_int_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        total_events: usize,
    ) -> Option<IntParser> {
        build_int_parser_2_0(st, &self.byteord, ps, total_events)
    }

    fn build_mixed_parser(
        &self,
        _: &mut DataParserState,
        _: &[Measurement<Self::P>],
        _: &AlphaNumType,
        _: usize,
    ) -> Option<Option<MixedParser>> {
        None
    }

    fn lookup_specific(
        st: &mut KwState,
        _: usize,
        names: &HashSet<&str>,
    ) -> Option<InnerMetadata3_0> {
        let maybe_mode = st.lookup_mode();
        let maybe_byteord = st.lookup_byteord();
        if let (Some(mode), Some(byteord)) = (maybe_mode, maybe_byteord) {
            Some(InnerMetadata3_0 {
                mode,
                byteord,
                cyt: st.lookup_cyt_opt(),
                comp: st.lookup_compensation_3_0(),
                timestamps: st.lookup_timestamps3_0(),
                cytsn: st.lookup_cytsn(),
                timestep: st.lookup_timestep_checked(names),
                unicode: st.lookup_unicode(),
            })
        } else {
            None
        }
    }

    fn keywords_inner(&self, other_textlen: usize, data_len: usize) -> MaybeKeywords {
        let ts = &self.timestamps;
        // TODO set analysis and stext if we have anything
        let zero = Some("0".to_string());
        let kws = [
            (BEGINANALYSIS, zero.clone()),
            (ENDANALYSIS, zero.clone()),
            (BEGINSTEXT, zero.clone()),
            (ENDSTEXT, zero.clone()),
            (MODE, Some(self.mode.to_string())),
            (BYTEORD, Some(self.byteord.to_string())),
            (CYT, self.cyt.as_opt_string()),
            (COMP, self.comp.as_opt_string()),
            (BTIM, ts.btim.as_opt_string()),
            (ETIM, ts.etim.as_opt_string()),
            (DATE, ts.date.as_opt_string()),
            (CYTSN, self.cytsn.as_opt_string()),
            (TIMESTEP, self.timestep.as_opt_string()),
            (UNICODE, self.unicode.as_opt_string()),
        ];
        let text_len = other_textlen + sum_keywords(&kws);
        make_data_offset_keywords(text_len, data_len)
            .into_iter()
            .chain(kws)
            .collect()
    }
}

impl VersionedMetadata for InnerMetadata3_1 {
    type P = InnerMeasurement3_1;
    type R = InnerReadData3_0;

    fn into_any_text(t: Box<StdText3_1>) -> AnyStdTEXT {
        AnyStdTEXT::FCS3_1(t)
    }

    fn get_byteord(&self) -> ByteOrd {
        ByteOrd::Endian(self.byteord)
    }

    fn build_int_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        total_events: usize,
    ) -> Option<IntParser> {
        build_int_parser_2_0(st, &ByteOrd::Endian(self.byteord), ps, total_events)
    }

    fn build_mixed_parser(
        &self,
        _: &mut DataParserState,
        _: &[Measurement<Self::P>],
        _: &AlphaNumType,
        _: usize,
    ) -> Option<Option<MixedParser>> {
        None
    }

    fn lookup_specific(
        st: &mut KwState,
        _: usize,
        names: &HashSet<&str>,
    ) -> Option<InnerMetadata3_1> {
        let maybe_mode = st.lookup_mode();
        let maybe_byteord = st.lookup_endian();
        if let (Some(mode), Some(byteord)) = (maybe_mode, maybe_byteord) {
            if mode != Mode::List {
                st.push_meta_deprecated_str("$MODE should only be L");
            };
            Some(InnerMetadata3_1 {
                mode,
                byteord,
                cyt: st.lookup_cyt_opt(),
                spillover: st.lookup_spillover_checked(names),
                timestamps: st.lookup_timestamps3_1(false),
                cytsn: st.lookup_cytsn(),
                timestep: st.lookup_timestep_checked(names),
                modification: st.lookup_modification(),
                plate: st.lookup_plate(false),
                vol: st.lookup_vol(),
            })
        } else {
            None
        }
    }

    fn keywords_inner(&self, other_textlen: usize, data_len: usize) -> MaybeKeywords {
        let mdn = &self.modification;
        let ts = &self.timestamps;
        let pl = &self.plate;
        // TODO set analysis and stext if we have anything
        let zero = Some("0".to_string());
        let fixed = [
            (BEGINANALYSIS, zero.clone()),
            (ENDANALYSIS, zero.clone()),
            (BEGINSTEXT, zero.clone()),
            (ENDSTEXT, zero.clone()),
            (MODE, Some(self.mode.to_string())),
            (BYTEORD, Some(self.byteord.to_string())),
            (CYT, self.cyt.as_opt_string()),
            (SPILLOVER, self.spillover.as_opt_string()),
            (BTIM, ts.btim.as_opt_string()),
            (ETIM, ts.etim.as_opt_string()),
            (DATE, ts.date.as_opt_string()),
            (CYTSN, self.cytsn.as_opt_string()),
            (TIMESTEP, self.timestep.as_opt_string()),
            (LAST_MODIFIER, mdn.last_modifier.as_opt_string()),
            (LAST_MODIFIED, mdn.last_modified.as_opt_string()),
            (ORIGINALITY, mdn.originality.as_opt_string()),
            (PLATEID, pl.plateid.as_opt_string()),
            (PLATENAME, pl.platename.as_opt_string()),
            (WELLID, pl.wellid.as_opt_string()),
            (VOL, self.vol.as_opt_string()),
        ];
        let text_len = sum_keywords(&fixed) + other_textlen;
        make_data_offset_keywords(text_len, data_len)
            .into_iter()
            .chain(fixed)
            .collect()
    }
}

impl VersionedMetadata for InnerMetadata3_2 {
    type P = InnerMeasurement3_2;
    type R = InnerReadData3_2;

    fn into_any_text(t: Box<StdText3_2>) -> AnyStdTEXT {
        AnyStdTEXT::FCS3_2(t)
    }

    fn get_byteord(&self) -> ByteOrd {
        ByteOrd::Endian(self.byteord)
    }

    fn build_int_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        total_events: usize,
    ) -> Option<IntParser> {
        build_int_parser_2_0(st, &ByteOrd::Endian(self.byteord), ps, total_events)
    }

    fn build_mixed_parser(
        &self,
        st: &mut DataParserState,
        ps: &[Measurement<Self::P>],
        dt: &AlphaNumType,
        total_events: usize,
    ) -> Option<Option<MixedParser>> {
        let endian = self.byteord;
        // first test if we have any PnDATATYPEs defined, if no then skip this
        // data parser entirely
        if ps
            .iter()
            .filter(|p| p.specific.datatype.as_ref().into_option().is_some())
            .count()
            == 0
        {
            return None;
        }
        let (pass, fail): (Vec<_>, Vec<_>) = ps
            .iter()
            .enumerate()
            .map(|(i, p)| {
                // TODO this range thing seems not necessary
                match (
                    p.specific.get_column_type(*dt),
                    p.specific.datatype.as_ref().into_option().is_some(),
                    p.range,
                    &p.bytes,
                ) {
                    (AlphaNumType::Ascii, _, _, Bytes::Fixed(bytes)) => {
                        Ok(MixedColumnType::Ascii(AsciiColumn {
                            width: *bytes,
                            data: vec![],
                        }))
                    }
                    (AlphaNumType::Single, _, _, Bytes::Fixed(4)) => Ok(MixedColumnType::Single(
                        f32::make_column_parser(endian, total_events),
                    )),
                    (AlphaNumType::Double, _, _, Bytes::Fixed(8)) => Ok(MixedColumnType::Double(
                        f64::make_column_parser(endian, total_events),
                    )),
                    (AlphaNumType::Integer, _, r, Bytes::Fixed(bytes)) => {
                        make_int_parser(*bytes, &r, &ByteOrd::Endian(self.byteord), total_events)
                            .map(MixedColumnType::Uint)
                    }
                    (dt, overridden, _, bytes) => {
                        let sdt = if overridden { "PnDATATYPE" } else { "DATATYPE" };
                        Err(vec![format!(
                            "{}={} but PnB={} for measurement {}",
                            sdt, dt, bytes, i
                        )])
                    }
                }
            })
            .partition_result();
        if fail.is_empty() {
            Some(Some(MixedParser {
                nrows: total_events,
                columns: pass,
            }))
        } else {
            for e in fail.into_iter().flatten() {
                st.push_meta_error(e);
            }
            None
        }
    }

    fn lookup_specific(
        st: &mut KwState,
        _: usize,
        names: &HashSet<&str>,
    ) -> Option<InnerMetadata3_2> {
        // Only L is allowed as of 3.2, so pull the value and check it if given.
        // The only thing we care about is that the value is valid, since we
        // don't need to use it anywhere.
        let _ = st.lookup_mode3_2();
        let maybe_byteord = st.lookup_endian();
        let maybe_cyt = st.lookup_cyt_req();
        if let (Some(byteord), Some(cyt)) = (maybe_byteord, maybe_cyt) {
            Some(InnerMetadata3_2 {
                byteord,
                cyt,
                spillover: st.lookup_spillover_checked(names),
                timestamps: st.lookup_timestamps3_1(true),
                cytsn: st.lookup_cytsn(),
                timestep: st.lookup_timestep_checked(names),
                modification: st.lookup_modification(),
                plate: st.lookup_plate(true),
                vol: st.lookup_vol(),
                carrier: st.lookup_carrier(),
                datetimes: st.lookup_datetimes(),
                unstained: st.lookup_unstained(names),
                flowrate: st.lookup_flowrate(),
            })
        } else {
            None
        }
    }

    fn keywords_inner(&self, other_textlen: usize, data_len: usize) -> MaybeKeywords {
        let mdn = &self.modification;
        let ts = &self.timestamps;
        let pl = &self.plate;
        let car = &self.carrier;
        let dt = &self.datetimes;
        let us = &self.unstained;
        // TODO set analysis and stext if we have anything
        // let zero = Some("0".to_string());
        let fixed = [
            // (BEGINANALYSIS, zero.clone()),
            // (ENDANALYSIS, zero.clone()),
            // (BEGINSTEXT, zero.clone()),
            // (ENDSTEXT, zero.clone()),
            (BYTEORD, Some(self.byteord.to_string())),
            (CYT, Some(self.cyt.to_string())),
            (SPILLOVER, self.spillover.as_opt_string()),
            (BTIM, ts.btim.as_opt_string()),
            (ETIM, ts.etim.as_opt_string()),
            (DATE, ts.date.as_opt_string()),
            (CYTSN, self.cytsn.as_opt_string()),
            (TIMESTEP, self.timestep.as_opt_string()),
            (LAST_MODIFIER, mdn.last_modifier.as_opt_string()),
            (LAST_MODIFIED, mdn.last_modified.as_opt_string()),
            (ORIGINALITY, mdn.originality.as_opt_string()),
            (PLATEID, pl.plateid.as_opt_string()),
            (PLATENAME, pl.platename.as_opt_string()),
            (WELLID, pl.wellid.as_opt_string()),
            (VOL, self.vol.as_opt_string()),
            (CARRIERID, car.carrierid.as_opt_string()),
            (CARRIERTYPE, car.carriertype.as_opt_string()),
            (LOCATIONID, car.locationid.as_opt_string()),
            (BEGINDATETIME, dt.begin.as_opt_string()),
            (ENDDATETIME, dt.end.as_opt_string()),
            (UNSTAINEDCENTERS, us.unstainedcenters.as_opt_string()),
            (UNSTAINEDINFO, us.unstainedinfo.as_opt_string()),
            (FLOWRATE, self.flowrate.as_opt_string()),
        ];
        let text_len = sum_keywords(&fixed) + other_textlen;
        make_data_offset_keywords(text_len, data_len)
            .into_iter()
            .chain(fixed)
            .collect()
    }
}

fn parse_raw_text(header: Header, raw: RawTEXT, conf: &StdTextReader) -> TEXTResult {
    match header.version {
        Version::FCS2_0 => StdText2_0::raw_to_std_text(header, raw, conf),
        Version::FCS3_0 => StdText3_0::raw_to_std_text(header, raw, conf),
        Version::FCS3_1 => StdText3_1::raw_to_std_text(header, raw, conf),
        Version::FCS3_2 => StdText3_2::raw_to_std_text(header, raw, conf),
    }
}

#[derive(Hash, Eq, PartialEq, Clone, Debug, Serialize)]
struct StdKey(String);

#[derive(Hash, Eq, PartialEq, Clone, Debug, Serialize)]
struct NonStdKey(String);

impl NonStdKey {
    fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

#[derive(Eq, PartialEq)]
enum ValueStatus {
    Raw,
    Error(String),
    Warning(String),
    Used,
}

struct KwValue {
    value: String,
    status: ValueStatus,
}

// all hail the almighty state monad :D

struct KwState<'a> {
    raw_standard_keywords: HashMap<StdKey, KwValue>,
    raw_nonstandard_keywords: HashMap<NonStdKey, String>,
    missing_keywords: Vec<StdKey>,
    deprecated_keys: Vec<StdKey>,
    deprecated_features: Vec<String>,
    meta_errors: Vec<String>,
    meta_warnings: Vec<String>,
    conf: &'a StdTextReader,
}

struct DataParserState<'a> {
    std_errors: StdTEXTErrors,
    conf: &'a StdTextReader,
}

#[derive(Debug, Clone)]
pub struct StdTEXTErrors {
    /// Required keywords that are missing
    missing_keywords: Vec<StdKey>,

    /// Errors that pertain to one keyword value
    keyword_errors: Vec<KeyError>,

    /// Errors involving multiple keywords, like PnB not matching DATATYPE
    meta_errors: Vec<String>,

    /// Nonstandard keys starting with "$". Error status depends on configuration.
    deviant_keywords: HashMap<StdKey, String>,

    /// Nonstandard keys. Error status depends on configuration.
    nonstandard_keywords: HashMap<NonStdKey, String>,

    /// Keywords that are deprecated. Error status depends on configuration.
    deprecated_keys: Vec<StdKey>,

    /// Features that are deprecated. Error status depends on configuration.
    deprecated_features: Vec<String>,

    /// Non-keyword warnings. Error status depends on configuration.
    meta_warnings: Vec<String>,

    /// Keyword warnings. Error status depends on configuration.
    keyword_warnings: Vec<KeyWarning>,
}

impl StdTEXTErrors {
    fn prune_errors(&mut self, conf: &StdTextReader) {
        if !conf.disallow_deviant {
            self.deviant_keywords.clear();
        };
        if !conf.disallow_nonstandard {
            self.nonstandard_keywords.clear();
        }
        if !conf.disallow_deprecated {
            self.deprecated_keys.clear();
            self.deprecated_features.clear();
        };
        if !conf.warnings_are_errors {
            self.meta_warnings.clear();
            self.keyword_warnings.clear();
        };
    }

    fn into_lines(self) -> Vec<String> {
        let ks = self
            .missing_keywords
            .into_iter()
            .map(|s| format!("Required keyword is missing: {}", s.0));
        let vs = self.keyword_errors.into_iter().map(|e| {
            format!(
                "Could not get value for {}. Error was '{}'. Value was '{}'.",
                e.key.0, e.msg, e.value
            )
        });
        // TODO add lots of other printing stuff here
        ks.chain(vs).chain(self.meta_errors).collect()
    }

    pub fn print(self) {
        for e in self.into_lines() {
            eprintln!("ERROR: {e}");
        }
    }
}

impl DataParserState<'_> {
    fn push_meta_error_str(&mut self, msg: &str) {
        self.push_meta_error(String::from(msg));
    }

    fn push_meta_error(&mut self, msg: String) {
        self.std_errors.meta_errors.push(msg);
    }

    fn push_meta_warning_str(&mut self, msg: &str) {
        self.push_meta_warning(String::from(msg));
    }

    fn push_meta_warning(&mut self, msg: String) {
        self.std_errors.meta_warnings.push(msg);
    }

    fn push_meta_deprecated_str(&mut self, msg: &str) {
        self.std_errors.deprecated_features.push(String::from(msg));
    }

    fn push_meta_error_or_warning(&mut self, is_error: bool, msg: String) {
        if is_error {
            self.std_errors.meta_errors.push(msg);
        } else {
            self.std_errors.meta_warnings.push(msg);
        }
    }

    fn into_errors(mut self) -> StdTEXTErrors {
        self.std_errors.prune_errors(self.conf);
        self.std_errors
    }

    fn into_result<M: VersionedMetadata>(
        self,
        it: IntermediateTEXT<M, <M as VersionedMetadata>::P, <M as VersionedMetadata>::R>,
        data_parser: DataParser,
        header: Header,
        raw: RawTEXT,
    ) -> TEXTResult {
        let mut s = self.std_errors;
        let c = &self.conf;
        let any_crit = !s.missing_keywords.is_empty()
            || !s.meta_errors.is_empty()
            || !s.keyword_errors.is_empty();
        let any_noncrit = (!s.deviant_keywords.is_empty() && c.disallow_deviant)
            || (!s.nonstandard_keywords.is_empty() && c.disallow_nonstandard)
            || (!(s.deprecated_features.is_empty() && s.deprecated_keys.is_empty())
                && c.disallow_deprecated)
            || (!(s.meta_warnings.is_empty() && s.keyword_warnings.is_empty())
                && c.warnings_are_errors);
        if any_crit || any_noncrit {
            s.prune_errors(c);
            // TODO this doesn't include nonstandard measurements, which is
            // probably fine, because if the user didn't want to include them
            // in the ns measurement field they wouldn't have used that param
            // anyways, in which case we probably need to call them something
            // different (like "upgradable")
            Err(Box::new(s))
        } else {
            let core = CoreText {
                metadata: it.metadata,
                measurements: it.measurements,
                nonstandard_keywords: s.nonstandard_keywords,
                deviant_keywords: s.deviant_keywords,
            };
            let std = StdText {
                core,
                data_offsets: it.data_offsets,
                read_data: it.read_data,
            };
            Ok(ParsedTEXT {
                standard: M::into_any_text(Box::new(std)),
                header,
                raw,
                data_parser,
                deprecated_keys: s.deprecated_keys,
                deprecated_features: s.deprecated_features,
                meta_warnings: s.meta_warnings,
                keyword_warnings: s.keyword_warnings,
            })
        }
    }
}

#[derive(Debug, Clone)]
struct KeyError {
    key: StdKey,
    value: String,
    msg: String,
}

#[derive(Debug, Clone, Serialize)]
struct KeyWarning {
    key: StdKey,
    value: String,
    msg: String,
}

impl<'a> KwState<'a> {
    // TODO not DRY (although will likely need HKTs)
    fn lookup_required<V: FromStr>(&mut self, k: &str, dep: bool) -> Option<V>
    where
        <V as FromStr>::Err: fmt::Display,
    {
        let sk = StdKey(String::from(k));
        match self.raw_standard_keywords.get_mut(&sk) {
            Some(v) => match v.status {
                ValueStatus::Raw => {
                    let (s, r) = v.value.parse().map_or_else(
                        |e| (ValueStatus::Error(format!("{}", e)), None),
                        |x| (ValueStatus::Used, Some(x)),
                    );
                    if dep {
                        self.deprecated_keys.push(sk);
                    }
                    v.status = s;
                    r
                }
                _ => None,
            },
            None => {
                self.missing_keywords.push(StdKey(String::from(k)));
                None
            }
        }
    }

    fn lookup_optional<V: FromStr>(&mut self, k: &str, dep: bool) -> OptionalKw<V>
    where
        <V as FromStr>::Err: fmt::Display,
    {
        let sk = StdKey(String::from(k));
        match self.raw_standard_keywords.get_mut(&sk) {
            Some(v) => match v.status {
                ValueStatus::Raw => {
                    let (s, r) = v.value.parse().map_or_else(
                        |w| (ValueStatus::Warning(format!("{}", w)), Absent),
                        |x| (ValueStatus::Used, OptionalKw::Present(x)),
                    );
                    if dep {
                        self.deprecated_keys.push(sk);
                    }
                    v.status = s;
                    r
                }
                _ => Absent,
            },
            None => Absent,
        }
    }

    fn build_offsets(&mut self, begin: u32, end: u32, id: SegmentId) -> Option<Segment> {
        match Segment::try_new(begin, end, id) {
            Ok(seg) => Some(seg),
            Err(err) => {
                self.meta_errors.push(err.to_string());
                None
            }
        }
    }

    // metadata

    fn lookup_begindata(&mut self) -> Option<u32> {
        self.lookup_required(BEGINDATA, false)
    }

    fn lookup_enddata(&mut self) -> Option<u32> {
        self.lookup_required(ENDDATA, false)
    }

    // TODO don't short circuit here
    fn lookup_data_offsets(&mut self) -> Option<Segment> {
        let begin = self.lookup_begindata()?;
        let end = self.lookup_enddata()?;
        self.build_offsets(begin, end, SegmentId::Data)
    }

    fn lookup_stext_offsets(&mut self) -> Option<Segment> {
        let beginstext = self.lookup_required(BEGINSTEXT, false)?;
        let endstext = self.lookup_required(ENDSTEXT, false)?;
        self.build_offsets(beginstext, endstext, SegmentId::SupplementalText)
    }

    fn lookup_analysis_offsets(&mut self) -> Option<Segment> {
        let beginstext = self.lookup_required(BEGINANALYSIS, false)?;
        let endstext = self.lookup_required(ENDANALYSIS, false)?;
        self.build_offsets(beginstext, endstext, SegmentId::Analysis)
    }

    fn lookup_supplemental3_0(&mut self) -> Option<SupplementalOffsets3_0> {
        let stext = self.lookup_stext_offsets()?;
        let analysis = self.lookup_analysis_offsets()?;
        Some(SupplementalOffsets3_0 { stext, analysis })
    }

    fn lookup_supplemental3_2(&mut self) -> SupplementalOffsets3_2 {
        let stext = OptionalKw::from_option(self.lookup_stext_offsets());
        let analysis = OptionalKw::from_option(self.lookup_analysis_offsets());
        SupplementalOffsets3_2 { stext, analysis }
    }

    fn lookup_byteord(&mut self) -> Option<ByteOrd> {
        self.lookup_required(BYTEORD, false)
    }

    fn lookup_endian(&mut self) -> Option<Endian> {
        self.lookup_required(BYTEORD, false)
    }

    fn lookup_datatype(&mut self) -> Option<AlphaNumType> {
        self.lookup_required(DATATYPE, false)
    }

    fn lookup_mode(&mut self) -> Option<Mode> {
        self.lookup_required(MODE, false)
    }

    fn lookup_mode3_2(&mut self) -> OptionalKw<Mode3_2> {
        self.lookup_optional(MODE, true)
    }

    fn lookup_nextdata(&mut self) -> Option<u32> {
        self.lookup_required(NEXTDATA, false)
    }

    fn lookup_par(&mut self) -> Option<usize> {
        self.lookup_required(PAR, false)
    }

    fn lookup_tot_req(&mut self) -> Option<u32> {
        self.lookup_required(TOT, false)
    }

    fn lookup_tot_opt(&mut self) -> OptionalKw<u32> {
        self.lookup_optional(TOT, false)
    }

    fn lookup_cyt_req(&mut self) -> Option<String> {
        self.lookup_required(CYT, false)
    }

    fn lookup_cyt_opt(&mut self) -> OptionalKw<String> {
        self.lookup_optional(CYT, false)
    }

    fn lookup_abrt(&mut self) -> OptionalKw<u32> {
        self.lookup_optional(ABRT, false)
    }

    fn lookup_cells(&mut self) -> OptionalKw<String> {
        self.lookup_optional(CELLS, false)
    }

    fn lookup_com(&mut self) -> OptionalKw<String> {
        self.lookup_optional(COM, false)
    }

    fn lookup_exp(&mut self) -> OptionalKw<String> {
        self.lookup_optional(EXP, false)
    }

    fn lookup_fil(&mut self) -> OptionalKw<String> {
        self.lookup_optional(FIL, false)
    }

    fn lookup_inst(&mut self) -> OptionalKw<String> {
        self.lookup_optional(INST, false)
    }

    fn lookup_lost(&mut self) -> OptionalKw<u32> {
        self.lookup_optional(LOST, false)
    }

    fn lookup_op(&mut self) -> OptionalKw<String> {
        self.lookup_optional(OP, false)
    }

    fn lookup_proj(&mut self) -> OptionalKw<String> {
        self.lookup_optional(PROJ, false)
    }

    fn lookup_smno(&mut self) -> OptionalKw<String> {
        self.lookup_optional(SMNO, false)
    }

    fn lookup_src(&mut self) -> OptionalKw<String> {
        self.lookup_optional(SRC, false)
    }

    fn lookup_sys(&mut self) -> OptionalKw<String> {
        self.lookup_optional(SYS, false)
    }

    fn lookup_trigger(&mut self) -> OptionalKw<Trigger> {
        self.lookup_optional(TR, false)
    }

    fn lookup_trigger_checked(&mut self, names: &HashSet<&str>) -> OptionalKw<Trigger> {
        if let Present(tr) = self.lookup_trigger() {
            let p = tr.measurement.as_str();
            if names.contains(p) {
                self.push_meta_error(format!(
                    "$TRIGGER refers to non-existent measurements '{p}'",
                ));
                Absent
            } else {
                Present(tr)
            }
        } else {
            Absent
        }
    }

    fn lookup_cytsn(&mut self) -> OptionalKw<String> {
        self.lookup_optional(CYTSN, false)
    }

    fn lookup_timestep(&mut self) -> OptionalKw<f32> {
        self.lookup_optional(TIMESTEP, false)
    }

    fn lookup_timestep_checked(&mut self, names: &HashSet<&str>) -> OptionalKw<f32> {
        let ts = self.lookup_timestep();
        if let Some(time_name) = &self.conf.time_shortname {
            if names.contains(time_name.as_str()) && ts == Absent {
                self.push_meta_error_or_warning(
                    self.conf.ensure_time_timestep,
                    String::from("$TIMESTEP must be present if time channel given"),
                )
            }
        }
        ts
    }

    fn lookup_vol(&mut self) -> OptionalKw<f32> {
        self.lookup_optional(VOL, false)
    }

    fn lookup_flowrate(&mut self) -> OptionalKw<String> {
        self.lookup_optional(FLOWRATE, false)
    }

    fn lookup_unicode(&mut self) -> OptionalKw<Unicode> {
        // TODO actually verify that these are real keywords, although this
        // doesn't matter too much since we are going to parse TEXT as utf8
        // anyways since we can, so this keywords isn't that useful.
        self.lookup_optional(UNICODE, false)
    }

    fn lookup_plateid(&mut self, dep: bool) -> OptionalKw<String> {
        self.lookup_optional(PLATEID, dep)
    }

    fn lookup_platename(&mut self, dep: bool) -> OptionalKw<String> {
        self.lookup_optional(PLATENAME, dep)
    }

    fn lookup_wellid(&mut self, dep: bool) -> OptionalKw<String> {
        self.lookup_optional(WELLID, dep)
    }

    fn lookup_unstainedinfo(&mut self) -> OptionalKw<String> {
        self.lookup_optional(UNSTAINEDINFO, false)
    }

    fn lookup_unstainedcenters(&mut self) -> OptionalKw<UnstainedCenters> {
        self.lookup_optional(UNSTAINEDCENTERS, false)
    }

    fn lookup_unstainedcenters_checked(
        &mut self,
        names: &HashSet<&str>,
    ) -> OptionalKw<UnstainedCenters> {
        if let Present(u) = self.lookup_unstainedcenters() {
            let noexist: Vec<_> = u.0.keys().filter(|m| !names.contains(m.as_str())).collect();
            if !noexist.is_empty() {
                let msg = format!(
                    "$UNSTAINEDCENTERS refers to non-existent measurements: {}",
                    noexist.iter().join(","),
                );
                self.push_meta_error(msg);
                Absent
            } else {
                Present(u)
            }
        } else {
            Absent
        }
    }

    fn lookup_last_modifier(&mut self) -> OptionalKw<String> {
        self.lookup_optional(LAST_MODIFIER, false)
    }

    fn lookup_last_modified(&mut self) -> OptionalKw<ModifiedDateTime> {
        self.lookup_optional(LAST_MODIFIED, false)
    }

    fn lookup_originality(&mut self) -> OptionalKw<Originality> {
        self.lookup_optional(ORIGINALITY, false)
    }

    fn lookup_carrierid(&mut self) -> OptionalKw<String> {
        self.lookup_optional(CARRIERID, false)
    }

    fn lookup_carriertype(&mut self) -> OptionalKw<String> {
        self.lookup_optional(CARRIERTYPE, false)
    }

    fn lookup_locationid(&mut self) -> OptionalKw<String> {
        self.lookup_optional(LOCATIONID, false)
    }

    fn lookup_begindatetime(&mut self) -> OptionalKw<FCSDateTime> {
        self.lookup_optional(BEGINDATETIME, false)
    }

    fn lookup_enddatetime(&mut self) -> OptionalKw<FCSDateTime> {
        self.lookup_optional(ENDDATETIME, false)
    }

    fn lookup_date(&mut self, dep: bool) -> OptionalKw<FCSDate> {
        self.lookup_optional(DATE, dep)
    }

    fn lookup_btim(&mut self) -> OptionalKw<FCSTime> {
        self.lookup_optional(BTIM, false)
    }

    fn lookup_etim(&mut self) -> OptionalKw<FCSTime> {
        self.lookup_optional(ETIM, false)
    }

    fn lookup_btim60(&mut self) -> OptionalKw<FCSTime60> {
        self.lookup_optional(BTIM, false)
    }

    fn lookup_etim60(&mut self) -> OptionalKw<FCSTime60> {
        self.lookup_optional(ETIM, false)
    }

    fn lookup_btim100(&mut self, dep: bool) -> OptionalKw<FCSTime100> {
        self.lookup_optional(BTIM, dep)
    }

    fn lookup_etim100(&mut self, dep: bool) -> OptionalKw<FCSTime100> {
        self.lookup_optional(ETIM, dep)
    }

    fn lookup_timestamps2_0(&mut self) -> Timestamps2_0 {
        Timestamps2_0 {
            btim: self.lookup_btim(),
            etim: self.lookup_etim(),
            date: self.lookup_date(false),
        }
    }

    fn lookup_timestamps3_0(&mut self) -> Timestamps3_0 {
        Timestamps3_0 {
            btim: self.lookup_btim60(),
            etim: self.lookup_etim60(),
            date: self.lookup_date(false),
        }
    }

    fn lookup_timestamps3_1(&mut self, dep: bool) -> Timestamps3_1 {
        Timestamps3_1 {
            btim: self.lookup_btim100(dep),
            etim: self.lookup_etim100(dep),
            date: self.lookup_date(dep),
        }
    }

    fn lookup_datetimes(&mut self) -> Datetimes {
        let begin = self.lookup_begindatetime();
        let end = self.lookup_enddatetime();
        // TODO make flag to enforce this as an error or warning
        if let (Present(b), Present(e)) = (&begin, &end) {
            if e.0 < b.0 {
                self.push_meta_warning_str("$BEGINDATETIME is after $ENDDATETIME");
            }
        }
        Datetimes { begin, end }
    }

    fn lookup_modification(&mut self) -> ModificationData {
        ModificationData {
            last_modifier: self.lookup_last_modifier(),
            last_modified: self.lookup_last_modified(),
            originality: self.lookup_originality(),
        }
    }

    fn lookup_plate(&mut self, dep: bool) -> PlateData {
        PlateData {
            wellid: self.lookup_plateid(dep),
            platename: self.lookup_platename(dep),
            plateid: self.lookup_wellid(dep),
        }
    }

    fn lookup_carrier(&mut self) -> CarrierData {
        CarrierData {
            locationid: self.lookup_locationid(),
            carrierid: self.lookup_carrierid(),
            carriertype: self.lookup_carriertype(),
        }
    }

    fn lookup_unstained(&mut self, names: &HashSet<&str>) -> UnstainedData {
        UnstainedData {
            unstainedcenters: self.lookup_unstainedcenters_checked(names),
            unstainedinfo: self.lookup_unstainedinfo(),
        }
    }

    fn lookup_compensation_2_0(&mut self, par: usize) -> OptionalKw<Compensation> {
        let mut matrix: Vec<_> = iter::repeat_with(|| vec![0.0; par]).take(par).collect();
        // column = src channel
        // row = target channel
        // These are "flipped" in 2.0, where "column" goes TO the "row"
        let mut any_error = false;
        for r in 0..par {
            for c in 0..par {
                let m = format!("DFC{c}TO{r}");
                if let Present(x) = self.lookup_optional(m.as_str(), false) {
                    matrix[r][c] = x;
                } else {
                    any_error = true;
                }
            }
        }
        if any_error {
            Absent
        } else {
            Present(Compensation { matrix })
        }
    }

    fn lookup_compensation_3_0(&mut self) -> OptionalKw<Compensation> {
        self.lookup_optional(COMP, false)
    }

    fn lookup_spillover(&mut self) -> OptionalKw<Spillover> {
        self.lookup_optional(SPILLOVER, false)
    }

    // TODO this is basically the same as unstained centers
    fn lookup_spillover_checked(&mut self, names: &HashSet<&str>) -> OptionalKw<Spillover> {
        if let Present(s) = self.lookup_spillover() {
            let noexist: Vec<_> = s
                .measurements
                .iter()
                .filter(|m| !names.contains(m.as_str()))
                .collect();
            if !noexist.is_empty() {
                let msg = format!(
                    "$SPILLOVER refers to non-existent measurements: {}",
                    noexist.iter().join(", ")
                );
                self.push_meta_error(msg);
            }

            Present(s)
        } else {
            Absent
        }
    }

    // measurements

    fn lookup_meas_req<V: FromStr>(&mut self, m: &'static str, n: usize, dep: bool) -> Option<V>
    where
        <V as FromStr>::Err: fmt::Display,
    {
        self.lookup_required(&format_measurement(&n.to_string(), m), dep)
    }

    fn lookup_meas_opt<V: FromStr>(&mut self, m: &'static str, n: usize, dep: bool) -> OptionalKw<V>
    where
        <V as FromStr>::Err: fmt::Display,
    {
        self.lookup_optional(&format_measurement(&n.to_string(), m), dep)
    }

    // this reads the PnB field which has "bits" in it, but as far as I know
    // nobody is using anything other than evenly-spaced bytes
    fn lookup_meas_bytes(&mut self, n: usize) -> Option<Bytes> {
        self.lookup_meas_req(BYTES_SFX, n, false)
    }

    fn lookup_meas_range(&mut self, n: usize) -> Option<Range> {
        self.lookup_meas_req(RANGE_SFX, n, false)
    }

    fn lookup_meas_wavelength(&mut self, n: usize) -> OptionalKw<u32> {
        self.lookup_meas_opt(WAVELEN_SFX, n, false)
    }

    fn lookup_meas_power(&mut self, n: usize) -> OptionalKw<u32> {
        self.lookup_meas_opt(POWER_SFX, n, false)
    }

    fn lookup_meas_detector_type(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(DET_TYPE_SFX, n, false)
    }

    fn lookup_meas_shortname_req(&mut self, n: usize) -> Option<Shortname> {
        self.lookup_meas_req(SHORTNAME_SFX, n, false)
    }

    fn lookup_meas_shortname_opt(&mut self, n: usize) -> OptionalKw<Shortname> {
        self.lookup_meas_opt(SHORTNAME_SFX, n, false)
    }

    fn lookup_meas_longname(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(LONGNAME_SFX, n, false)
    }

    fn lookup_meas_filter(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(FILTER_SFX, n, false)
    }

    fn lookup_meas_percent_emitted(&mut self, n: usize, dep: bool) -> OptionalKw<u32> {
        self.lookup_meas_opt(PCNT_EMT_SFX, n, dep)
    }

    fn lookup_meas_detector_voltage(&mut self, n: usize) -> OptionalKw<f32> {
        self.lookup_meas_opt(DET_VOLT_SFX, n, false)
    }

    fn lookup_meas_detector(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(DET_NAME_SFX, n, false)
    }

    fn lookup_meas_tag(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(TAG_SFX, n, false)
    }

    fn lookup_meas_analyte(&mut self, n: usize) -> OptionalKw<String> {
        self.lookup_meas_opt(ANALYTE_SFX, n, false)
    }

    fn lookup_meas_gain(&mut self, n: usize) -> OptionalKw<f32> {
        self.lookup_meas_opt(GAIN_SFX, n, false)
    }

    fn lookup_meas_gain_timecheck(&mut self, n: usize, name: &Shortname) -> OptionalKw<f32> {
        let gain = self.lookup_meas_gain(n);
        if let Present(g) = &gain {
            if self.conf.time_name_matches(name) && *g != 1.0 {
                if self.conf.ensure_time_nogain {
                    self.push_meta_error(String::from("Time channel must not have $PnG"));
                } else {
                    self.push_meta_warning(String::from(
                        "Time channel should not have $PnG, dropping $PnG",
                    ));
                }
                Absent
            } else {
                gain
            }
        } else {
            gain
        }
    }

    fn lookup_meas_gain_timecheck_opt(
        &mut self,
        n: usize,
        name: &OptionalKw<Shortname>,
    ) -> OptionalKw<f32> {
        if let Present(x) = name {
            self.lookup_meas_gain_timecheck(n, x)
        } else {
            self.lookup_meas_gain(n)
        }
    }

    fn lookup_meas_scale_req(&mut self, n: usize) -> Option<Scale> {
        self.lookup_meas_req(SCALE_SFX, n, false)
    }

    fn lookup_meas_scale_timecheck(&mut self, n: usize, name: &Shortname) -> Option<Scale> {
        let scale = self.lookup_meas_scale_req(n);
        if let Some(s) = &scale {
            if self.conf.time_name_matches(name)
                && *s != Scale::Linear
                && self.conf.ensure_time_linear
            {
                self.push_meta_error(String::from("Time channel must have linear $PnE"));
                None
            } else {
                scale
            }
        } else {
            scale
        }
    }

    fn lookup_meas_scale_timecheck_opt(
        &mut self,
        n: usize,
        name: &OptionalKw<Shortname>,
    ) -> Option<Scale> {
        if let Present(x) = name {
            self.lookup_meas_scale_timecheck(n, x)
        } else {
            self.lookup_meas_scale_req(n)
        }
    }

    fn lookup_meas_scale_opt(&mut self, n: usize) -> OptionalKw<Scale> {
        self.lookup_meas_opt(SCALE_SFX, n, false)
    }

    fn lookup_meas_calibration3_1(&mut self, n: usize) -> OptionalKw<Calibration3_1> {
        self.lookup_meas_opt(CALIBRATION_SFX, n, false)
    }

    fn lookup_meas_calibration3_2(&mut self, n: usize) -> OptionalKw<Calibration3_2> {
        self.lookup_meas_opt(CALIBRATION_SFX, n, false)
    }

    // for 3.1+ PnL measurements, which can have multiple wavelengths
    fn lookup_meas_wavelengths(&mut self, n: usize) -> OptionalKw<Wavelengths> {
        self.lookup_meas_opt(WAVELEN_SFX, n, false)
    }

    fn lookup_meas_display(&mut self, n: usize) -> OptionalKw<Display> {
        self.lookup_meas_opt(DISPLAY_SFX, n, false)
    }

    fn lookup_meas_datatype(&mut self, n: usize) -> OptionalKw<NumType> {
        self.lookup_meas_opt(DATATYPE_SFX, n, false)
    }

    fn lookup_meas_type(&mut self, n: usize) -> OptionalKw<MeasurementType> {
        self.lookup_meas_opt(DET_TYPE_SFX, n, false)
    }

    fn lookup_meas_feature(&mut self, n: usize) -> OptionalKw<Feature> {
        self.lookup_meas_opt(FEATURE_SFX, n, false)
    }

    /// Find nonstandard keys that a specific for a given measurement
    fn lookup_meas_nonstandard(&mut self, n: usize) -> HashMap<NonStdKey, String> {
        let mut ns = HashMap::new();
        // ASSUME the pattern does not start with "$" and has a %n which will be
        // subbed for the measurement index. The pattern will then be turned
        // into a legit rust regular expression, which may fail depending on
        // what %n does, so check it each time.
        if let Some(p) = &self.conf.nonstandard_measurement_pattern {
            let rep = p.replace("%n", n.to_string().as_str());
            if let Ok(pattern) = Regex::new(rep.as_str()) {
                for (k, v) in self.raw_nonstandard_keywords.iter() {
                    if pattern.is_match(k.as_str()) {
                        ns.insert(k.clone(), v.clone());
                    }
                }
            } else {
                self.push_meta_warning(format!(
                    "Could not make regular expression using \
                     pattern '{rep}' for measurement {n}"
                ));
            }
        }
        // TODO it seems like there should be a more efficient way to do this,
        // but the only ways I can think of involve taking ownership of the
        // keywords and then moving matching key/vals into a new hashlist.
        for k in ns.keys() {
            self.raw_nonstandard_keywords.remove(k);
        }
        ns
    }

    fn push_meta_error_str(&mut self, msg: &str) {
        self.push_meta_error(String::from(msg));
    }

    fn push_meta_error(&mut self, msg: String) {
        self.meta_errors.push(msg);
    }

    fn push_meta_warning_str(&mut self, msg: &str) {
        self.push_meta_warning(String::from(msg));
    }

    fn push_meta_warning(&mut self, msg: String) {
        self.meta_warnings.push(msg);
    }

    fn push_meta_deprecated_str(&mut self, msg: &str) {
        self.deprecated_features.push(String::from(msg));
    }

    fn push_meta_error_or_warning(&mut self, is_error: bool, msg: String) {
        if is_error {
            self.meta_errors.push(msg);
        } else {
            self.meta_warnings.push(msg);
        }
    }

    fn split_keywords(
        kws: HashMap<StdKey, KwValue>,
    ) -> (Vec<KeyError>, HashMap<StdKey, String>, Vec<KeyWarning>) {
        let mut deviant_keywords = HashMap::new();
        let mut keyword_warnings = Vec::new();
        let mut value_errors = Vec::new();
        for (key, v) in kws {
            match v.status {
                ValueStatus::Raw => {
                    deviant_keywords.insert(key, v.value);
                }
                ValueStatus::Warning(msg) => keyword_warnings.push(KeyWarning {
                    msg,
                    key,
                    value: v.value,
                }),
                ValueStatus::Error(msg) => value_errors.push(KeyError {
                    msg,
                    key,
                    value: v.value,
                }),
                ValueStatus::Used => (),
            }
        }
        (value_errors, deviant_keywords, keyword_warnings)
    }

    fn into_data_parser_state(self) -> DataParserState<'a> {
        let (keyword_errors, deviant_keywords, keyword_warnings) =
            Self::split_keywords(self.raw_standard_keywords);
        let std_errors = StdTEXTErrors {
            keyword_errors,
            keyword_warnings,
            deviant_keywords,
            nonstandard_keywords: self.raw_nonstandard_keywords,
            missing_keywords: self.missing_keywords,
            meta_errors: self.meta_errors,
            meta_warnings: self.meta_warnings,
            deprecated_keys: self.deprecated_keys,
            deprecated_features: self.deprecated_features,
        };
        DataParserState {
            std_errors,
            conf: self.conf,
        }
    }

    fn into_errors(self) -> StdTEXTErrors {
        let mut st = self.into_data_parser_state();
        st.std_errors.prune_errors(st.conf);
        st.std_errors
    }
}

fn parse_header_offset(s: &str, allow_blank: bool) -> Option<u32> {
    if allow_blank && s.trim().is_empty() {
        return Some(0);
    }
    let re = Regex::new(r" *(\d+)").unwrap();
    re.captures(s).map(|c| {
        let [i] = c.extract().1;
        i.parse().unwrap()
    })
}

fn parse_bounds(s0: &str, s1: &str, allow_blank: bool) -> Result<Segment, &'static str> {
    if let (Some(begin), Some(end)) = (
        parse_header_offset(s0, allow_blank),
        parse_header_offset(s1, allow_blank),
    ) {
        if begin > end {
            Err("beginning is greater than end")
        } else {
            Ok(Segment { begin, end })
        }
    } else if allow_blank {
        Err("could not make bounds from integers/blanks")
    } else {
        Err("could not make bounds from integers")
    }
}

const hre: &str = r"(.{6})    (.{8})(.{8})(.{8})(.{8})(.{8})(.{8})";

// TODO this error could be better
fn parse_header(s: &str) -> Result<Header, &'static str> {
    let re = Regex::new(hre).unwrap();
    re.captures(s)
        .and_then(|c| {
            let [v, t0, t1, d0, d1, a0, a1] = c.extract().1;
            if let (Ok(version), Ok(text), Ok(data), Ok(analysis)) = (
                v.parse(),
                parse_bounds(t0, t1, false),
                parse_bounds(d0, d1, false),
                parse_bounds(a0, a1, true),
            ) {
                Some(Header {
                    version,
                    text,
                    data,
                    analysis,
                })
            } else {
                None
            }
        })
        .ok_or("malformed header")
}

fn read_header<R: Read>(h: &mut BufReader<R>) -> io::Result<Header> {
    let mut verbuf = [0; 58];
    h.read_exact(&mut verbuf)?;
    if let Ok(hs) = str::from_utf8(&verbuf) {
        parse_header(hs).map_err(io::Error::other)
    } else {
        Err(io::Error::other("header sequence is not valid text"))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RawTEXT {
    delimiter: u8,
    standard_keywords: HashMap<StdKey, String>,
    nonstandard_keywords: HashMap<NonStdKey, String>,
    warnings: Vec<String>,
}

impl RawTEXT {
    fn to_state<'a>(&self, conf: &'a StdTextReader) -> KwState<'a> {
        let mut raw_standard_keywords = HashMap::new();
        for (k, v) in self.standard_keywords.iter() {
            raw_standard_keywords.insert(
                k.clone(),
                KwValue {
                    value: v.clone(),
                    status: ValueStatus::Raw,
                },
            );
        }
        KwState {
            raw_standard_keywords,
            raw_nonstandard_keywords: self.nonstandard_keywords.clone(),
            deprecated_keys: vec![],
            deprecated_features: vec![],
            missing_keywords: vec![],
            meta_errors: vec![],
            meta_warnings: vec![],
            conf,
        }
    }
}

struct FCSFile<M, P> {
    keywords: CoreText<M, P>,
    data: ParsedData,
}

type FCSFile2_0 = FCSFile<InnerMeasurement2_0, InnerMeasurement2_0>;
type FCSFile3_0 = FCSFile<InnerMeasurement3_0, InnerMeasurement3_0>;
type FCSFile3_1 = FCSFile<InnerMeasurement3_1, InnerMeasurement3_1>;
type FCSFile3_2 = FCSFile<InnerMeasurement3_2, InnerMeasurement3_2>;

pub struct FCSSuccess {
    pub header: Header,
    pub raw: RawTEXT,
    pub std: AnyStdTEXT,
    pub data: ParsedData,
}

// /// Represents result which may fail but still have immediately usable data.
// ///
// /// Useful for situations where the program should try to compute as much as
// /// possible before failing, which entails gather errors and carrying them
// /// forward rather than exiting immediately as would be the case if the error
// /// were wrapped in a Result<_, _>.
// struct Tentative<X> {
//     // ignorable: DeferredErrors,
//     // unignorable: DeferredErrors,
//     errors: AllDeferredErrors,
//     data: X,
// }

// struct AllDeferredErrors {
//     ignorable: DeferredErrors,
//     unignorable: DeferredErrors,
// }

// impl AllDeferredErrors {
//     fn new() -> AllDeferredErrors {
//         AllDeferredErrors {
//             ignorable: DeferredErrors::new(),
//             unignorable: DeferredErrors::new(),
//         }
//     }

//     fn from<E: Error + 'static>(e: E) -> AllDeferredErrors {
//         let mut x = AllDeferredErrors::new();
//         x.push_unignorable(e);
//         x
//     }

//     fn push_ignorable<E: Error + 'static>(&mut self, e: E) {
//         self.ignorable.0.push(Box::new(e))
//     }

//     fn push_unignorable<E: Error + 'static>(&mut self, e: E) {
//         self.unignorable.0.push(Box::new(e))
//     }

//     fn extend_ignorable(&mut self, es: DeferredErrors) {
//         self.ignorable.0.extend(es.0);
//     }

//     fn extend_unignorable(&mut self, es: DeferredErrors) {
//         self.unignorable.0.extend(es.0);
//     }

//     fn extend(&mut self, es: AllDeferredErrors) {
//         self.extend_unignorable(es.unignorable);
//         self.extend_ignorable(es.ignorable);
//     }
// }

#[derive(Copy, Clone)]
enum PureErrorLevel {
    Error,
    Warning,
    // TODO debug, info, etc
}

/// A pure error thrown during FCS file parsing.
///
/// This is very basic, since the only functionality we need is capturing a
/// message to show the user and an error level. The latter will dictate how the
/// error(s) is/are handled when we finish parsing.
struct PureError {
    msg: String,
    level: PureErrorLevel,
}

/// A collection of pure FCS errors.
///
/// Rather than exiting when we encounter the first error, we wish to capture
/// all possible errors and show the user all at once so they know what issues
/// in their files to fix. Therefore make an "error" type which is actually many
/// errors.
struct PureErrorBuf {
    errors: Vec<PureError>,
}

/// The result of a successful pure FCS computation which may have errors.
///
/// Since we are collecting errors and displaying them at the end of the parse
/// process, "success" needs to include any errors that have been previously
/// thrown (aka they are "deferred"). Decide later if these are a real issue and
/// parsed data needs to be withheld from the user.
struct PureSuccess<X> {
    deferred: PureErrorBuf,
    data: X,
}

/// The result of a failed computation.
///
/// This includes the immediate reason for failure as well as any errors
/// encountered previously which were deferred until now.
struct Failure<E> {
    reason: E,
    deferred: PureErrorBuf,
}

/// The result of a failed pure FCS computation.
type PureFailure = Failure<String>;

/// Success or failure of a pure FCS computation.
type PureResult<T> = Result<PureSuccess<T>, PureFailure>;

/// Error which may either be pure or impure (within IO context).
///
/// In the pure case this only has a single error rather than the collection
/// of errors. The pure case is meant to be used as the single reason for
/// a critical error; deferred errors will be captured elsewhere. Given that
/// this is only meant to be used in the failure case, pure errors do not have
/// an error level (they are always "critical").
///
/// The impure case is always "critical" as usually this indicates something
/// went wrong with file IO, which is usually an OS issue.
enum ImpureError {
    IO(io::Error),
    Pure(String),
}

/// The result of either a failed pure or impure computation.
type ImpureFailure = Failure<ImpureError>;

/// Success or failure of a pure or impure computation.
type ImpureResult<T> = Result<PureSuccess<T>, ImpureFailure>;

impl<E> Failure<E> {
    fn new(reason: E) -> Failure<E> {
        Failure {
            reason,
            deferred: PureErrorBuf::new(),
        }
    }

    fn map<X, F: Fn(E) -> X>(self, f: F) -> Failure<X> {
        Failure {
            reason: f(self.reason),
            deferred: self.deferred,
        }
    }

    fn from_result<X>(res: Result<X, E>) -> Result<X, Failure<E>> {
        res.map_err(Failure::new)
    }

    fn extend(&mut self, other: PureErrorBuf) {
        self.deferred.errors.extend(other.errors);
    }
}

impl PureErrorBuf {
    fn new() -> PureErrorBuf {
        PureErrorBuf { errors: vec![] }
    }

    fn from(msg: String, level: PureErrorLevel) -> PureErrorBuf {
        PureErrorBuf {
            errors: vec![PureError { msg, level }],
        }
    }

    fn concat(mut self, other: PureErrorBuf) -> PureErrorBuf {
        self.errors.extend(other.errors);
        PureErrorBuf {
            errors: self.errors,
        }
    }

    fn from_many(msgs: Vec<String>, level: PureErrorLevel) -> PureErrorBuf {
        PureErrorBuf {
            errors: msgs
                .into_iter()
                .map(|msg| PureError { msg, level })
                .collect(),
        }
    }
}

impl<X> PureSuccess<X> {
    fn from(data: X) -> PureSuccess<X> {
        PureSuccess {
            data,
            deferred: PureErrorBuf::new(),
        }
    }

    fn push(&mut self, e: PureError) {
        self.deferred.errors.push(e)
    }

    fn push_msg(&mut self, msg: String, level: PureErrorLevel) {
        self.push(PureError { msg, level })
    }

    fn push_msg_leveled(&mut self, msg: String, is_error: bool) {
        if is_error {
            self.push_error(msg);
        } else {
            self.push_warning(msg);
        }
    }

    fn push_error(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Error)
    }

    fn push_warning(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Warning)
    }

    fn extend(&mut self, es: PureErrorBuf) {
        self.deferred.errors.extend(es.errors)
    }

    fn map<Y, F: FnOnce(X) -> Y>(self, f: F) -> PureSuccess<Y> {
        let data = f(self.data);
        PureSuccess {
            data,
            deferred: self.deferred,
        }
    }

    fn into_result(self) -> PureResult<X> {
        Ok(self)
    }

    fn and_then<Y, F: FnOnce(X) -> PureSuccess<Y>>(self, f: F) -> PureSuccess<Y> {
        let mut new = f(self.data);
        // TODO order?
        new.extend(self.deferred);
        new
    }

    fn try_map<E, Y, F>(self, f: F) -> Result<PureSuccess<Y>, Failure<E>>
    where
        F: FnOnce(X) -> Result<PureSuccess<Y>, Failure<E>>,
    {
        match f(self.data) {
            Ok(mut new) => {
                new.deferred.errors.extend(self.deferred.errors);
                Ok(new)
            }
            Err(mut err) => {
                // TODO order?
                err.deferred.errors.extend(self.deferred.errors);
                Err(err)
            }
        }
    }

    fn combine<Y, Z, F: FnOnce(X, Y) -> Z>(self, other: PureSuccess<Y>, f: F) -> PureSuccess<Z> {
        PureSuccess {
            data: f(self.data, other.data),
            deferred: self.deferred.concat(other.deferred),
        }
    }

    fn combine_result<E, F, Y, Z>(
        self,
        other: Result<PureSuccess<Y>, Failure<E>>,
        f: F,
    ) -> Result<PureSuccess<Z>, Failure<E>>
    where
        F: FnOnce(X, Y) -> Z,
    {
        match other {
            Ok(pass) => Ok(self.combine(pass, f)),
            Err(mut fail) => {
                fail.extend(self.deferred);
                Err(fail)
            }
        }
    }

    fn combine_some_result<E, F, Y, Z>(
        self,
        other: Result<Y, Failure<E>>,
        f: F,
    ) -> Result<PureSuccess<Z>, Failure<E>>
    where
        F: FnOnce(X, Y) -> Z,
    {
        match other {
            Ok(pass) => Ok(PureSuccess {
                data: f(self.data, pass),
                deferred: self.deferred,
            }),
            Err(mut fail) => {
                fail.extend(self.deferred);
                Err(fail)
            }
        }
    }

    fn from_result(res: Result<X, PureErrorBuf>) -> PureSuccess<Option<X>> {
        match res {
            Ok(data) => PureSuccess::from(Some(data)),
            Err(deferred) => PureSuccess {
                data: None,
                deferred,
            },
        }
    }
}

#[derive(Debug)]
struct DelimError {
    delimiter: u8,
    kind: DelimErrorKind,
}

impl fmt::Display for DelimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        let x = match self.kind {
            DelimErrorKind::NotAscii => "an ASCII character 1-126",
            DelimErrorKind::NotUTF8 => "a utf8 character",
        };
        write!(f, "Delimiter {} is not {}", self.delimiter, x)
    }
}

impl Error for DelimError {}

#[derive(Debug)]
enum DelimErrorKind {
    NotUTF8,
    NotAscii,
}

fn verify_delim(xs: &[u8], conf: &RawTextReader) -> PureSuccess<u8> {
    // First character is the delimiter
    let delimiter: u8 = xs[0];

    // Check that it is a valid UTF8 character
    //
    // TODO we technically don't need this to be true in the case of double
    // delimiters, but this is non-standard anyways and probably rare
    let mut res = PureSuccess::from(delimiter);
    if String::from_utf8(vec![delimiter]).is_err() {
        res.push_error(format!(
            "Delimiter {delimiter} is not a valid utf8 character"
        ));
    }

    // Check that the delim is valid; this is technically only written in the
    // spec for 3.1+ but for older versions this should still be true since
    // these were ASCII-everywhere
    if !(1..=126).contains(&delimiter) {
        let msg = format!("Delimiter {delimiter} is not an ASCII character b/t 1-126");
        res.push_msg_leveled(msg, conf.force_ascii_delim);
    }
    res
}

enum RawTextError {
    DelimAtBoundary,
}

#[derive(Debug)]
struct MsgError(String);

impl Error for MsgError {}

impl fmt::Display for MsgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

type RawPairs = Vec<(String, String)>;

fn split_raw_text(xs: &[u8], delim: u8, conf: &RawTextReader) -> PureSuccess<RawPairs> {
    let mut keywords: vec![];
    let mut res = PureSuccess::from(keywords);
    let mut warnings = vec![];
    let textlen = xs.len();

    // Record delim positions
    let delim_positions: Vec<_> = xs
        .iter()
        .enumerate()
        .filter_map(|(i, c)| if *c == delim { Some(i) } else { None })
        .collect();

    // bail if we only have two positions
    if delim_positions.len() <= 2 {
        return res;
    }

    // Reduce position list to 'boundary list' which will be tuples of position
    // of a given delim and length until next delim.
    let raw_boundaries = delim_positions.windows(2).filter_map(|x| match x {
        [a, b] => Some((*a, b - a)),
        _ => None,
    });

    // Compute word boundaries depending on if we want to "escape" delims or
    // not. Technically all versions of the standard allow double delimiters to
    // be used in a word to represented a single delimiter. However, this means
    // we also can't have blank values. Many FCS files unfortunately use blank
    // values, so we need to be able to toggle this behavior.
    let boundaries = if conf.no_delim_escape {
        raw_boundaries.collect()
    } else {
        // Remove "escaped" delimiters from position vector. Because we disallow
        // blank values and also disallow delimiters at the start/end of words,
        // this implies that we should only see delimiters by themselves or in a
        // consecutive sequence whose length is even. Any odd-length'ed runs will
        // be treated as one delimiter if config permits
        let mut filtered_boundaries = vec![];
        for (key, chunk) in raw_boundaries.chunk_by(|(_, x)| *x).into_iter() {
            if key == 1 {
                if chunk.count() % 2 == 1 {
                    res.push_unignorable(RawTextError::DelimAtBoundary);
                }
            } else {
                for x in chunk {
                    filtered_boundaries.push(x);
                }
            }
        }

        // If all went well in the previous step, we should have the following:
        // 1. at least one boundary
        // 2. first entry coincides with start of TEXT
        // 3. last entry coincides with end of TEXT
        if let (Some((x0, _)), Some((xf, len))) =
            (filtered_boundaries.first(), filtered_boundaries.last())
        {
            if *x0 > 0 {
                let msg = format!("first key starts with a delim '{delim}'");
                res.push_error(msg);
            }
            if *xf + len < textlen - 1 {
                let msg = format!("final value ends with a delim '{delim}'");
                res.push_error(msg);
            }
        } else {
            return res;
        }
        filtered_boundaries
    };

    // Check that the last char is also a delim, if not file probably sketchy
    // ASSUME this will not fail since we have at least one delim by definition
    if !delim_positions.last().unwrap() == xs.len() - 1 {
        res.push_msg_leveled(
            "Last char is not a delimiter".to_string(),
            conf.enforce_final_delim,
        );
    }

    let delim2 = [delim, delim];
    let delim1 = [delim];
    // ASSUME these won't fail as we checked the delimiter is an ASCII character
    let escape_from = str::from_utf8(&delim2).unwrap();
    let escape_to = str::from_utf8(&delim1).unwrap();

    let final_boundaries: Vec<_> = boundaries
        .into_iter()
        .map(|(a, b)| (a + 1, a + b))
        .collect();

    for chunk in final_boundaries.chunks(2) {
        if let [(ki, kf), (vi, vf)] = *chunk {
            if let (Ok(k), Ok(v)) = (str::from_utf8(&xs[ki..kf]), str::from_utf8(&xs[vi..vf])) {
                let kupper = k.to_uppercase();
                // test if keyword is ascii
                if !kupper.is_ascii() {
                    // TODO actually include keyword here
                    res.push_msg_leveled(
                        "keywords must be ASCII".to_string(),
                        conf.enfore_keyword_ascii,
                    )
                }
                // if delimiters were escaped, replace them here
                if conf.no_delim_escape {
                    // Test for empty values if we don't allow delim escaping;
                    // anything empty will either drop or produce an error
                    // depending on user settings
                    if v.is_empty() {
                        // TODO tell the user that this key will be dropped
                        let msg = format!("key {kupper} has a blank value");
                        res.push_msg_leveled(msg, conf.enforce_nonempty);
                        None
                    } else {
                        keywords.push((kupper.clone(), v.to_string()))
                    }
                } else {
                    let krep = kupper.replace(escape_from, escape_to);
                    let rrep = v.replace(escape_from, escape_to);
                    keywords.push((krep, rrep))
                };
                // test if the key was inserted already
                //
                // TODO this will be better assessed when we have both hashmaps
                // from primary and supp text
                // if res.is_some() {
                //     let msg = format!("key {kupper} is found more than once");
                //     res.push_msg_leveled(msg, conf.enforce_unique)
                // }
            } else {
                let msg = "invalid UTF-8 byte encountered when parsing TEXT".to_string();
                res.push_msg_leveled(msg, conf.error_on_invalid_utf8)
            }
        } else {
            res.push_msg_leveled("number of words is not even".to_string(), conf.enforce_even)
        }
    }
    res
}

fn repair_keywords(pairs: &mut RawPairs, conf: &RawTextReader) {
    for (key, v) in pairs.iter_mut() {
        let k = key.as_str();
        if k == DATE {
            if let Some(pattern) = &conf.date_pattern {
                if let Ok(d) = NaiveDate::parse_from_str(v, pattern.as_str()) {
                    *v = format!("{}", FCSDate(d))
                }
            }
        }
    }
}

fn split_raw_pairs(
    pairs: Vec<(String, String)>,
    conf: &RawTextReader,
) -> PureSuccess<(HashMap<StdKey, String>, HashMap<NonStdKey, String>)> {
    let standard: HashMap<_, _> = HashMap::new();
    let nonstandard: HashMap<_, _> = HashMap::new();
    let mut res = PureSuccess::from((standard, nonstandard));
    // TODO filter keywords based on pattern somewhere here
    for (key, value) in pairs.into_iter() {
        let oldkey = key.clone(); // TODO this seems lame
        let ires = if key.starts_with("$") {
            res.data.0.insert(StdKey(key), value)
        } else {
            res.data.1.insert(NonStdKey(key), value)
        };
        if ires.is_some() {
            let msg = format!("Skipping already-inserted key: {oldkey}");
            res.push_msg_leveled(msg, conf.enforce_unique);
        }
    }
    res
}

impl Error for SegmentError {}

// macro_rules! wrap_enum {
//     ($enum_name:ident, $wrapper_name:ident) => {
//         impl From<$enum_name> for $wrapper_name {
//             fn from(enm: $enum_name) -> Self {
//                 Self(enm)
//             }
//         }
//     };
// }

impl From<PureFailure> for ImpureFailure {
    fn from(value: PureFailure) -> Self {
        value.map(ImpureError::Pure)
    }
}

impl From<io::Error> for ImpureFailure {
    fn from(value: io::Error) -> Self {
        Failure::new(ImpureError::IO(value))
    }
}

fn pad_zeros(s: &str) -> String {
    let len = s.len();
    let trimmed = s.trim_start();
    let newlen = trimmed.len();
    ("0").repeat(len - newlen) + trimmed
}

fn parse_segment(
    begin: Option<String>,
    end: Option<String>,
    begin_delta: i32,
    end_delta: i32,
    id: SegmentId,
    level: PureErrorLevel,
) -> Result<Segment, PureErrorBuf> {
    let parse_one = |s: Option<String>, which| {
        s.ok_or(format!("{which} not present for {id}"))
            .and_then(|pass| pass.parse::<u32>().map_err(|e| e.to_string()))
    };
    let b = parse_one(begin, "begin");
    let e = parse_one(end, "end");
    let res = match (b, e) {
        (Ok(bn), Ok(en)) => {
            Segment::try_new_adjusted(bn, en, begin_delta, end_delta, id).map_err(|e| vec![e])
        }
        (Err(be), Err(en)) => Err(vec![be, en]),
        (Err(be), _) => Err(vec![be]),
        (_, Err(en)) => Err(vec![en]),
    };
    res.map_err(|msgs| PureErrorBuf::from_many(msgs, level))
}

fn find_raw_segments(
    pairs: RawPairs,
    conf: &RawTextReader,
    header_data_seg: &Segment,
    header_analysis_seg: &Segment,
) -> (
    RawPairs,
    PureResult<Segment>,
    PureSuccess<Option<Segment>>,
    PureSuccess<Option<Segment>>,
) {
    // iterate through all pairs and strip out the ones that denote an offset
    let mut data0 = None;
    let mut data1 = None;
    let mut stext0 = None;
    let mut stext1 = None;
    let mut analysis0 = None;
    let mut analysis1 = None;
    let mut newpairs = vec![];
    let pad_maybe = |s: String| {
        if conf.repair_offset_spaces {
            pad_zeros(s.as_str())
        } else {
            s
        }
    };
    for (key, v) in pairs.into_iter() {
        match key.as_str() {
            BEGINDATA => data0 = Some(pad_maybe(v)),
            ENDDATA => data1 = Some(pad_maybe(v)),
            BEGINSTEXT => stext0 = Some(pad_maybe(v)),
            ENDSTEXT => stext1 = Some(pad_maybe(v)),
            BEGINANALYSIS => analysis0 = Some(pad_maybe(v)),
            ENDANALYSIS => analysis1 = Some(pad_maybe(v)),
            _ => newpairs.push((key, v)),
        }
    }
    // The DATA segment can be specified in either the header or TEXT. If within
    // offset 99,999,999, then the two should match. if they don't match then
    // trust the header and throw warning/error. If outside this range then the
    // header will be 0,0 and TEXT will have the real offsets.
    let data = parse_segment(
        data0,
        data1,
        conf.startdata_delta,
        conf.enddata_delta,
        SegmentId::Data,
        PureErrorLevel::Error,
    )
    .map(|data_seg| {
        let mut res = PureSuccess::from(data_seg);
        if !header_data_seg.is_unset() && data_seg != *header_data_seg {
            res.data = *header_data_seg;
            // TODO toggle level since this could indicate a sketchy file
            res.push_msg_leveled(
                "DATA offsets differ in HEADER and TEXT, using HEADER".to_string(),
                false,
            );
        }
        res
    })
    // TODO this seems like something I'll be doing alot
    .map_err(|deferred| Failure {
        reason: "DATA segment could not be found".to_string(),
        deferred,
    });

    // Supplemental TEXT offsets are only in TEXT, so just parse and return
    // if found.
    let stext = PureSuccess::from_result(parse_segment(
        stext0,
        stext1,
        conf.start_stext_delta,
        conf.end_stext_delta,
        SegmentId::SupplementalText,
        PureErrorLevel::Warning,
    ));

    // ANALYSIS offsets are analogous to DATA offsets except they are optional.
    let analysis = PureSuccess::from_result(parse_segment(
        analysis0,
        analysis1,
        conf.start_analysis_delta,
        conf.end_analysis_delta,
        SegmentId::Analysis,
        PureErrorLevel::Warning,
    ))
    .and_then(|anal_seg| {
        // TODO this doesn't seem the most efficient since I make a new
        // PureSuccess object in some branches
        match anal_seg {
            None => {
                let seg = if header_analysis_seg.is_unset() {
                    None
                } else {
                    Some(*header_analysis_seg)
                };
                PureSuccess::from(seg)
            }
            Some(seg) => {
                let mut res = PureSuccess::from(Some(seg));
                if !header_analysis_seg.is_unset() && seg != *header_analysis_seg {
                    res.data = Some(*header_analysis_seg);
                    // TODO toggle level since this could indicate a sketchy file
                    res.push_msg_leveled(
                        "ANALYSIS offsets differ in HEADER and TEXT, using HEADER".to_string(),
                        false,
                    );
                }
                res
            }
        }
    });

    (newpairs, data, stext, analysis)
}

struct RawTEXTBetter {
    standard: HashMap<StdKey, String>,
    nonstandard: HashMap<NonStdKey, String>,
    data_seg: Segment,
    analysis_seg: Option<Segment>,
    // not totally necessary
    delim: u8,
}

fn read_segment<R: Read + Seek>(
    h: &mut BufReader<R>,
    seg: &Segment,
    buf: &mut Vec<u8>,
) -> io::Result<()> {
    let begin = u64::from(seg.begin);
    let nbytes = u64::from(seg.num_bytes());

    h.seek(SeekFrom::Start(begin))?;
    h.take(nbytes).read_to_end(buf)?;
    Ok(())
}

fn read_raw_text<R: Read + Seek>(
    h: &mut BufReader<R>,
    header: &Header,
    conf: &RawTextReader,
) -> ImpureResult<RawTEXTBetter> {
    let adjusted_text = Failure::from_result(header.text.try_adjust(
        conf.starttext_delta,
        conf.endtext_delta,
        SegmentId::PrimaryText,
    ))?;

    let mut buf = vec![];
    read_segment(h, &adjusted_text, &mut buf)?;

    verify_delim(&buf, conf).try_map(|delim| {
        let mut res = split_raw_text(&buf, delim, conf);
        let pairs_res = if header.version == Version::FCS2_0 {
            repair_keywords(&mut res.data, conf);
            // TODO check that analysis is not blank (and DATA)
            Ok(res.map(|pairs| (pairs, header.data, Some(header.analysis.clone()))))
        } else {
            let (mut new_pairs, data_res, stext_res, anal_res) =
                find_raw_segments(res.data, conf, &header.data, &header.analysis);
            let stext_pairs_res = stext_res.try_map(|maybe_stext| {
                maybe_stext.map_or(Ok(PureSuccess::from(vec![])), |stext| {
                    buf.clear();
                    read_segment(h, &stext, &mut buf)?;
                    Ok(split_raw_text(&buf, delim, conf))
                })
            })?;
            stext_pairs_res
                .map(|stext_pairs| {
                    new_pairs.extend(stext_pairs);
                    new_pairs
                })
                .combine_result(data_res, |pairs, data_res| (pairs, data_res))
                .map(|pass| {
                    pass.combine(anal_res, |(pairs, data_seg), anal_seg| {
                        (pairs, data_seg, anal_seg)
                    })
                })
                .map_err(|err| err.map(ImpureError::Pure))
        }?;
        Ok(pairs_res.and_then(|(pairs, data_seg, analysis_seg)| {
            split_raw_pairs(pairs, conf).map(|(standard, nonstandard)| RawTEXTBetter {
                standard,
                nonstandard,
                data_seg,
                analysis_seg,
                delim,
            })
        }))
    })
}

/// Instructions for reading the TEXT segment as raw key/value pairs.
#[derive(Default, Clone)]
pub struct RawTextReader {
    /// Will adjust the offset of the start of the TEXT segment by `offset + n`.
    pub starttext_delta: i32,

    /// Will adjust the offset of the end of the TEXT segment by `offset + n`.
    pub endtext_delta: i32,

    pub startdata_delta: i32,
    pub enddata_delta: i32,

    pub start_stext_delta: i32,
    pub end_stext_delta: i32,

    pub start_analysis_delta: i32,
    pub end_analysis_delta: i32,

    /// If true, all raw text parsing warnings will be considered fatal errors
    /// which will halt the parsing routine.
    pub warnings_are_errors: bool,

    /// Will treat every delimiter as a literal delimiter rather than "escaping"
    /// double delimiters
    pub no_delim_escape: bool,

    /// If true, only ASCII characters 1-126 will be allowed for the delimiter
    pub force_ascii_delim: bool,

    /// If true, throw an error if the last byte of the TEXT segment is not
    /// a delimiter.
    pub enforce_final_delim: bool,

    /// If true, throw an error if any key in the TEXT segment is not unique
    pub enforce_unique: bool,

    /// If true, throw an error if the number or words in the TEXT segment is
    /// not an even number (ie there is a key with no value)
    pub enforce_even: bool,

    /// If true, throw an error if we encounter a key with a blank value.
    /// Only relevant if [`no_delim_escape`] is also true.
    pub enforce_nonempty: bool,

    /// If true, throw an error if the parser encounters a bad UTF-8 byte when
    /// creating the key/value list. If false, merely drop the bad pair.
    pub error_on_invalid_utf8: bool,

    /// If true, throw error when encoutering keyword with non-ASCII characters
    pub enfore_keyword_ascii: bool,

    /// If true, throw error when total event width does not evenly divide
    /// the DATA segment. Meaningless for delimited ASCII data.
    pub enfore_data_width_divisibility: bool,

    /// If true, throw error if the total number of events as computed by
    /// dividing DATA segment length event width doesn't match $TOT. Does
    /// nothing if $TOT not given, which may be the case in version 2.0.
    pub enfore_matching_tot: bool,

    /// If true, replace leading spaces in offset keywords with 0.
    ///
    ///These often need to be padded to make the DATA segment appear at a
    /// predictable offset. Many machines/programs will pad with spaces despite
    /// the spec requiring that all numeric fields be entirely numeric
    /// character.
    pub repair_offset_spaces: bool,

    /// If supplied, will be used as an alternative pattern when parsing $DATE.
    ///
    /// It should have specifiers for year, month, and day as outlined in
    /// https://docs.rs/chrono/latest/chrono/format/strftime/index.html. If not
    /// supplied, $DATE will be parsed according to the standard pattern which
    /// is '%d-%b-%Y'.
    pub date_pattern: Option<String>,
    // TODO add keyword and value overrides, something like a list of patterns
    // that can be used to alter each keyword
    // TODO allow lambda function to be supplied which will alter the kv list
}

/// Instructions for reading the TEXT segment in a standardized structure.
#[derive(Default, Clone)]
pub struct StdTextReader {
    pub raw: RawTextReader,

    /// If true, all metadata standardization warnings will be considered fatal
    /// errors which will halt the parsing routine.
    pub warnings_are_errors: bool,

    /// If given, will be the $PnN used to identify the time channel. Means
    /// nothing for 2.0.
    ///
    /// Will be used for the [`ensure_time*`] options below. If not given, skip
    /// time channel checking entirely.
    pub time_shortname: Option<String>,

    /// If true, will ensure that time channel is present
    pub ensure_time: bool,

    /// If true, will ensure TIMESTEP is present if time channel is also
    /// present.
    pub ensure_time_timestep: bool,

    /// If true, will ensure PnE is 0,0 for time channel.
    pub ensure_time_linear: bool,

    /// If true, will ensure PnG is absent for time channel.
    pub ensure_time_nogain: bool,

    /// If true, throw an error if TEXT includes any keywords that start with
    /// "$" which are not standard.
    pub disallow_deviant: bool,

    /// If true, throw an error if TEXT includes any deprecated features
    pub disallow_deprecated: bool,

    /// If true, throw an error if TEXT includes any keywords that do not
    /// start with "$".
    pub disallow_nonstandard: bool,

    /// If supplied, this pattern will be used to group "nonstandard" keywords
    /// with matching measurements.
    ///
    /// Usually this will be something like '^P%n.+' where '%n' will be
    /// substituted with the measurement index before using it as a regular
    /// expression to match keywords. It should not start with a "$".
    ///
    /// This will matching something like 'P7FOO' which would be 'FOO' for
    /// measurement 7. This might be useful in the future when this code offers
    /// "upgrade" routines since these are often used to represent future
    /// keywords in an older version where the newer version cannot be used for
    /// some reason.
    pub nonstandard_measurement_pattern: Option<String>,
    // TODO add repair stuff
}

impl StdTextReader {
    fn time_name_matches(&self, name: &Shortname) -> bool {
        self.time_shortname
            .as_ref()
            .map(|n| n == name.0.as_str())
            .unwrap_or(false)
    }
}

/// Instructions for reading the DATA segment.
#[derive(Default)]
pub struct DataReader {
    /// Will adjust the offset of the start of the TEXT segment by `offset + n`.
    datastart_delta: u32,
    /// Will adjust the offset of the end of the TEXT segment by `offset + n`.
    dataend_delta: u32,
}

/// Instructions for reading an FCS file.
#[derive(Default)]
pub struct Reader {
    pub text: StdTextReader,
    pub data: DataReader,
}

// type FCSResult = Result<FCSSuccess, Box<StdTEXTErrors>>;

/// Return header in an FCS file.
///
/// The header contains the version and offsets for the TEXT, DATA, and ANALYSIS
/// segments, all of which are present in fixed byte offset segments. This
/// function will fail and return an error if the file does not follow this
/// structure. Will also check that the begin and end segments are not reversed.
///
/// Depending on the version, all of these except the TEXT offsets might be 0
/// which indicates they are actually stored in TEXT due to size limitations.
pub fn read_fcs_header(p: &path::PathBuf) -> io::Result<Header> {
    let file = fs::File::options().read(true).open(p)?;
    let mut reader = BufReader::new(file);
    read_header(&mut reader)
}

/// Return header and raw key/value metadata pairs in an FCS file.
///
/// First will parse the header according to [`read_fcs_header`]. If this fails
/// an error will be returned.
///
/// Next will use the offset information in the header to parse the TEXT segment
/// for key/value pairs. On success will return these pairs as-is using Strings
/// in a HashMap. No other processing will be performed.
pub fn read_fcs_raw_text(p: &path::PathBuf, conf: &Reader) -> io::Result<(Header, RawTEXT)> {
    let file = fs::File::options().read(true).open(p)?;
    let mut reader = BufReader::new(file);
    let header = read_header(&mut reader)?;
    let raw = read_raw_text(&mut reader, &header, &conf.text.raw)?;
    Ok((header, raw))
}

/// Return header and standardized metadata in an FCS file.
///
/// Begins by parsing header and raw keywords according to [`read_fcs_raw_text`]
/// and will return error if this function fails.
///
/// Next, all keywords in the TEXT segment will be validated to conform to the
/// FCS standard indicated in the header and returned in a struct storing each
/// key/value pair in a standardized manner. This will halt and return any
/// errors encountered during this process.
pub fn read_fcs_text(p: &path::PathBuf, conf: &Reader) -> io::Result<TEXTResult> {
    let (header, raw) = read_fcs_raw_text(p, conf)?;
    Ok(parse_raw_text(header, raw, &conf.text))
}

// fn read_fcs_text_2_0(p: path::PathBuf, conf: StdTextReader) -> TEXTResult<TEXT2_0>;
// fn read_fcs_text_3_0(p: path::PathBuf, conf: StdTextReader) -> TEXTResult<TEXT3_0>;
// fn read_fcs_text_3_1(p: path::PathBuf, conf: StdTextReader) -> TEXTResult<TEXT3_1>;
// fn read_fcs_text_3_2(p: path::PathBuf, conf: StdTextReader) -> TEXTResult<TEXT3_2>;

/// Return header, structured metadata, and data in an FCS file.
///
/// Begins by parsing header and raw keywords according to [`read_fcs_text`]
/// and will return error if this function fails.
///
/// Next, the DATA segment will be parsed according to the metadata present
/// in TEXT.
///
/// On success will return all three of the above segments along with any
/// non-critical warnings.
///
/// The [`conf`] argument can be used to control the behavior of each reading
/// step, including the repair of non-conforming files.
pub fn read_fcs_file(p: &path::PathBuf, conf: &Reader) -> io::Result<PureResult> {
    let file = fs::File::options().read(true).open(p)?;
    let mut reader = BufReader::new(file);
    let header = read_header(&mut reader)?;
    let raw = read_raw_text(&mut reader, &header, &conf.text.raw)?;
    // TODO useless clone?
    match parse_raw_text(header, raw, &conf.text) {
        Ok(std) => {
            let data = read_data(&mut reader, std.data_parser).unwrap();
            Ok(Ok(PureSuccess {
                header: std.header,
                raw: std.raw,
                std: std.standard,
                data,
            }))
        }
        Err(e) => Ok(Err(e)),
    }
}

// fn read_fcs_file_2_0(p: path::PathBuf, conf: Reader) -> FCSResult<TEXT2_0>;
// fn read_fcs_file_3_0(p: path::PathBuf, conf: Reader) -> FCSResult<TEXT3_0>;
// fn read_fcs_file_3_1(p: path::PathBuf, conf: Reader) -> FCSResult<TEXT3_1>;
// fn read_fcs_file_3_2(p: path::PathBuf, conf: Reader) -> FCSResult<TEXT3_2>;

// /// Return header, raw metadata, and data in an FCS file.
// ///
// /// In contrast to [`read_fcs_file`], this will return the keywords as a flat
// /// list of key/value pairs. Only the bare minimum of these will be read in
// /// order to determine how to parse the DATA segment (including $DATATYPE,
// /// $BYTEORD, etc). No other checks will be performed to ensure the metadata
// /// conforms to the FCS standard version indicated in the header.
// ///
// /// This might be useful for applications where one does not necessarily need
// /// the strict structure of the standardized metadata, or if one does not care
// /// too much about the degree to which the metadata conforms to standard.
// ///
// /// Other than this, behavior is identical to [`read_fcs_file`],
// pub fn read_fcs_raw_file(p: path::PathBuf, conf: Reader) -> io::Result<FCSResult<()>> {
//     let file = fs::File::options().read(true).open(p)?;
//     let mut reader = BufReader::new(file);
//     let header = read_header(&mut reader)?;
//     let raw = read_raw_text(&mut reader, &header, &conf.text.raw)?;
//     // TODO need to modify this so it doesn't do the crazy version checking
//     // stuff we don't actually want in this case
//     match parse_raw_text(header.clone(), raw.clone(), &conf.text) {
//         Ok(std) => {
//             let data = read_data(&mut reader, std.data_parser).unwrap();
//             Ok(Ok(FCSSuccess {
//                 header,
//                 raw,
//                 std: (),
//                 data,
//             }))
//         }
//         Err(e) => Ok(Err(e)),
//     }
// }
