# データ蓄積 runbook (本番)

本番スタック (`deploy/production`) の `jv` スキーマに JV-Data を蓄積する
一連の手順。スタック自体の構築・バックアップ・更新は [README.md](README.md)、
JV-Link 内部の仕組みは [docs/jv-link-setup.md](../../docs/jv-link-setup.md) を参照。

## 構成

```text
jvdata (ホスト上の CLI)
  ├─ PostgreSQL へ SQL (loopback に一時公開したポート経由。手順 6)
  └─ scripts/jvlink-bridge.sh
       └─ docker exec jvdata-prod-wine-1 sh -c 'wine Z:/opt/jvdata/jvlink-bridge.exe ...'
            └─ jvlink-bridge.exe → JVDTLab.dll (COM, 32bit) → JRA-VAN
```

書き込みはすべてホスト側の `jvdata` が行う。wine 内の bridge は
framed binary を stdout に流すだけで DB には繋がない。

## 前提

- **x86_64 の Linux ホスト** (JV-Link が 32-bit x86。ARM では wine 自体が動かない)
- JRA-VAN Data Lab. の利用登録と **17 桁の利用キー**
- JV-Link インストーラ (`JV-Link.exe`。JVDTLABSDK* に同梱)
- Docker / Docker Compose と varlock (README.md「準備」)
- Rust toolchain (`rust-toolchain.toml` の pin) と mingw リンカ (手順 1)

## 1. バイナリのビルド (ホスト)

必要なバイナリは 2 つ: ホストで動く `jvdata` と、wine の中で動く
`jvlink-bridge.exe` (32-bit Windows exe)。後者はイメージに焼かず
ホストでクロスビルドしてコンテナに bind mount する。

```sh
# リポジトリ root で

# jvdata CLI
cargo build --release -p importer-cli
# → target/release/jvdata

# jvlink-bridge.exe
#   mingw リンカが要る (gnullvm は self-contained のリンクを持たない):
#     sudo apt install gcc-mingw-w64-i686
#   または llvm-mingw (https://github.com/mstorsjo/llvm-mingw/releases) を
#   展開して PATH に足す (sudo 不要):
#     export PATH=/path/to/llvm-mingw/bin:$PATH
rustup target add i686-pc-windows-gnullvm
RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release --target i686-pc-windows-gnullvm -p bridge --bin jvlink-bridge
# → target/i686-pc-windows-gnullvm/release/jvlink-bridge.exe
```

- `--target` を忘れると「runs only under Windows」とだけ言うスタブが
  出来てしまう (bin は `#[cfg(windows)]` 囲み)
- `+crt-static` は CRT / libunwind を静的リンクし、wine prefix に無い
  DLL への依存を無くすために必須
- 確認: `file target/i686-pc-windows-gnullvm/release/jvlink-bridge.exe`
  が `PE32 executable ... Intel i386`
- exe は `target/i686-pc-windows-gnullvm/release/` ごと wine の
  `/opt/jvdata` (`Z:\opt\jvdata`) に read-only mount される。差し替えても
  コンテナ再起動は不要 (次の `docker exec` から新しい exe が読まれる)

## 2. deploy/production/.env

```sh
cd deploy/production
umask 077
cat > .env <<'EOF'
POSTGRES_PASSWORD=<管理用ロール>
POSTGRES_LOADER_PASSWORD=<収集用ロール>
POSTGRES_METRICS_PASSWORD=<監視用ロール>
GF_SECURITY_ADMIN_PASSWORD=<Grafana admin>
EOF
varlock load     # 不足があればここで落ちる
```

## 3. postgresql.local.conf

`postgresql.local.conf` のメモリ設定をホストの RAM に合わせる
(既定は 8GB 想定の例)。開発スタックと同居するならその分も見込む。

## 4. スタック起動

```sh
# リポジトリ root から。本番ホストでは MISE_TASK_RUN_AUTO_INSTALL=false
# を付ける (root の mise.toml は開発ツールも宣言しているため)
MISE_TASK_RUN_AUTO_INSTALL=false mise run prod:check
MISE_TASK_RUN_AUTO_INSTALL=false mise run prod:up
```

初回起動時に init スクリプトが `jvdata_loader` / `jvdata_metrics` ロールを
作る。`down -v` は pgdata ごと消えるので絶対に叩かない。

## 5. JV-Link の導入 (prod wine)

本番の `wineprefix` は開発用とは別ボリュームなので、導入とキー登録は
ここでやり直す。コンテナ名も開発用 (`jvdata-pipeline-wine-1`) とは違う。

```sh
docker cp JV-Link.exe jvdata-prod-wine-1:/tmp/
# http://127.0.0.1:16080/vnc.html を開き、インストーラを実行して
# 利用規約に同意・利用登録する (GUI が使えない場合は unshield + regsvr32 の
# 手動登録。docs/jv-link-setup.md 参照)
docker restart jvdata-prod-wine-1
docker logs jvdata-prod-wine-1    # 未導入警告が消えたか確認

# 疎通確認と利用キー登録 (リポジトリ root で)
export JVDATA_WINE_CONTAINER=jvdata-prod-wine-1
./scripts/jvlink-bridge.sh ping                    # stderr に JVInit -> 0 なら OK
./scripts/jvlink-bridge.sh set-key <17桁の利用キー>  # wine prefix のレジストリに永続化
```

