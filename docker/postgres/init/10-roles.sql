-- 接続するロールは用務ごとに分ける。管理用のスーパーユーザー (POSTGRES_USER)
-- はホスト側だけが使い、収集側と監視側には渡さない。
--
-- このファイルはボリュームが空のときに 1 度だけ実行される。
-- 既存のボリュームには効かないので、その場合は手で流す。

-- wine コンテナの中の 32-bit プロセス (bridge) が使うロール。
-- テーブル権限はスキーマを作るときに、必要な分だけここに足す。
create role jvdata_loader login password 'jvdata_loader';
grant connect on database jvdata to jvdata_loader;

-- postgres_exporter が使うロール。統計ビューしか読まない。
-- pg_monitor は pg_read_all_settings / pg_read_all_stats / pg_stat_scan_tables を含む。
create role jvdata_metrics login password 'jvdata_metrics';
grant connect on database jvdata to jvdata_metrics;
grant pg_monitor to jvdata_metrics;

-- pg_stat_statements は shared_preload_libraries で読み込む設定にしてある
-- (docker/postgres/conf/postgresql.conf)。CREATE EXTENSION 自体はライブラリが
-- 読み込まれていなくても通る。拡張はデータベースごとのものなので、
-- データベースを増やしたらその中でも実行する。
create extension if not exists pg_stat_statements;
