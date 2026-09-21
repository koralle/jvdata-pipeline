//! PostgreSQL (SQLx) アクセス層。
//!
//! ingest.* への raw 永続化と jv.* への正規化を担当する。
//! JV-Link や COM の知識は持たない。

mod ingest;
mod normalize;
mod status;

use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::{ConnectOptions, PgPool};

pub use ingest::*;
pub use normalize::*;
pub use status::*;

pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// ingest 系コマンド (backfill/resume/normalize/reparse/migrate) の
/// 相互排他用 advisory lock のキー。pool ではなく専用接続で取るので、
/// guard の drop (= セッション切断) で確実に解放される。
const INGEST_LOCK_KEY: i64 = 0x4A56_4441_5441_4C4B; // "JVDATALK"

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
    opts: PgConnectOptions,
}

impl Store {
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        Self::connect_options(url.parse()?).await
    }

    /// URL を介さない接続。パスワードに URL の区切り文字
    /// (@, /, #, ?, %) が含まれていても正しく接続できる。
    pub async fn connect_options(opts: PgConnectOptions) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect_with(opts.clone())
            .await?;
        Ok(Self { pool, opts })
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// ingest 系コマンドの排他ロックを試みる。
    /// 他プロセスが保持中なら None。取得できた場合は guard が
    /// drop されるまでロックが生きる (セッションレベルロック)。
    /// checkpoint や run の状態を壊さないよう、同時に走る
    /// ingest プロセスは 1 つに限る。
    pub async fn try_ingest_lock(&self) -> Result<Option<IngestLock>, sqlx::Error> {
        let mut conn = self.opts.clone().connect().await?;
        let got: bool = sqlx::query_scalar("select pg_try_advisory_lock($1)")
            .bind(INGEST_LOCK_KEY)
            .fetch_one(&mut conn)
            .await?;
        Ok(got.then_some(IngestLock { conn }))
    }

    /// ブロッキング版の排他ロック。空くまで待つ。
    /// 統合テストの直列化用 (normalize_pending は全 run の pending を
    /// 処理するので、同一 DB を触るテスト同士は直列にしないと
    /// 片方の cleanup がもう片方の parse_errors 挿入と競合する)。
    pub async fn ingest_lock(&self) -> Result<IngestLock, sqlx::Error> {
        let mut conn = self.opts.clone().connect().await?;
        sqlx::query("select pg_advisory_lock($1)")
            .bind(INGEST_LOCK_KEY)
            .execute(&mut conn)
            .await?;
        Ok(IngestLock { conn })
    }

    pub async fn migrate(&self) -> Result<(), sqlx::migrate::MigrateError> {
        MIGRATOR.run(&self.pool).await
    }
}

/// advisory lock 保持中を表す guard。drop で接続が切れて解放される。
/// (drop による解放はセッション切断完了まで非同期に遅れる。
///   すぐに再取得させたい場合は `unlock` で明示的に解放する)
pub struct IngestLock {
    conn: sqlx::postgres::PgConnection,
}

impl IngestLock {
    /// 明示的にロックを解放する。戻った時点で他セッションが取得できる。
    pub async fn unlock(self) -> Result<(), sqlx::Error> {
        let mut conn = self.conn;
        sqlx::query("select pg_advisory_unlock($1)")
            .bind(INGEST_LOCK_KEY)
            .execute(&mut conn)
            .await?;
        Ok(())
    }
}
