//! JV-Link bridge 層。
//!
//! JV-Link (Windows COM) と pure Rust の間の境界。
//! このクレートは JV-Data のデータモデルを知らない。やりとりするのは
//! 「ファイル名つきの raw bytes の列」だけ。
//!
//! - [`protocol`][]: jvlink-bridge.exe (wine 内) ↔ ホスト側の frame codec
//! - [`source`][]:   ホスト側の取得インターフェース (process spawn / fixture)

pub mod fixture;
pub mod protocol;
pub mod source;
