# PostgreSQL 18 (Debian)。
#
# alpine 版の musl にはロケールデータが無く、lc_time などを設定しても
# 無言で効かない (to_char の TMMonth が英語のままになる)。日本語の月日名を
# DB 側で出すために Debian 版を使い、ja_JP.UTF-8 をイメージに焼き込む。
#
# ロケールはイメージに入れる必要がある。コンテナを作り直すと消えるので、
# 起動時 (init スクリプト) に生成しても間に合わない。
#
# ダイジェストは arm64 / amd64 両方を含むインデックスのもの。
FROM postgres:18@sha256:4ef4dbc939d61acea57712655ddb4b4ab27419c913f94cca0cd57cb3ea3c2280

RUN localedef -i ja_JP -c -f UTF-8 -A /usr/share/locale/locale.alias ja_JP.UTF-8
