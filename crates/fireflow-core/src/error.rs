use std::io;

#[derive(Copy, Clone, Eq, PartialEq)]
pub enum PureErrorLevel {
    Error,
    Warning,
    // TODO debug, info, etc
}

/// A pure error thrown during FCS file parsing.
///
/// This is very basic, since the only functionality we need is capturing a
/// message to show the user and an error level. The latter will dictate how the
/// error(s) is/are handled when we finish parsing.
#[derive(Eq, PartialEq)]
pub struct PureError {
    pub msg: String,
    pub level: PureErrorLevel,
}

/// A collection of pure FCS errors.
///
/// Rather than exiting when we encounter the first error, we wish to capture
/// all possible errors and show the user all at once so they know what issues
/// in their files to fix. Therefore make an "error" type which is actually many
/// errors.
#[derive(Default)]
pub struct PureErrorBuf {
    pub errors: Vec<PureError>,
}

/// The result of a successful pure FCS computation which may have errors.
///
/// Since we are collecting errors and displaying them at the end of the parse
/// process, "success" needs to include any errors that have been previously
/// thrown (aka they are "deferred"). Decide later if these are a real issue and
/// parsed data needs to be withheld from the user.
pub struct PureSuccess<X> {
    pub deferred: PureErrorBuf,
    pub data: X,
}

/// The result of a failed computation.
///
/// This includes the immediate reason for failure as well as any errors
/// encountered previously which were deferred until now.
pub struct Failure<E> {
    pub reason: E,
    pub deferred: PureErrorBuf,
}

/// The result of a failed pure FCS computation.
pub type PureFailure = Failure<String>;

/// Success or failure of a pure FCS computation.
pub type PureResult<T> = Result<PureSuccess<T>, PureFailure>;

/// Result of a computation which may have failed but does not require
/// executation to be immediately terminated.
pub type PureMaybe<T> = PureSuccess<Option<T>>;

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
pub enum ImpureError {
    IO(io::Error),
    Pure(String),
}

/// The result of either a failed pure or impure computation.
pub type ImpureFailure = Failure<ImpureError>;

/// Success or failure of a pure or impure computation.
pub type ImpureResult<T> = Result<PureSuccess<T>, ImpureFailure>;

impl PureError {
    pub fn new_error(msg: String) -> Self {
        Self {
            msg,
            level: PureErrorLevel::Error,
        }
    }

    pub fn new_warning(msg: String) -> Self {
        Self {
            msg,
            level: PureErrorLevel::Warning,
        }
    }

    pub fn new(msg: String, is_error: bool) -> Self {
        if is_error {
            Self::new_error(msg)
        } else {
            Self::new_warning(msg)
        }
    }
}

impl<E> Failure<E> {
    pub fn new(reason: E) -> Failure<E> {
        Failure {
            reason,
            deferred: PureErrorBuf::default(),
        }
    }

    pub fn from_many(reason: E, deferred: PureErrorBuf) -> Self {
        Failure { reason, deferred }
    }

    pub fn from_many_msgs(reason: E, msgs: Vec<String>, level: PureErrorLevel) -> Self {
        Self::from_many(reason, PureErrorBuf::from_many(msgs, level))
    }

    pub fn from_many_errors(reason: E, msgs: Vec<String>) -> Self {
        Self::from_many_msgs(reason, msgs, PureErrorLevel::Error)
    }

    pub fn map<X, F: Fn(E) -> X>(self, f: F) -> Failure<X> {
        Failure {
            reason: f(self.reason),
            deferred: self.deferred,
        }
    }

    pub fn from_result<X>(res: Result<X, E>) -> Result<X, Failure<E>> {
        res.map_err(Failure::new)
    }

    pub fn extend(&mut self, other: PureErrorBuf) {
        self.deferred.errors.extend(other.errors);
    }
}

impl PureErrorBuf {
    pub fn from(msg: String, level: PureErrorLevel) -> Self {
        PureErrorBuf {
            errors: vec![PureError { msg, level }],
        }
    }

    pub fn concat(&mut self, other: Self) {
        self.errors.extend(other.errors)
    }

    pub fn chain(self, other: Self) -> Self {
        PureErrorBuf {
            errors: self.errors.into_iter().chain(other.errors).collect(),
        }
    }

    pub fn from_many(msgs: Vec<String>, level: PureErrorLevel) -> PureErrorBuf {
        PureErrorBuf {
            errors: msgs
                .into_iter()
                .map(|msg| PureError { msg, level })
                .collect(),
        }
    }

    pub fn push(&mut self, e: PureError) {
        self.errors.push(e)
    }

    // TODO not DRY
    pub fn push_msg(&mut self, msg: String, level: PureErrorLevel) {
        self.push(PureError { msg, level })
    }

    pub fn push_msg_leveled(&mut self, msg: String, is_error: bool) {
        if is_error {
            self.push_error(msg);
        } else {
            self.push_warning(msg);
        }
    }

    pub fn push_error(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Error)
    }

    pub fn push_warning(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Warning)
    }

    pub fn has_errors(&self) -> bool {
        self.errors
            .iter()
            .filter(|e| e.level == PureErrorLevel::Error)
            .count()
            > 0
    }

    pub fn split(self) -> (Vec<String>, Vec<String>) {
        let (err, warn): (Vec<_>, Vec<_>) = self
            .errors
            .into_iter()
            .partition(|e| e.level == PureErrorLevel::Error);
        (
            err.into_iter().map(|e| e.msg).collect(),
            warn.into_iter().map(|e| e.msg).collect(),
        )
    }

    pub fn into_errors(self) -> Vec<String> {
        self.into_level(PureErrorLevel::Error)
    }

    pub fn into_warnings(self) -> Vec<String> {
        self.into_level(PureErrorLevel::Warning)
    }

    fn into_level(self, level: PureErrorLevel) -> Vec<String> {
        self.errors
            .into_iter()
            .filter(|e| e.level == level)
            .map(|e| e.msg)
            .collect()
    }

    pub fn mconcat(xs: Vec<Self>) -> Self {
        let errors = xs.into_iter().fold(vec![], |mut acc, mut next| {
            acc.append(&mut next.errors);
            acc
        });
        Self { errors }
    }
}

