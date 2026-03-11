#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

mkdir -p src
cat > src/auth.ts <<'EOF'
export function authenticate(token: string): boolean {
    return token.length > 0;
}
EOF
git add src/auth.ts
git commit -m "feat: add auth entry point" >/dev/null

cat > src/auth.ts <<'EOF'
export function authenticate(token: string): boolean {
    if (!token || token.length < 8) {
        return false;
    }

    return token.startsWith("sk-");
}
EOF
git add src/auth.ts
git commit -m "hotfix: tighten token validation after auth incident" >/dev/null
