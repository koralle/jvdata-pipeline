//! JV-Link のデータ種別 ID と取得モード。
//!
//! 値は JRA-VAN Data Lab. SDK インターフェース仕様書 (JV-Link4901.pdf)
//! 4.2.2 JVOpen に基づく。

use std::fmt;
use std::str::FromStr;

/// 蓄積系データ種別 (JVOpen option=1,3,4 で取得)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dataspec {
    /// 特別登録馬 (TK)
    Toku,
    /// レース詳細・馬毎・払戻・票数・オッズ・調教師等変更・除外馬 (RA,SE,HR,H1,H6,O1..O6,WF,JG)
    Race,
    /// マスタ差分 (UM,KS,CH,BR,BN,RC + 地方/海外 RA,SE)
    Diff,
    /// マスタ差分 (DIFF の SNPC 利用向け)
    Difn,
    /// 血統 (HN,SK,BT + 地方/海外 RA,SE)
    Blod,
    /// 血統 (BLOD の SNPC 利用向け)
    Bldn,
    /// タイム型/対戦型マイニング (DM,TM)
    Ming,
    /// 競走馬データ指標 (CK)
    Snap,
    /// 競走馬データ指標 (SNAP の SNPC 利用向け)
    Snpn,
    /// 坂路調教 (HC)
    Slop,
    /// 開催スケジュール (YS)
    Ysch,
    /// 市場取引価格 (HS)
    Hose,
    /// 市場取引価格 (HOSE の SNPC 利用向け)
    Hosn,
    /// 馬名の意味由来 (HY)
    Hoyu,
    /// データ解説 (CS)
    Comm,
    /// ウッドチップ調教 (WC)
    Wood,
}

impl Dataspec {
    /// 全蓄積系 dataspec。
    pub const ALL: &'static [Self] = &[
        Self::Toku,
        Self::Race,
        Self::Diff,
        Self::Difn,
        Self::Blod,
        Self::Bldn,
        Self::Ming,
        Self::Snap,
        Self::Snpn,
        Self::Slop,
        Self::Ysch,
        Self::Hose,
        Self::Hosn,
        Self::Hoyu,
        Self::Comm,
        Self::Wood,
    ];

    /// backfill の既定セット (minimum slice に必要なもの)。
    /// RACE: レース/出走馬/払戻、DIFF: 馬・騎手・調教師マスタ、YSCH: 開催スケジュール。
    pub const DEFAULT_BACKFILL: &'static [Self] = &[Self::Race, Self::Diff, Self::Ysch];

    pub fn id(self) -> &'static str {
        match self {
            Self::Toku => "TOKU",
            Self::Race => "RACE",
            Self::Diff => "DIFF",
            Self::Difn => "DIFN",
            Self::Blod => "BLOD",
            Self::Bldn => "BLDN",
            Self::Ming => "MING",
            Self::Snap => "SNAP",
            Self::Snpn => "SNPN",
            Self::Slop => "SLOP",
            Self::Ysch => "YSCH",
            Self::Hose => "HOSE",
            Self::Hosn => "HOSN",
            Self::Hoyu => "HOYU",
            Self::Comm => "COMM",
            Self::Wood => "WOOD",
        }
    }

    /// 仕様書 4.2.2: 「全データを取得するため、読み出し終了ポイント時刻を
    /// 指定することができません」とある dataspec。これらは from 以降の
    /// 全件しか取れないので year-window 分割できない。
    pub fn supports_end_time(self) -> bool {
        !matches!(
            self,
            Self::Toku
                | Self::Diff
                | Self::Difn
                | Self::Hose
                | Self::Hosn
                | Self::Hoyu
                | Self::Comm
        )
    }

    /// 仕様書 4.2.2 の option×dataspec 組み合わせ表: option=1 (通常データ)
    /// で指定できる dataspec。MING / COMM は option=3,4 (セットアップ) でしか
    /// 指定できないので、これらに option=1 を投げると -116 で失敗する。
    /// resume の差分取得はこの判定で対象を絞る。
    pub fn supports_normal_option(self) -> bool {
        !matches!(self, Self::Ming | Self::Comm)
    }

    /// JVOpen option: 蓄積系の初回セットアップ取得は 4 (指定した取得元を使う)。
    pub const SETUP_OPTION: i32 = 4;
    /// JVOpen option: 通常の差分取得は 1。
    pub const NORMAL_OPTION: i32 = 1;
}

impl fmt::Display for Dataspec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

impl FromStr for Dataspec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|d| d.id() == s.to_ascii_uppercase())
            .ok_or_else(|| format!("unknown dataspec: {s}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// option=1 (通常) を受理しないデータ種で diff 取得しない
    /// (JV-Data 仕様書のデータ種別 option 一覧)。
    #[test]
    fn setup_only_dataspecs_do_not_support_normal_option() {
        for &ds in Dataspec::ALL {
            assert_eq!(
                ds.supports_normal_option(),
                ds != Dataspec::Ming && ds != Dataspec::Comm,
                "{ds:?}"
            );
        }
    }
}
