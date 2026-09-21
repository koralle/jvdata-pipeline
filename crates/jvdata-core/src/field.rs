//! 固定長フィールドの切り出しヘルパー。
//!
//! JV-Data は全角 Shift_JIS / 半角 JIS8 のバイト列。フィールドは必ず
//! byte offset で切る (String の char index と混同しない)。

use crate::ParseError;
use encoding_rs::SHIFT_JIS;
use time::Date;
use time::macros::format_description;

const DATE_FMT: &[time::format_description::FormatItem<'static>] =
    format_description!("[year][month][day]");

/// raw フィールドスライス。範囲外は TooShort。
pub fn raw(d: &[u8], off: usize, len: usize) -> Result<&[u8], ParseError> {
    d.get(off..off + len).ok_or(ParseError::TooShort {
        need: off + len,
        got: d.len(),
    })
}

/// Shift_JIS テキストを trim して String 化。末尾の NUL/空白は落とす。
pub fn text(d: &[u8], off: usize, len: usize) -> Result<String, ParseError> {
    let s = raw(d, off, len)?;
    let (cow, _, _) = SHIFT_JIS.decode(s);
    Ok(cow.trim_end_matches(['\0', ' ']).to_string())
}

/// text の空文字を None にした版。
pub fn text_opt(d: &[u8], off: usize, len: usize) -> Result<Option<String>, ParseError> {
    let s = text(d, off, len)?;
    Ok((!s.is_empty()).then_some(s))
}

/// ASCII 数字列。空白・全ゼロは None。JV-Data の数値は符号なし 0 埋め。
pub fn num(d: &[u8], off: usize, len: usize) -> Result<Option<i64>, ParseError> {
    let s = raw(d, off, len)?;
    let t = core::str::from_utf8(s)
        .map_err(|_| ParseError::InvalidNumeric {
            offset: off,
            raw: String::from_utf8_lossy(s).into_owned(),
        })?
        .trim_matches(['\0', ' ']);
    if t.is_empty() || t.bytes().all(|b| b == b'0') {
        return Ok(None);
    }
    if !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ParseError::InvalidNumeric {
            offset: off,
            raw: t.to_string(),
        });
    }
    t.parse::<i64>()
        .map(Some)
        .map_err(|_| ParseError::InvalidNumeric {
            offset: off,
            raw: t.to_string(),
        })
}

/// ×10 スケールの整数フィールド → 実数値。
/// 負担重量 (0.1kg 単位)・オッズ (0.1 倍単位)・ハロンタイム ("99秒9") 用。
/// 空・全ゼロは None。
pub fn scaled10(d: &[u8], off: usize, len: usize) -> Result<Option<f64>, ParseError> {
    Ok(num(d, off, len)?.map(|v| v as f64 / 10.0))
}

/// `9分99秒9` (分.秒.1/10秒 の packed 形式、4 bytes) → 秒。
/// 空・全ゼロは None (取消・中止等でタイム無し)。
/// 例: "1335" → 1分33秒5 = 93.5 秒。秒部が 60 以上はフォーマット違反。
pub fn packed_time(d: &[u8], off: usize) -> Result<Option<f64>, ParseError> {
    let Some(v) = num(d, off, 4)? else {
        return Ok(None);
    };
    let (min, sst) = (v / 1000, v % 1000);
    if sst >= 600 {
        return Err(ParseError::InvalidNumeric {
            offset: off,
            raw: format!("{v:04}"),
        });
    }
    // 全体を 0.1 秒単位の整数 (分×600 + 秒.1/10) にしてから 1 回だけ
    // 割ると、値は "X.Y" リテラルと同じ最近傍 double に正確に丸まる。
    Ok(Some((min * 600 + sst) as f64 / 10.0))
}

/// `YYYYMMDD` (8 bytes) → Date。空・ゼロ埋めは None。
pub fn date(d: &[u8], off: usize) -> Result<Option<Date>, ParseError> {
    let s = raw(d, off, 8)?;
    let t = core::str::from_utf8(s)
        .map_err(|_| ParseError::InvalidDate {
            offset: off,
            raw: String::from_utf8_lossy(s).into_owned(),
        })?
        .trim_matches(['\0', ' ']);
    if t.is_empty() || t.bytes().all(|b| b == b'0') {
        return Ok(None);
    }
    Date::parse(t, DATE_FMT)
        .map(Some)
        .map_err(|_| ParseError::InvalidDate {
            offset: off,
            raw: t.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_time_decodes_minute_second_tenth() {
        // "9分99秒9": "1335" = 1'33"5 = 93.5 秒 (0.1秒生値の 133.5 ではない)
        assert_eq!(packed_time(b"1335", 0).ok().flatten(), Some(93.5));
        assert_eq!(packed_time(b"0549", 0).ok().flatten(), Some(54.9));
        assert_eq!(packed_time(b"2243", 0).ok().flatten(), Some(144.3));
        // 全ゼロ・空白はタイム無し
        assert_eq!(packed_time(b"0000", 0).ok().flatten(), None);
        assert_eq!(packed_time(b"    ", 0).ok().flatten(), None);
    }

    #[test]
    fn packed_time_rejects_invalid_seconds() {
        // 秒部が 60 以上は "9分99秒9" のフォーマット違反
        assert!(packed_time(b"0965", 0).is_err()); // 0'96"5
        assert!(packed_time(b"1600", 0).is_err()); // 1'60"0
        assert!(packed_time(b"99a9", 0).is_err()); // 数字以外
        assert!(packed_time(b"12", 0).is_err()); // 4 bytes 未満
    }

    #[test]
    fn scaled10_decodes_tenth_scaled_fields() {
        assert_eq!(scaled10(b"570", 0, 3).ok().flatten(), Some(57.0));
        assert_eq!(scaled10(b"000", 0, 3).ok().flatten(), None);
        assert!(scaled10(b"12a", 0, 3).is_err());
    }
}
