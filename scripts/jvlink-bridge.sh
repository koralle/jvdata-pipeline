#!/usr/bin/env bash
# jvdata から呼ばれる bridge エントリポイント。
# compose の wine コンテナ内で jvlink-bridge.exe (32-bit Windows exe) を実行する。
#
# 前提:
#   - wine サービスが起動済み (mise run up)
#   - jvlink-bridge.exe が ./target/i686-pc-windows-gnullvm/release/ にある
#     (compose.yaml が /opt/jvdata にマウントする → Z:\opt\jvdata\)
#   - JV-Link (JVDTLab.dll) が wine prefix に登録済み (docs/jv-link-setup.md)
#
# docker compose exec ではなく docker exec を使う:
#   - compose は全 env の補間が要る (varlock 経由になり、その層が binary stdout を
#     UTF-8 化して壊す)。docker exec はコンテナ名だけで済み binary が素通りする。
#   - fetch では wine の fixme/err ログは stderr に混ざるのでコンテナ内で
#     ファイルに逃がし、stdout を framed binary 専用に保つ
#     (log は /tmp/jvlink-bridge.stderr.log)。exit code は捨てる: 実エラーは
#     Error frame が持つので、ここで非 0 を返しても情報が増えない。
#   - ping / set-key など対話コマンドでは stderr をそのまま呼び出し側に返し、
#     exit code も伝える (疎通確認が黙って成功扱いにならないように)。
set -euo pipefail

CONTAINER="${JVDATA_WINE_CONTAINER:-jvdata-pipeline-wine-1}"

# 引数を wine 側コマンドラインに安全に乗せる (dataspec/fromtime/option/ファイル名)
# パスは forward slash: sh -c 経由だとバックスラッシュがエスケープ解釈で消える。
inner="wine Z:/opt/jvdata/jvlink-bridge.exe"
for arg in "$@"; do
  case "$arg" in
    *[!A-Za-z0-9_.:/=-]*) echo "bad arg: $arg" >&2; exit 2 ;;
  esac
  inner="$inner $arg"
done

# -i < /dev/null が要る: stdin を明示しないと wine 側プロセスが入力待ちで
# stdout を流さないことがある (docker exec の stdin アタッチの癖)。
# JVDATA_SID (JVInit に渡すソフトウェア ID) は設定時のみコンテナへ引き渡す
# (未設定なら bridge exe 側の既定 "UNKNOWN" が使われる)。
exec_env=(-i)
if [ -n "${JVDATA_SID:-}" ]; then
  exec_env+=(-e "JVDATA_SID=$JVDATA_SID")
fi
case "${1:-}" in
  fetch)
    exec docker exec "${exec_env[@]}" "$CONTAINER" sh -c \
      "$inner 2>/tmp/jvlink-bridge.stderr.log; exit 0" < /dev/null
    ;;
  *)
    exec docker exec "${exec_env[@]}" "$CONTAINER" sh -c "$inner" < /dev/null
    ;;
esac
