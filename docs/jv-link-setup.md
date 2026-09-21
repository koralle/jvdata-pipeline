# JV-Link セットアップ (wine コンテナ)

JV-Link は 32-bit Windows COM (ActiveX) コンポーネント。Linux では動かないので
compose の `wine` サービス (winehq-stable + noVNC) の中で動かす。

## 構成

```text
jvdata (host, Rust)
  └─ scripts/jvlink-bridge.sh
       └─ docker exec jvdata-pipeline-wine-1 sh -c 'wine Z:/opt/jvdata/jvlink-bridge.exe ...'
            └─ jvlink-bridge.exe (i686-pc-windows-gnullvm でクロスビルド)
                 └─ IDispatch → JVDTLab.dll (CLSID 2AB1774D-0C41-11D7-916F-0003479BEB3F)
                      └─ JRA-VAN Data Lab.
```

stdout は framed binary protocol (`crates/bridge/src/protocol.rs`) で
`OpenOk / Progress / FileBegin / Record / FileEnd / Done / Error` の frame が流れる。
`JVRead` (BSTR 変換) は使わず `JVGets` (raw Shift_JIS bytes) で lossless に取る。

## 手順 (このリポジトリで実施済みのもの)

```sh
# 1. wine コンテナ起動
mise run up            # = varlock run -- docker compose up -d

# 2. JV-Link をインストールする
#    a) 公式 SDK (JVDTLABSDK500_32bit.zip) 内の JV-Link/JV-Link.exe をコンテナへ
docker compose cp JV-Link.exe wine:/tmp/
docker compose exec wine wine /tmp/JV-Link.exe   # GUI。http://localhost:6080/vnc.html

#    b) GUI が使えない場合: JV-Link.exe は InstallShield SFX で、
#       解凍先 Temp/{...}/Disk1/data1.cab の中に JVDTLab.dll がある。
#       unshield で取り出して手動登録でも同等になる:
#         cp JVDTLab.dll /wineprefix/drive_c/windows/syswow64/JVDTLAB/
#         wine regsvr32 /s JVDTLab.dll
#       (実際にこの手順で登録した。entrypoint の smoke test は
#        syswow64/JVDTLAB/JVDTLab.dll の存在を見る)

# 3. 疎通確認 (JVInit まで)
./scripts/jvlink-bridge.sh ping
#    stderr に `JVInit -> 0` が出て終了コード 0 なら OK。
#    失敗時は非 0 で落ちる (ping/set-key は stderr をそのまま返す。
#    fetch だけは stdout を framed binary に保つため stderr を
#    コンテナ内の /tmp/jvlink-bridge.stderr.log に逃がす)。

# 4. 利用キー (17桁) の登録 — JRA-VAN Data Lab. の利用登録で発行されるもの
./scripts/jvlink-bridge.sh set-key <17桁の利用キー>
#    JVSetServiceKey / JVSetSavePath (C:\jvdata) / JVSetSaveFlag(1) を設定する。
#    設定内容は wine prefix のレジストリに残り、ボリュームに永続化される。

# 5. 取得
jvdata backfill --from 2016-01-01 --to 2026-09-21 --source wine
```

## bridge exe のビルド

```sh
# llvm-mingw が要る (apt の gcc-mingw-w64 でも可)。rustup の target 追加だけでは
# gnullvm はリンクできない (self-contained に mingw の .a が無いため)。
rustup target add i686-pc-windows-gnullvm
PATH=/path/to/llvm-mingw/bin:$PATH \
  RUSTFLAGS="-C target-feature=+crt-static" \
  cargo build --release --target i686-pc-windows-gnullvm -p bridge --bin jvlink-bridge
# → target/i686-pc-windows-gnullvm/release/jvlink-bridge.exe
#   compose.yaml がこれを wine コンテナの /opt/jvdata (Z:\opt\jvdata) にマウント
```

`+crt-static` で libunwind.dll 等の動的依存を無くし、単一 exe で動くようにしている。

## エラーコードの読み方 (JV-Link 仕様書より)

| code | 意味 | 対処 |
| --- | --- | --- |
| -1 | 該当データなし | その期間に提供データが無い。bridge は Done を返し、その window は 0 件で done になる |
| -2 | セットアップダイアログでキャンセル | noVNC でダイアログを操作して再実行 (下記「初回ダイアログ」) |
| -303 | 利用キー未設定 | `set-key` で 17 桁キーを登録 |
| -301 | 認証エラー | キー不正 or 複数マシンで同一キー |
| -302 | 利用キー期限切れ | Data Lab. 契約の更新 |
| -211 | レジストリ内容が不正 | `set-key` が save path を入れる |
| -201 | JVInit 未呼出 | bridge の不具合 (JVInit 成功後に JVOpen するので通常は来ない) |

## 初回ダイアログについて

仕様上、セットアップ取得 (option=3/4) は「スタートキットを持っているか」を
尋ねるダイアログが出る。option=4 (ダイアログ無しセットアップ) でも
**初回の 1 度だけ**表示される。headless の wine では <http://localhost:6080/vnc.html>
を開き「スタートキット(CD/DVD-ROM)を持っていない」を選んで進める
(2 回目以降は表示されず、ダイアログがキャンセルされた場合は JVOpen が -2 を返す)。
`set-key` でレジストリに利用キー・保存パスを事前に入れておけば、
ダイアログの入力内容は事前登録済みになる。

## 実環境での現在の状態

- `ping` (CoCreateInstance + JVInit): **成功** (`JVInit -> 0`)
- `fetch` (JVOpen option=4): **-303** (利用キー未設定)
  - COM 呼び出し・frame protocol・エラー伝播までは実機検証済み
  - 残 blocker は利用キーのみ (JRA-VAN への利用登録が必要)
