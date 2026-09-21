//! Pure-Rust 側のドメイン層。
//!
//! JV-Data の raw record bytes (Shift_JIS 固定長) を解析し、
//! 正規化された `jv.*` 行に変換する。I/O・COM・DB には一切依存しない。

pub mod dataspec;
pub mod error;
pub mod field;
pub mod model;
pub mod parse;
pub mod plan;
pub mod record;

pub use error::ParseError;
pub use record::{ParsedRecord, RecordHead};
