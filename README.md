# jvdata-pipeline

## 開発用コンテナ

`compose.yaml` で一式を起動する。

| サービス | 役割 | 公開 |
| --- | --- | --- |
| `postgres` | PostgreSQL 18。Debian 版に ja_JP.UTF-8 を焼き込んで使う | `127.0.0.1:5432` |
| `postgres_exporter` | PostgreSQL のメトリクスを Prometheus 形式で公開 | `127.0.0.1:9187` |
| `prometheus` | メトリクスの収集と保存 (保持 30 日) | `127.0.0.1:9090` |
| `grafana` | メトリクスの可視化。ダッシュボード同梱 | `127.0.0.1:13000` |
| `pgadmin` | GUI の DB 運用ツール (接続先登録済み) | `127.0.0.1:5050` |
| `dbtools` | psql / pgcli。DB コンテナに運用ツールを入れないため分離している | - |
| `wine` | 32-bit Windows COM (JV-Link) を呼ぶための Wine。noVNC のデスクトップ付き | `127.0.0.1:6080` |

```sh
docker compose up -d
docker compose exec dbtools psql                     # DB を触る
docker compose exec wine wine /opt/smoke/com32.exe   # 32-bit COM が動くか確認する
docker compose down                                  # データはボリュームに残る
```

| | URL | ログイン |
| --- | --- | --- |
| Grafana | http://127.0.0.1:13000 | `admin` / `jvdata` |
| pgAdmin | http://127.0.0.1:5050 | `jvdata@example.com` / `jvdata` |
| Prometheus | http://127.0.0.1:9090 | - |

DB の運用・監視・チューニングの手順は [docs/postgres-operations.md](docs/postgres-operations.md)。

### PostgreSQL への接続経路とロール

| どこから | 接続先 | ロール |
| --- | --- | --- |
| ホスト OS | `127.0.0.1:5432` | `jvdata` (管理用) |
| wine コンテナ | `postgres:5432` | `jvdata_loader` (収集用 / 管理権限なし) |
| postgres_exporter / Grafana | `postgres:5432` | `jvdata_metrics` (統計の読み取りのみ) |

接続文字列は `postgres://<user>:<password>@<host>:5432/jvdata`。wine コンテナには
`POSTGRES_HOST` / `POSTGRES_PORT` / `POSTGRES_DB` / `POSTGRES_USER` /
`POSTGRES_PASSWORD` を渡してあるので、その中の 32-bit プロセスはそこから読める。
`jvdata_loader` のテーブル権限は、スキーマを作るときに
`docker/postgres/init/10-roles.sql` へ足す。

どちらの経路かは PostgreSQL には見えない (見えるのはロールと接続元 IP だけ)。
接続元 IP は実行環境で変わるため (実測: Docker Desktop のホストからは
`172.19.0.1`、Linux のホストからは `127.0.0.1`、wine コンテナからは `172.19.0.x`)、
経路での制限は pg_hba ではなくロールの分離で行う。

### wine は x86_64 でしか動かない

JV-Link も、それを呼ぶ 32-bit プロセスも x86 のバイナリなので、ARM ホストから
Wine 経由で実行する経路は無い。`wine` サービスは `platform: linux/amd64` に固定
してあり、起動のたびに 32-bit のスモークテスト (`/opt/smoke/com32.exe`) を実行
する。32-bit が動かないホストではここで落ちる。ARM ホストではそれより手前、
prefix の作成 (`wineboot`) で Wine 自身が落ちる (qemu の `anon_mmap_fixed`
アサーション)。

### JV-Link の導入は手作業

JV-Link は再配布できないのでイメージには入っていない。Wine の prefix は
`wineprefix` ボリュームにあるため、一度入れれば `docker compose down` では消えない。

1. 入手したインストーラをコンテナへコピーする (`docker compose cp ./JVLink.exe wine:/tmp/`)
2. http://localhost:6080/vnc.html を開く
3. デスクトップ上でインストーラを実行し、JRA-VAN の利用規約に同意して利用登録する
4. `docker compose restart wine` を実行し、起動ログから JV-Link の未導入警告が消えたことを確認する
