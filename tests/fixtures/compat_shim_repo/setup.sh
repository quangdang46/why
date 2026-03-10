#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/http.rs <<'EOF'
pub fn set_charset(content_type: &str) -> String {
    format!("{}; charset=utf-8", content_type)
}
EOF
git add src/http.rs
git commit -m "feat: normalize content type headers" >/dev/null

cat > src/http.rs <<'EOF'
pub fn set_charset(content_type: &str) -> String {
    // backward compat: older mobile clients require explicit charset values
    if content_type.contains("text/") {
        return format!("{}; charset=utf-8", content_type);
    }

    content_type.to_string()
}
EOF
git add src/http.rs
git commit -m "fix: preserve explicit charset for legacy mobile clients (#318)" >/dev/null
