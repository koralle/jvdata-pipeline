# 本番スタック

JRA-VAN のデータを実際に蓄積するホスト用。**x86_64 の Linux 1 台**を占有する前提
(JV-Link が 32-bit x86 なので、wine を動かせるホストが要る)。

開発用スタック (リポジトリ直下) とは別物。共有しているのはイメージのビルド元
(`docker/`) と設定・ダッシュボードのファイルだけ。

## 何が違うか

| | 開発用 | 本番用 |
| --- | --- | --- |
| 資格情報 | `.env` に自動生成した使い捨ての値 | 環境変数か、手元で用意した `.env` (repo に値は無い) |
| Postgres の待ち受け | `127.0.0.1:5432` に公開 | **公開しない** (SSH ポート転送で入る) |
| 再起動 | 手動 | `restart: unless-stopped` |
| PostgreSQL の設定 | `docker/postgres/conf/postgresql.conf` | それ + `postgresql.local.conf` で上書き |
| バックアップ | 無し (消して良い) | `pg_dump` 毎日 + `pg_basebackup` 毎週 |
| pgAdmin | 置く | 置かない (手元の pgAdmin から SSH 経由) |
| Prometheus の保持 | 30 日 | 90 日 |

## 準備 (ホストで 1 回)

必要なもの: Docker / Docker Compose と、varlock を動かすための Node.js。

```sh
npm install -g varlock@1.19.0     # mise で入れても良い

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

## 起動

```sh
# 先に postgresql.local.conf のメモリ設定をホストの RAM に合わせる
varlock run -- docker compose up -d
varlock run -- docker compose ps
```

初回だけ init スクリプトがロールと拡張を作る。以降は走らないので、ロールを
足したときは初期化し直しが必要 (`down -v` は**データも消える**ので注意)。

## 運用

DB は公開していないので、psql はホスト上で:

```sh
varlock run -- docker compose exec dbtools psql
```

外から見るときは SSH ポート転送:

| | 手元で見るには |
| --- | --- |
| Grafana (13000) | `ssh -L 13000:127.0.0.1:13000 <host>` |
| Prometheus (9090) | `ssh -L 9090:127.0.0.1:9090 <host>` |
| noVNC (6080, JV-Link の画面) | `ssh -L 6080:127.0.0.1:6080 <host>` |
| DB (5432, 手元の pgAdmin / psql 用) | `ssh -L 15432:127.0.0.1:5432 <host>` |

`127.0.0.1:5432` はホスト側では待ち受けていないため、上の転送は
`docker compose exec` 経由か、一時的に `postgres` の ports を足す必要がある。
転送で DB を使うなら、ホストで `socat` などを挟むか、
`dbtools` コンテナ経由で `psql` を使うのが簡単。

## バックアップ

`backup` サービスが常駐している。

| 頻度 | 中身 | 置き場 | 世代 |
| --- | --- | --- | --- |
| 毎日 | `pg_dump -Fc` | `/backup/dump` | 7 |
| 毎週 | `pg_basebackup -Ft -z` | `/backup/base` | 2 |

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
