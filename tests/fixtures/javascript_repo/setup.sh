#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/auth.js <<'EOF'
class AuthService {
  login(token) {
    return Boolean(token)
  }
}

module.exports = { AuthService }
EOF
git add src/auth.js
git commit -m "feat: add auth service" >/dev/null

cat > src/auth.js <<'EOF'
class AuthService {
  login(token) {
    if (!token || token.length < 8) {
      return false
    }

    return token.startsWith("sk-")
  }
}

module.exports = { AuthService }
EOF
git add src/auth.js
git commit -m "hotfix: prevent weak token login bypass" >/dev/null
