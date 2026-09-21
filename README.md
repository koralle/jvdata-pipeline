# jvdata-pipeline

## 開発用コンテナ

`compose.yaml` で一式を起動する。

| サービス | 役割 | 公開 |
| --- | --- | --- |
| `postgres` | PostgreSQL 18。Debian 版に ja_JP.UTF-8 を焼き込んで使う | `127.0.0.1:5432` |
| `postgres_exporter` | PostgreSQL のメトリクスを Prometheus 形式で公開 | `127.0.0.1:9187` |
| `prometheus` | メトリクスの収集と保存 (保持 30 日) | tailnet IP の `9090` |
| `grafana` | メトリクスの可視化。ダッシュボード同梱 | tailnet IP の `13000` |
| `pgadmin` | GUI の DB 運用ツール (接続先登録済み) | tailnet IP の `5050` |
| `dbtools` | psql / pgcli。DB コンテナに運用ツールを入れないため分離している | - |
| `wine` | 32-bit Windows COM (JV-Link) を呼ぶための Wine。noVNC のデスクトップ付き | `127.0.0.1:6080` |

```sh
mise run up      # = varlock run -- docker compose up -d
mise run cert    # Tailscale の証明書を取って UI を再起動 (初回と 90 日ごと)
mise run psql    # psql で DB を触る (pgcli は mise run pgcli / ログは mise run logs)
mise run down    # 停止 (データはボリュームに残る)

# 設定を変えたときは、起動せずに検証できる (`docker compose config -q`)
mise run compose:check
```

`docker compose` を直接叩きたいときは `varlock run -- docker compose ...` 経由にする。
資格情報は `.env.schema` から注入される (直接叩くと未設定で落ちる)。

| | URL | ログイン |
| --- | --- | --- |
| Grafana | `https://<TS_FQDN>:13000` | `admin` / `.env` の `GF_ADMIN_PASSWORD` |
| pgAdmin | `https://<TS_FQDN>:5050` | `jvdata@example.com` / `.env` の `PGADMIN_PASSWORD` |
| Prometheus | `https://<TS_FQDN>:9090` | - |

UI の 3 つは Tailscale の証明書で自分で HTTPS を喋る (`tailscale serve` も
前段のプロキシも置いていない)。HTTP では開けない。ブラウザの secure context 判定は
スキームしか見ないので、tailnet 内でも HTTPS にしておくと Clipboard などが使える。

- `<TS_FQDN>` は MagicDNS 名 (`.env.schema` / `.env` の `TS_FQDN`、既定は開発機の値)。
  `https://100.x.x.x:13000` のような IP 直打ちは証明書の名前が合わないので弾かれる。
- 初回だけ `mise run cert` で証明書を取る (`docker/tls/` は git 管理外)。
  証明書は 90 日で切れるので、期限が近づいたら同じコマンドを叩く。
- bind 先は tailnet の IP だけ。`http://localhost:13000` のような loopback は使わない
  (このマシン自身からも上の URL で開く)。

DB の運用・監視・チューニングの手順は [docs/postgres-operations.md](docs/postgres-operations.md)。

### これは開発用。本番では使わない

このスタックは開発機向けで、本番のデータを載せる前提で作っていない。

- パスワードは repo に無い。開発機の分は `.env` (初回に自動生成)、本番は環境変数で渡す
- データは named volume にあるだけ。バックアップも WAL アーカイブも無く、`docker compose down -v` で消える
- `shared_buffers` などは開発機向けの初期値のまま
- `restart` ポリシーが無いので、ホストを再起動しても勝手には戻らない
- pgAdmin / Grafana / noVNC が同じホストに同居する

本番は別構成にしてある: **[deploy/production](deploy/production/README.md)**。
同じマシンで同居できる (プロジェクト名もポートも別)。起動は root から
`mise run prod:up` (`mise run prod:psql` / `mise run prod:logs` / `mise run prod:down`)。
この compose を流用しないこと（本番側は DB をホストに公開せず、`restart` と
バックアップを入れてある）。

接続が loopback 限定なのは変える時の事故を防ぐため。別ホストから繋ぐなら
SSH ポート転送など、経路を意識して開けること。

