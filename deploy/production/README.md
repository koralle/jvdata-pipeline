# 本番スタック

JRA-VAN のデータを実際に蓄積するホスト用。**x86_64 の Linux** が要る
(JV-Link が 32-bit x86 なので、wine を動かせるホスト)。

このスタックを立ててから JV-Data を蓄積・運用するまでの一連の手順は
[ingest.md](ingest.md) にまとめてある。

開発用スタック (リポジトリ直下) とは別プロジェクト・別ディレクトリのままで、
同じマシンで同居できる。ホストのポートは重ならない番号に割り当ててある
(Grafana 23000 / Prometheus 19090 / noVNC 16080)。共有しているのはイメージの
ビルド元 (`docker/`) と設定・ダッシュボードのファイルだけ。

## 何が違うか

| | 開発用 | 本番用 |
| --- | --- | --- |
| 資格情報 | `.env` に自動生成した使い捨ての値 | 環境変数か、手元で用意した `.env` (repo に値は無い) |
| Postgres の待ち受け | `127.0.0.1:5432` に公開 | **公開しない** (`exec dbtools psql` で入る) |
| 再起動 | 手動 | `restart: unless-stopped` |
| PostgreSQL の設定 | `docker/postgres/conf/postgresql.conf` | それ + `postgresql.local.conf` で上書き |
| バックアップ | 無し (消して良い) | `pg_dump` 毎日 + `pg_basebackup` 毎週 |
| pgAdmin | 置く | 置かない (管理 UI を本番ホストに置かない) |
| Prometheus の保持 | 30 日 | 90 日 |

## 準備 (ホストで 1 回)

必要なもの: Docker / Docker Compose と、varlock を動かすための Node.js。

```sh
npm install -g varlock@1.19.0     # mise で入れても良い

# 新しく用意したホストなら clone する (開発用と同じマシンなら不要)
git clone <repo> && cd <repo>/deploy/production

# 値を作る。.env.schema が要求するのはこの 4 つだけ
umask 077
cat > .env <<'EOF'
POSTGRES_PASSWORD=<管理用ロール>
POSTGRES_LOADER_PASSWORD=<収集用ロール>
POSTGRES_METRICS_PASSWORD=<監視用ロール>
GF_SECURITY_ADMIN_PASSWORD=<Grafana の admin>
EOF

varlock load     # 足りない変数があればここで落ちる
```

secret manager を使うなら、`varlock run` の前段で環境変数に入れる
(環境変数はスキーマの値より優先される)。

収集に使う `jvlink-bridge.exe` は wine コンテナに bind mount するので、
先にホスト側でクロスビルドしておく (手順は `docs/jv-link-setup.md`):

```sh
# リポジトリの root から。llvm-mingw 等のリンカが要る
cargo build --release --target i686-pc-windows-gnullvm -p bridge --bin jvlink-bridge
# → target/i686-pc-windows-gnullvm/release/jvlink-bridge.exe
#   compose.yaml がこのディレクトリを wine の /opt/jvdata (Z:\opt\jvdata) にマウント
```

## 起動

```sh
# リポジトリの root から。dir で deploy/production に入ってから実行される
mise run prod:check    # 設定を変えたら先にこれ (起動しない)
mise run prod:up
mise run prod:psql    # ログは mise run prod:logs

# mise を入れないホストでは deploy/production で直接
varlock run -- docker compose up -d
varlock run -- docker compose ps
```

初回だけ init スクリプトがロールと拡張を作る。以降は走らないので、ロールを
足したときは初期化し直しが必要 (`down -v` は**データも消える**ので注意)。

起動前に `postgresql.local.conf` のメモリ設定を合わせること (要再起動)。
開発用スタックと同居するなら、その分 (開発用の postgres / Prometheus / Grafana)
も見込んで決める。

## 運用

DB は公開していないので、psql は `dbtools` 経由で:

```sh
mise run prod:psql
varlock run -- docker compose exec dbtools psql    # 直接叩く場合
```

画面は loopback にだけ出ている:

| | 見るには |
| --- | --- |
| Grafana (23000) | <http://127.0.0.1:23000> |
| Prometheus (19090) | <http://127.0.0.1:19090> |
| noVNC (16080, JV-Link の画面) | <http://127.0.0.1:16080/vnc.html> |

別のマシンから見るときは SSH ポート転送 (`ssh -L 23000:127.0.0.1:23000 <host>`
のように、上のホスト側ポートをそのまま使う)。DB は待ち受けていないので、
ホストから `psql` を直接使いたいときは一時的に `postgres` の `ports` を足す。

## バックアップ

`backup` サービスが常駐している。

| 頻度 | 中身 | 置き場 | 世代 |
| --- | --- | --- | --- |
| 毎日 | `pg_dump -Fc` | `/backup/dump` | 7 |
| 毎週 | `pg_basebackup -Ft -z` | `/backup/base` | 2 |

WAL アーカイブ (`walarchive` ボリューム) は、ベースバックアップが成功する
たびに `pg_archivecleanup` で世代管理している。残っているベースのうち
最古のものの START WAL より前の分は replay できないので消す。ベースが
取れていない間は境が進まないので、アーカイブだけ先に消えることはない。
それでも WAL は増え続ける (アイドル時でも `archive_timeout` で 15 分ごとに
セグメントが切れる) ので、容量の監視は要る。

```sh
varlock run -- docker compose exec backup ls -l /backup/dump
varlock run -- docker compose run --rm -e BACKUP_RUN_ONCE=1 backup   # 手で 1 回
```

**同じホストのボリュームに置いているだけなので、別ホストへ退避すること。**

```sh
# 例: 手元に引く
ssh <host> 'docker run --rm -v jvdata-prod_backup:/b -v /tmp:/out alpine \
  tar cf /out/backup.tar -C /b .'
```

復元:

```sh
# 論理 (dump から)。まず別 DB に戻して中身を確認する
varlock run -- docker compose exec postgres createdb -U jvdata restore_check
varlock run -- docker compose exec postgres pg_restore -U jvdata -d restore_check /path/to.dump

# PITR (base + WAL)。PostgreSQL のドキュメント通り:
#   1. DB を止める
#   2. pgdata を退避し、base の tar を展開して配置
#   3. recovery.signal を置き、restore_command を書く
#      restore_command = 'cp /var/lib/postgresql/wal-archive/%f %p'
#   4. 起動して recovery_target_time まで戻す
```

`backup` / `walarchive` ボリュームは同じディスクを食う。埋まると DB も止まるので
使用量を見ておくこと。

## 更新

```sh
git pull
varlock run -- docker compose build --pull   # ベースイメージはダイジェスト固定
varlock run -- docker compose up -d
```

設定を変えたときは `varlock run -- docker compose restart postgres`。

## まだ無いもの

- レプリカ / フェイルオーバー (1 台構成。ホストが死ねば止まる)
- アラート通知 (ダッシュボードはあるが通知は未設定)
- バックアップの自動退避 (上記は手動)
- パイプライン自体のメトリクス (importer 実装後)
