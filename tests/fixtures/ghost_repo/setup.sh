#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
BENCH_HISTORY_COMMITS="${WHY_BENCH_HISTORY_COMMITS:-0}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/auth.rs <<'EOF'
// This function is never called from anywhere (orphaned after refactor)
pub fn validate_auth_token_legacy(token: &str, session_id: &str) -> bool {
    // security: added after token forgery incident #7890
    !token.is_empty() && token_matches_session(token, session_id)
}

pub fn authenticate(user: &str, password: &str) -> bool {
    check_password_hash(user, password)
}
EOF
git add src/auth.rs
git commit -m "hotfix: add token validation after auth forgery incident #7890" >/dev/null

cat > src/main.rs <<'EOF'
fn main() {
    let user = "alice";
    let pass = "password";
    if authenticate(user, pass) {
        println!("logged in");
    }
}
EOF
git add src/main.rs
git commit -m "feat: add main entry point using authenticate" >/dev/null

for i in $(seq 1 "$BENCH_HISTORY_COMMITS"); do
  cat > src/auth.rs <<EOF
// This function is never called from anywhere (orphaned after refactor)
pub fn validate_auth_token_legacy(token: &str, session_id: &str) -> bool {
    // security: added after token forgery incident #7890
    // benchmark history marker ${i}
    !token.is_empty() && token_matches_session(token, session_id) && token.len() >= ${i}
}

pub fn authenticate(user: &str, password: &str) -> bool {
    check_password_hash(user, password)
}
EOF
  git add src/auth.rs
  git commit -m "security maintenance: refine legacy auth validation ${i}" >/dev/null
done