### 環境変数

資格情報は compose.yaml に書かず、`.env.schema` に定義して varlock で注入する。

```sh
mise run up      # = varlock run -- docker compose up -d
varlock load     # 検証して中身を見る (sensitive は伏字)
```

- `.env.schema` … どの変数が必要かの定義。`@sensitive` で伏字対象を指定する
- 値の優先順 … プロセスの環境変数 > `.env` > `.env.schema` に書いた既定値
- `.env` … 実際の値。git 管理外 (`.gitignore` 済み)。`mise run up` の初回に
  `mise run env:init` が作る (値はマシンごとのランダム)。無ければ varlock が
  「required but empty」で止めるので、空のパスワードで起動することはない
- compose 側は未設定の変数で落ちる (`${VAR:?}`) ので、注入し忘れは起動前に分かる

別ホストの DB を向けたいときは `.env` で上書きする。

```sh
# .env (例: SSH ポート転送で別ホストの DB を使う)
POSTGRES_HOST=host.docker.internal
POSTGRES_PORT=15432
```

### PostgreSQL への接続経路とロール

| どこから | 接続先 | ロール |
| --- | --- | --- |
| ホスト OS (`jvdata migrate`) | `127.0.0.1:5432` | `jvdata` (管理用) |
| ホスト OS (backfill 等の収集系) | `127.0.0.1:5432` | `jvdata` または `jvdata_loader` (収集用 / 管理権限なし) |
| postgres_exporter / Grafana | `postgres:5432` | `jvdata_metrics` (統計の読み取りのみ) |

接続文字列は `postgres://<user>:<password>@<host>:5432/jvdata`。
DB への書き込みはすべてホスト側の `jvdata` CLI が行う
(wine コンテナ内の bridge は PostgreSQL に接続しない。framed binary を
stdout に流すだけ)。`jvdata_loader` には ingest/jv の DML と sequence の
権限があるので、`migrate` 以外のコマンド (backfill / resume / normalize /
reparse / status) は loader の資格情報でも動く。
`jvdata_loader` のテーブル権限は、スキーマを作るときに
`docker/postgres/init/10-roles.sh` へ足す。

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
2. <http://localhost:6080/vnc.html> を開く
3. デスクトップ上でインストーラを実行し、JRA-VAN の利用規約に同意して利用登録する
4. `docker compose restart wine` を実行し、起動ログから JV-Link の未導入警告が消えたことを確認する

GUI が使えない環境では unshield + regsvr32 での手動登録と、利用キーの
CLI 登録 (`jvlink-bridge.exe set-key`) の手順を **[docs/jv-link-setup.md](docs/jv-link-setup.md)**
にまとめてある。

## データ取り込み (`jvdata` CLI)

`crates/importer-cli` の `jvdata` が取得・蓄積・正規化を司る。

```sh
cargo build -p importer-cli        # → target/debug/jvdata
export DATABASE_URL=postgres://jvdata:$POSTGRES_PASSWORD@127.0.0.1:5432/jvdata

jvdata migrate                     # ingest.* / jv.* schema を適用
jvdata backfill --from 2016-01-01 --to 2026-09-21   # 年単位で分割して蓄積系取得 (option=4)
jvdata resume                      # 中断した window の続き + 差分取得 (option=1)
jvdata normalize                   # raw_records → jv.* へ反映 (backfill/resume でも自動実行)
jvdata reparse                     # parse 失敗・未対応の raw を parse_state=0 に戻して再処理
                                   #   (--all で全 raw を再 parse。parser 修正後に使う)
jvdata abandon --dataspec RACE --window-start 20250101000000
                                   # 恒久的に失敗する window を 'abandoned' にして実行対象から外す
jvdata status                      # run / checkpoint / raw / jv の件数を表示
```

`--source` で取得元を選ぶ (既定 `wine`):

- `wine` … `scripts/jvlink-bridge.sh` 経由で wine コンテナ内の
  `jvlink-bridge.exe` を呼び、JV-Link から `JVGets` (raw Shift_JIS) で取得
