#!/bin/sh
# ロールと拡張を作る。
#
# パスワードはコンテナの環境変数から取る (compose.yaml が varlock で解決した値を
# 渡す)。compose.yaml にもここにも平文では書かない。
#
# このファイルはボリュームが空のときに 1 度だけ実行される。既存のボリュームには
# 効かないので、その場合は手で流すか、ボリュームを作り直す。
#
# 接続するロールは用務ごとに分ける。管理用のスーパーユーザー (POSTGRES_USER) は
# ホスト側だけが使い、収集側と監視側には渡さない。
set -eu

psql -v ON_ERROR_STOP=1 \
  -v db="$POSTGRES_DB" \
  -v loader_password="$POSTGRES_LOADER_PASSWORD" \
  -v metrics_password="$POSTGRES_METRICS_PASSWORD" \
  --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" <<'SQL'
-- wine コンテナの中の 32-bit プロセス (bridge) が使うロール。
-- テーブル権限はスキーマを作るときに、必要な分だけここに足す。
create role jvdata_loader login password :'loader_password';
grant connect on database :"db" to jvdata_loader;

-- postgres_exporter が使うロール。統計ビューしか読まない。
-- pg_monitor は pg_read_all_settings / pg_read_all_stats / pg_stat_scan_tables を含む。
create role jvdata_metrics login password :'metrics_password';
grant connect on database :"db" to jvdata_metrics;
grant pg_monitor to jvdata_metrics;

-- pg_stat_statements は shared_preload_libraries で読み込む設定にしてある
-- (docker/postgres/conf/postgresql.conf)。CREATE EXTENSION 自体はライブラリが
-- 読み込まれていなくても通る。拡張はデータベースごとのものなので、
-- データベースを増やしたらその中でも実行する。
create extension if not exists pg_stat_statements;
SQL
