# psql / pgcli を入れた運用ツール用イメージ。
# DB コンテナ (docker/postgres) に運用ツールを入れないために分離している。
FROM python:3.13-slim@sha256:9d2e5553305c7c7b0097999bb17187c69b921ccd6bc9d40e4bb5ebe652c00285

# postgresql-client はサーバー (PostgreSQL 18) と同じメジャーを PGDG から入れる。
# Debian 同梱のものは 1 つ古く、接続ごとにバージョン警告が出て、
# pg_dump はメジャー不一致で使えない。
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        gpg \
    && install -d -m 0755 /etc/apt/keyrings \
    && curl -fsSL https://www.postgresql.org/media/keys/ACCC4CF8.asc \
        | gpg --dearmor -o /etc/apt/keyrings/pgdg.gpg - \
    && . /etc/os-release \
    && echo "deb [signed-by=/etc/apt/keyrings/pgdg.gpg] http://apt.postgresql.org/pub/repos/apt ${VERSION_CODENAME}-pgdg main" \
        > /etc/apt/sources.list.d/pgdg.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends \
        less \
        postgresql-client-18 \
    && rm -rf /var/lib/apt/lists/* \
    && pip install --no-cache-dir pgcli
