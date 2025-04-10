use serde::Serialize;
use std::fmt;
use std::str::FromStr;

/// The value for the $PnN key (all versions).
///
/// This cannot contain commas.
#[derive(Debug, Clone, Serialize, Eq, PartialEq, Hash)]
pub struct Shortname(String);

impl fmt::Display for Shortname {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for Shortname {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl Shortname {
    pub fn new_unchecked<T: AsRef<str>>(s: T) -> Self {
        Shortname(s.as_ref().to_owned())
    }

    pub fn from_index(n: usize) -> Self {
        Shortname(format!("M{n}"))
    }
}

impl FromStr for Shortname {
    type Err = ShortnameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.contains(',') {
            Err(ShortnameError(s.to_string()))
        } else {
            Ok(Shortname(s.to_string()))
        }
    }
}

pub struct ShortnameError(String);

impl fmt::Display for ShortnameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "commas are not allowed in name '{}'", self.0)
    }
}
