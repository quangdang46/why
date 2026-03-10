#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/util.rs <<'EOF'
pub fn maybe_trim(value: &str) -> &str {
    value.trim()
}
EOF
git add src/util.rs
git commit -m "refactor: move string helper" >/dev/null
