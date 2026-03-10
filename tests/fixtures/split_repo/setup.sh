#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/auth.rs <<'EOF'
pub fn authenticate(user: &str, token: &str) -> bool {
    check_password(user, token)
}
EOF
git add src/auth.rs
git commit -m "feat: initial auth implementation" >/dev/null

cat > src/auth.rs <<'EOF'
pub fn authenticate(user: &str, token: &str) -> bool {
    // security: added after incident #4521
    if is_rate_limited(user) { return false; }
    if token.is_empty() { return false; }
    let session = new_session(user);
    validate_token_with_session(token, &session)
}
EOF
git add src/auth.rs
git commit -m "hotfix: harden authenticate after auth bypass incident #4521" >/dev/null

cat > src/auth.rs <<'EOF'
pub fn authenticate(user: &str, token: &str) -> bool {
    // security: added after incident #4521
    if is_rate_limited(user) { return false; }
    if token.is_empty() { return false; }

    // backward compat: legacy v1 token format for mobile clients
    if token.starts_with("v1:") {
        return validate_legacy_token(token, user);
    }

    let session = new_session(user);
    validate_token_with_session(token, &session)
}
EOF
git add src/auth.rs
git commit -m "feat: add legacy v1 token support for mobile backward compat (#234)" >/dev/null