impl<X> From<X> for PureSuccess<X> {
    fn from(data: X) -> Self {
        PureSuccess {
            data,
            deferred: PureErrorBuf::default(),
        }
    }
}

impl<X> PureSuccess<X> {
    pub fn push(&mut self, e: PureError) {
        self.deferred.errors.push(e)
    }

    pub fn push_msg(&mut self, msg: String, level: PureErrorLevel) {
        self.push(PureError { msg, level })
    }

    pub fn push_msg_leveled(&mut self, msg: String, is_error: bool) {
        if is_error {
            self.push_error(msg);
        } else {
            self.push_warning(msg);
        }
    }

    pub fn push_error(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Error)
    }

    pub fn push_warning(&mut self, msg: String) {
        self.push_msg(msg, PureErrorLevel::Warning)
    }

    pub fn extend(&mut self, es: PureErrorBuf) {
        self.deferred.errors.extend(es.errors)
    }

    pub fn map<Y, F: FnOnce(X) -> Y>(self, f: F) -> PureSuccess<Y> {
        let data = f(self.data);
        PureSuccess {
            data,
            deferred: self.deferred,
        }
    }

    pub fn and_then<Y, F: FnOnce(X) -> PureSuccess<Y>>(self, f: F) -> PureSuccess<Y> {
        let mut new = f(self.data);
        // TODO order?
        new.extend(self.deferred);
        new
    }

    pub fn try_map<E, Y, F>(self, f: F) -> Result<PureSuccess<Y>, Failure<E>>
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

    pub fn combine<Y, Z, F: FnOnce(X, Y) -> Z>(
        self,
        other: PureSuccess<Y>,
        f: F,
    ) -> PureSuccess<Z> {
        PureSuccess {
            data: f(self.data, other.data),
            deferred: self.deferred.chain(other.deferred),
        }
    }

    pub fn sequence(xs: Vec<PureSuccess<X>>) -> PureSuccess<Vec<X>> {
        let (data, es): (Vec<_>, Vec<_>) = xs.into_iter().map(|x| (x.data, x.deferred)).unzip();
        PureSuccess {
            data,
            deferred: PureErrorBuf::mconcat(es),
        }
    }

    pub fn combine3<A, B, Y, F: FnOnce(X, A, B) -> Y>(
        self,
        a: PureSuccess<A>,
        b: PureSuccess<B>,
        f: F,
    ) -> PureSuccess<Y> {
        PureSuccess {
            data: f(self.data, a.data, b.data),
            deferred: self.deferred.chain(a.deferred).chain(b.deferred),
        }
    }

    pub fn combine4<A, B, C, Y, F: FnOnce(X, A, B, C) -> Y>(
        self,
        a: PureSuccess<A>,
        b: PureSuccess<B>,
        c: PureSuccess<C>,
        f: F,
    ) -> PureSuccess<Y> {
        PureSuccess {
            data: f(self.data, a.data, b.data, c.data),
            deferred: self
                .deferred
                .chain(a.deferred)
                .chain(b.deferred)
                .chain(c.deferred),
        }
    }

    pub fn combine_result<E, F, Y, Z>(
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

    pub fn combine_some_result<E, F, Y, Z>(
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
}

impl<X> PureMaybe<X> {
    pub fn empty() -> PureMaybe<X> {
        PureSuccess::from(None)
    }

    pub fn map_maybe<Y, F: FnOnce(X) -> Y>(self, f: F) -> PureMaybe<Y> {
        self.map(|x| x.map(f))
    }

    pub fn into_result(self, reason: String) -> PureResult<X> {
        if let Some(d) = self.data {
            Ok(PureSuccess {
                data: d,
                deferred: self.deferred,
            })
        } else {
            Err(PureFailure {
                reason,
                deferred: self.deferred,
            })
        }
    }

    pub fn from_result_1(res: Result<X, String>, level: PureErrorLevel) -> Self {
        match res {
            Ok(data) => PureSuccess::from(Some(data)),
            Err(msg) => PureSuccess {
                data: None,
                deferred: PureErrorBuf::from(msg, level),
            },
        }
    }

    pub fn from_result(res: Result<X, PureErrorBuf>) -> Self {
        match res {
            Ok(data) => PureSuccess::from(Some(data)),
            Err(deferred) => PureSuccess {
                data: None,
                deferred,
            },
        }
    }

    pub fn from_result_strs(res: Result<X, Vec<String>>, level: PureErrorLevel) -> Self {
        match res {
            Ok(data) => PureSuccess::from(Some(data)),
            Err(msgs) => PureSuccess {
                data: None,
                deferred: PureErrorBuf::from_many(msgs, level),
            },
        }
    }

    pub fn from_result_errors(res: Result<X, Vec<String>>) -> Self {
        Self::from_result_strs(res, PureErrorLevel::Error)
    }

    pub fn and_then_opt<Y, F: FnOnce(X) -> PureMaybe<Y>>(self, f: F) -> PureMaybe<Y> {
        match self.data {
            Some(d) => {
                let mut new = f(d);
                // TODO order?
                new.extend(self.deferred);
                new
            }
            None => PureSuccess {
                data: None,
                deferred: self.deferred,
            },
        }
    }
}

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
