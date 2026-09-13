-- wine コンテナの中の 32-bit プロセス (bridge) が使うロール。
--
-- ホスト側が使う管理用ロール (POSTGRES_USER) とは別にして、収集側には
-- 管理権限を渡さない。テーブル権限はスキーマを作るときに、必要な分だけ
-- ここに足す。
--
-- このファイルはボリュームが空のときに 1 度だけ実行される。
create role jvdata_loader login password 'jvdata_loader';

grant connect on database jvdata to jvdata_loader;