## 6. PostgreSQL を loopback に一時公開

本番の postgres はホストにポートを公開しないので、ホスト側の `jvdata`
が繋ぐために `compose.yaml` の `postgres` に ports を足して `prod:up`
し直す (loopback のみ。README.md「運用」も同じ方式)。

```yaml
  postgres:
    # ...
    ports:
      - "127.0.0.1:15432:5432"   # dev の 5432 と競合しない任意の空きポート
```

## 7. migrate → backfill

```sh
cd <repo root>
set -a; . ./deploy/production/.env; set +a      # パスワードを env に読み込む
export POSTGRES_HOST=127.0.0.1 POSTGRES_PORT=15432
export JVDATA_WINE_CONTAINER=jvdata-prod-wine-1  # 忘れると dev コンテナを叩きにいく
# export JVDATA_SID=<JRA-VAN 発行の SID があれば>

./target/release/jvdata migrate                  # ingest.* / jv.* を作成

# 5 年分の例。JV-Link の逐次取得なので長時間。tmux 等で実行する
./target/release/jvdata backfill --from 2021-01-01 --to 2026-09-21
./target/release/jvdata status
```

- dataspec の既定は `RACE,DIFF,YSCH` (レース・出走馬・払戻・マスタ・
  開催スケジュール)。`--dataspec` で追加できる (例: `BLOD` 血統、
  `SLOP`/`WOOD` 調教、`TOKU` 特別登録馬、`SNAP` 馬指標)
- window は暦年で分割される (終了時刻を指定できる dataspec)。
  `DIFF` 等は「from 以降の全件」の open-ended 1 window
- `--to` が今日以降だと最終 window は open-ended になり、以降の更新は
  `jvdata resume` の差分取得 (option=1) に繋がる
- ingest 系は同時 1 プロセス (advisory lock)。`status` は実行中でも読める
- `migrate` だけ管理用 `jvdata` ロールが必要。backfill/resume/normalize/
  status は `jvdata_loader` の資格情報でも動く
  (`POSTGRES_USER=jvdata_loader` + `POSTGRES_PASSWORD=$POSTGRES_LOADER_PASSWORD`)

### 初回だけ noVNC の操作が要る

option=4 の最初の JVOpen で「スタートキットを持っているか」ダイアログが
**1 度だけ**出る。<http://127.0.0.1:16080/vnc.html> で
「スタートキット(CD/DVD-ROM)を持っていない」を選んで進める。
放置すると JVOpen が -2 を返してその window が失敗になる。

## 8. 蓄積後の運用

```sh
./target/release/jvdata resume    # 残 window + 各 dataspec の差分 + normalize
./target/release/jvdata status    # run / checkpoint / raw / jv の件数
```

- 中断しても checkpoint 冪等。同じ `backfill` か `resume` で
  `last_file` まで JVSkip して続きから読む
- window 単位の失敗は隣を止めず最後にまとめて報告される
- 恒久的に失敗する window は `jvdata abandon --dataspec RACE
  --window-start <YYYYMMDDhhmmss>` で実行対象から外せる
  (復活させるには `--to` を変えて backfill を seed し直す)
- parser を修正したあとは `jvdata reparse` で失敗分を再処理
  (`--all` で全 raw を再 parse)
- 定期差分は `jvdata resume` を cron 等で回す (例: 開催日の夜)

## 容量と監視

`pgdata` / `walarchive` / `backup` ボリュームは同じディスクを食う。
raw は Shift_JIS の無加工バイトを保持するため、年数分それなりの
サイズになる。`df` と `docker system df -v` で監視すること。
WAL はアイドル中も `archive_timeout = 15min` で増え続け、バックアップは
毎日 `pg_dump` / 毎週 `pg_basebackup` が走る (README.md「バックアップ」)。
埋まると DB も止まる。

## トラブルシュート

| 症状 | 原因 | 対処 |
| --- | --- | --- |
| JVOpen `-303` | 利用キー未設定 | `set-key` で 17 桁キーを登録 |
| JVOpen `-301` | キー不正 / 別環境で同一キー | キー確認。dev prefix に同キーを残したまま並行利用しない |
| JVOpen `-2` | 初回ダイアログがキャンセル | noVNC (16080) で応答してから再実行 |
| JVOpen `-1` | 該当データなし | 正常。その window は 0 件で done |
| `別の jvdata プロセスが実行中です` | advisory lock | 先行プロセスの終了を待つ |
| 全 window が同じ理由で即中断 | DB 断 / bridge spawn 不可 | コンテナ・ポート公開・`JVDATA_WINE_CONTAINER` を確認 |
| window が停滞で失敗 | JVStatus の進捗が `JVDATA_STALL_SECS` (既定 600s) 無し | JV-Link / wine 側の状態を確認して resume |
| exe が古い/見つからない | bind mount 先が空 | 手順 1 の出力先 (`target/i686-pc-windows-gnullvm/release`) を確認 |

その他の JV-Link エラーコードは [docs/jv-link-setup.md](../../docs/jv-link-setup.md) の表を参照。

## 片付け

一時公開した `ports` は外して `prod:up` し直すと元の非公開構成に戻る。
loopback 限定なので、定期 `resume` をホストから回す運用なら残す判断もある。