- `fixture:DIR` … ローカルファイルを同じ frame 列として再生 (JV-Link 不要)
- 任意のコマンド文字列 … そのコマンドを bridge プロセスとして spawn

```sh
jvdata gen-fixture /tmp/fixture    # 合成 JV-Data 風データを生成
jvdata backfill --from 2024-05-01 --to 2024-05-31 --source fixture:/tmp/fixture
```

### `jvdata` / bridge の環境変数

| 変数 | 用途 |
| --- | --- |
| `DATABASE_URL` | PostgreSQL 接続 URL (`--database-url` で上書き可) |
| `JVDATA_SOURCE` | `--source` の既定値 (`wine` / `fixture:DIR` / 任意コマンド) |
| `JVDATA_WINE_CONTAINER` | bridge を実行する wine コンテナ名 (既定 `jvdata-pipeline-wine-1`) |
| `JVDATA_BRIDGE_SCRIPT` | `scripts/jvlink-bridge.sh` のパス上書き |
| `JVDATA_SID` | JVInit に渡すソフトウェア ID (既定 `UNKNOWN`。docker exec で wine コンテナへ転送される) |
| `JVDATA_STALL_SECS` | ダウンロード停滞検出の上限秒数 (既定 600。JVStatus の進捗がこの時間無いと window 失敗) |

### 再開と冪等性

- 取得は「JV-Link の 1 物理ファイル」をトランザクション境界にする。
  raw_records の INSERT と checkpoint (last_file) の更新は同じ tx で commit され、
  commit より先に checkpoint が進むことはない。
- 途中で切れても `jvdata resume` (または同じ `backfill` の再実行) で
  checkpoint の `last_file` まで JVSkip して続きから読む (JV-Link 公式の再開手順)。
  再開対象ファイルが stream 内に見つからない場合はその window は失敗になる
  (読み飛ばして done にすることはしない)。
- `backfill` / `resume` は取得完了後に pending な raw_records の normalize
  (raw → `jv.*`) まで自動で行う。`jvdata normalize` は手動で回したい場合用。
- window の失敗は隣の window や差分取得、normalize を止めない
  (失敗は checkpoint に記録され、最後にまとめて報告される)。
  DB 障害や bridge の起動不可など全 window が同じ理由で落ちる
  系統的エラーの場合のみ即座に中断する。
- 恒久的に失敗し続ける window は `jvdata abandon` で `abandoned` にして
  実行対象から外せる。復活させたい場合は `--to` を変えて `backfill`
  を再実行する (window 範囲の変更で pending に戻る)。
- 取得側が Done frame を送らずに切れた場合、その window は失敗になる
  (部分的に取れたとみなして done にしない)。JVOpen の -1
  (該当データなし) は「0 件で完了」として扱う。
- `raw_records` は `unique(filename, file_seq, payload_sha256)` で冪等。
  同一レコードの再取得は skip、訂正データ (同キーで中身が違う) は別バージョンとして残る。
- セットアップ (option=4) は「コンテンツ日付 (=開催日)」でフィルタされるので
  ウィンドウは開催日単位で分割すればよい (現在は年単位)。

### schema

- `ingest.runs` … 1 取得実行 (JVOpen 1回) の記録
- `ingest.checkpoints` … dataspec×window ごとの再開位置と状態
- `ingest.cursors` … dataspec ごとの差分取得起点 (lastfiletimestamp)
- `ingest.raw_records` … JV-Link の raw bytes (lossless)
- `ingest.parse_errors` … parse 失敗の記録 (再処理可能)
- `jv.races / entries / horses / jockeys / trainers / payouts / schedule_days` … 正規化済み

### 分析クエリ

`queries/` に分析用 SQL がある (`mise run psql` から `\i` か `-f` で実行できる)。

- `queries/races_by_year.sql` … 年・場別レース数
- `queries/horse_results.sql` … 馬ごとの出走・勝率、騎手別成績
- `queries/payouts_and_odds.sql` … 人気別勝率・回収率、レース別払戻
- `queries/ingest_status.sql` … ingest の実行履歴・checkpoint・DB サイズ
