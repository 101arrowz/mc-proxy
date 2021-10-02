use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum Error {
    #[error("unexpected end of input")]
    UnexpectedEOF,
    #[error("output buffer too small")]
    NeedMore,
    #[error("malformed data")]
    Malformed,
    #[error("unknown error")]
    Unknown
}
