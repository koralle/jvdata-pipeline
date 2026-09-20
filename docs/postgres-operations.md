# PostgreSQL の運用・監視・チューニング

`docker compose up -d` で上がるスタックの使い方。構成は次のとおり。

```text
PostgreSQL ── postgres_exporter ── Prometheus ── Grafana
     └───────────────────────────────────────────────┘
              Grafana の PostgreSQL データソース
              (pg_stat_statements / pg_stat_activity を SQL で直接見る)
```

測定の分担:

- **Prometheus** … 時系列 (推移とレート)。容量の増加、レート、しきい値の監視
- **Grafana の PostgreSQL データソース** … スナップショット (今の SQL、今の接続)

## 接続

接続先とロールの一覧は
[README の「PostgreSQL への接続経路とロール」](../README.md#postgresql-への接続経路とロール)
を参照。

### psql

```sh
mise run psql
```

接続情報は環境変数 (`PGHOST` / `PGDATABASE` / `PGUSER` / `PGPASSWORD`) で
入っているので、引数なしで繋がる。ホスト側のクライアントから直接繋ぐ場合は
`psql "postgresql://jvdata:<password>@127.0.0.1:5432/jvdata"`
(password は `.env` の `POSTGRES_PASSWORD`)。

### pgcli

```sh
mise run pgcli
```

補完とシンタックスハイライト付き。`.pgclirc` を置けば設定できる。

### pgAdmin

`https://<TS_FQDN>:5050` (`jvdata@example.com` / `jvdata`)

サーバーは `docker/pgadmin/servers.json` で登録済み。初回だけパスワード
(`.env` の `POSTGRES_PASSWORD`) の入力を求められる。

## メトリクスとダッシュボード

| | URL | ログイン |
| --- | --- | --- |
| Grafana | `https://<TS_FQDN>:13000` | `admin` / `.env` の `GF_ADMIN_PASSWORD` |
| Prometheus | `https://<TS_FQDN>:9090` | - |
| postgres_exporter (生のメトリクス) | http://127.0.0.1:9187/metrics | - |

ダッシュボードは `jvdata` フォルダの **PostgreSQL (jvdata)** 1 枚
(`docker/grafana/dashboards/jvdata-postgres.json`)。見方は次のとおり。

| パネル | 見方 |
| --- | --- |
| 接続数 / 使用率 | `max_connections` に対する余裕。上がりっぱなしなら接続が漏れている |
| トランザクション / 秒 | commit と rollback の比率。rollback が増えるなら書き込み側のエラー |
| クエリ / 行 / 秒 | 取り込み量。パイプライン側の数字と突き合わせる |
| 行の挿入 / 更新 / 削除 / 秒 | 取り込み量そのもの |
| キャッシュヒット率 | 下がるなら shared_buffers かインデックスを見直す |
| ブロック I/O の所要時間 | read/write の待ち時間。ディスクが律速かどうか |
| 一時ファイル | work_mem 不足の信号。ソート/ハッシュがディスクに落ちている |
| DB / テーブル / インデックスサイズ | 容量の増加傾向。どのテーブルが伸びているか |
| dead tuples / autovacuum | 増え続けるなら autovacuum が追いついていない |
| WAL 発生量 / WAL サイズ | 書き込み量に対する WAL の量とディスク使用量 |
| チェックポイント | 「明示要求」が増えるなら `max_wal_size` が書き込み量に対して小さい |
| ロック数 / 最長トランザクション | 競合の有無。長いトランザクションは autovacuum を妨げる |
| XID の年齢 | `autovacuum_freeze_max_age` に近づいていないか |
| 重い SQL / 実行中のクエリ | 下の調査手順の入口 |

パネルの定義を変えたら、Grafana の UI で編集して
`Save dashboard` → `Export` で `docker/grafana/dashboards/jvdata-postgres.json`
を上書きする (provisioning は 30 秒ごとにファイルを読み直す)。

## パフォーマンス調査の手順

1. **Grafana で当たりを付ける**

   まず「行の挿入 / 秒」と「クエリ / 行 / 秒」で取り込み量を見る。その上で
   次のどれが律速かを切り分ける。

   | 症状 | 疑うところ |
   | --- | --- |
   | キャッシュヒット率が低い / ブロック read 時間が高い | ディスク I/O、`shared_buffers`、インデックス不足 |
   | 一時ファイルが出ている | `work_mem` 不足 (ソート/ハッシュがディスクに落ちている) |
   | チェックポイントの「明示要求」が増える | `max_wal_size` 不足 (書き込みの山でチェックポイントが詰まる) |
   | ロック待ち・長いトランザクション | 書き込みの競合、トランザクションの長時間化 |
   | dead tuples が増え続ける | autovacuum が追いついていない |

2. **重い SQL を特定する**

   ダッシュボードの「重い SQL (合計実行時間)」で `query` と `queryid` を見る。
   同じことを psql でもできる。

   ```sql
   -- 実行時間の合計が大きい SQL
   SELECT queryid, left(query, 100) AS query, calls,
          round(total_exec_time::numeric, 1) AS total_ms,
          round(mean_exec_time::numeric, 2) AS mean_ms,
          rows
   FROM pg_stat_statements
   ORDER BY total_exec_time DESC
   LIMIT 20;

   -- I/O が大きい SQL
   SELECT queryid, left(query, 100) AS query, calls,
          shared_blks_read, shared_blks_written, shared_blks_dirtied
   FROM pg_stat_statements
   ORDER BY shared_blks_read + shared_blks_written DESC
   LIMIT 20;

   -- 実行中のクエリと待ちイベント
   SELECT pid, state, wait_event_type, wait_event,
          now() - xact_start AS duration, left(query, 100) AS query
   FROM pg_stat_activity
   WHERE datname = current_database()
   ORDER BY xact_start;
   ```

   統計をリセットして、変更の前後だけを測ることもできる。

   ```sql
   SELECT pg_stat_statements_reset();
   ```

3. **実行計画を見る**

   ```sql
   EXPLAIN (ANALYZE, BUFFERS) SELECT ...;
   ```

   見るポイント:

   - `Buffers: shared hit=... read=...` … `read` が多いならキャッシュに載っていない
   - `rows` の見積もり (estimated) と actual の差 … 大きくずれるなら
     `ANALYZE` が古いか、統計情報の精度不足 (`default_statistics_target`)
   - `Sort Method: external merge Disk: ...` … `work_mem` 不足
   - `Heap Fetches` (Index Only Scan) … visibility map が進んでいない (VACUUM 不足)

4. **設定を変えて再測定する**

   ```sh
   # docker/postgres/conf/postgresql.conf を編集して
   varlock run -- docker compose restart postgres
   ```

   主なノブ:

   | 設定 | 効くところ |
   | --- | --- |
   | `shared_buffers` | キャッシュヒット率。ホスト RAM の 25% 目安 |
   | `work_mem` | ソート/ハッシュ。一時ファイルが出ているなら上げる (接続数分のメモリを食う点に注意) |
   | `max_wal_size` / `checkpoint_timeout` | チェックポイントの頻度。書き込みの山では `max_wal_size` を上げる |
   | `maintenance_work_mem` | VACUUM / CREATE INDEX の速さ |
   | `effective_cache_size` | プランナの判断 (実際にメモリを確保はしない) |

   自動 vacuum はテーブル単位でも調整できる。

   ```sql
   ALTER TABLE race SET (autovacuum_vacuum_scale_factor = 0.02);
   ALTER TABLE race SET (autovacuum_analyze_scale_factor = 0.01);
   ```

## 注意点

- `docker/postgres/init/` のスクリプトは**ボリュームが空のときだけ**実行される。
  ロールや拡張を足したときは、既存のボリュームでは手で流す。
- 計測用のロール `jvdata_metrics` は統計ビューしか読めない。Grafana の
  PostgreSQL データソースもこれを使っているので、任意の SQL を Grafana から
  流すことはできない (見たい SQL はダッシュボードに足して、
  権限が必要なら `10-roles.sh` に足す)。
- メトリクスの保持期間は Prometheus の `--storage.tsdb.retention.time=30d`。
- パイプライン側のメトリクス (records/sec、backlog、error rate) は
  importer の実装後。当面は「行の挿入 / 秒」と「xact_commit / 秒」で
  書き込み量を見て、パイプライン側と突き合わせる。
