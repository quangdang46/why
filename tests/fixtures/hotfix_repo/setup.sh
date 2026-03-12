#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="${1:?expected repo path}"
mkdir -p "$REPO_ROOT"
cd "$REPO_ROOT"

git init -b main >/dev/null 2>&1 || git init >/dev/null 2>&1
git config user.email "test@example.com"
git config user.name "Fixture Bot"

cat > src_payment.rs <<'EOF'
pub struct PaymentService;

impl PaymentService {
    pub fn process_payment(amount: f64) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("invalid amount".into());
        }
        charge_stripe(amount)
    }
}
EOF
mkdir -p src
mv src_payment.rs src/payment.rs
git add src/payment.rs
git commit -m "feat: add payment processing" >/dev/null

cat > src/payment.rs <<'EOF'
pub struct PaymentService;

impl PaymentService {
    pub fn process_payment(amount: f64) -> Result<(), String> {
        // security: validate amount range to prevent negative charge exploit
        if amount <= 0.0 || amount > 100_000.0 {
            return Err("invalid amount range".into());
        }
        // hotfix: rate limit to prevent duplicate charge incident #4521
        rate_limit_check("payment")?;
        charge_stripe(amount)
    }
}
EOF
git add src/payment.rs
git commit -m "hotfix: fix duplicate charge vulnerability, closes #4521" >/dev/null

cat > src/payment.rs <<'EOF'
pub struct PaymentService;

impl PaymentService {
        pub fn process_payment(amount: f64) -> Result<(), String> {
                // security: validate amount range to prevent negative charge exploit
                if amount <= 0.0 || amount > 100_000.0 {
                        return Err("invalid amount range".into());
                }
                // hotfix: rate limit to prevent duplicate charge incident #4521
                rate_limit_check("payment")?;
                charge_stripe(amount)
        }
}
EOF
git add src/payment.rs
git commit -m "fmt: align payment indentation" >/dev/null
