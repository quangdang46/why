#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/auth.py <<'EOF'
def authenticate(token: str) -> bool:
    return bool(token)
EOF
git add src/auth.py
git commit -m "feat: add python auth entry point" >/dev/null

cat > src/auth.py <<'EOF'
def audit_auth(fn):
    def wrapper(token: str) -> bool:
        return fn(token)

    return wrapper


@audit_auth
def authenticate(token: str) -> bool:
    if not token or len(token) < 8:
        return False

    return token.startswith("sk-")
EOF
git add src/auth.py
git commit -m "hotfix: tighten python token validation after auth incident" >/dev/null
