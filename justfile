# 開発環境をセットアップする (Rust toolchain + 開発ツールを mise で導入)
setup:
  #!/usr/bin/env bash
  set -euo pipefail

  if ! command -v mise >/dev/null 2>&1; then
    echo "mise が必要です: https://mise.jdx.dev/" >&2
    exit 1
  fi

  mise install
  echo "セットアップ完了: just ci で全チェックを実行できます"

check:
  @cargo fmt --check
  @cargo clippy --all-targets --all-features -- -D warnings

lint:
  @cargo clippy --all-targets --all-features -- -D warnings

format:
  @cargo fmt

test:
  @cargo nextest run --workspace --all-features

# 依存クレートのポリシーチェック (脆弱性 / ライセンス / 禁止事項 / 取得元)
deny:
  @cargo deny --all-features --locked check

# 未使用クレートの検出
machete:
  @cargo machete

ci:
  cargo fmt --all -- --check
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  cargo nextest run --workspace --all-features
  cargo test --doc --workspace --all-features
  cargo machete
  cargo deny --all-features --locked check

