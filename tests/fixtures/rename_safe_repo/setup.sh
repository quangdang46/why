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
cat > src/payment.rs <<'EOF'
pub struct PaymentService;
pub struct CheckoutOrchestrator;
pub struct AuditLogger;

impl PaymentService {
    pub fn process_payment(amount: f64) -> Result<(), String> {
        if amount <= 0.0 {
            return Err("invalid amount".into());
        }
        charge_stripe(amount)
    }
}

impl CheckoutOrchestrator {
    pub fn complete_checkout(&self, amount: f64) -> Result<(), String> {
        PaymentService::process_payment(amount)?;
        Ok(())
    }
}

impl AuditLogger {
    pub fn audit_payment(&self, amount: f64) {
        let _ = PaymentService::process_payment(amount);
    }
}
EOF
git add src/payment.rs
git commit -m "feat: add payment workflow" >/dev/null

cat > src/payment.rs <<'EOF'
pub struct PaymentService;
pub struct CheckoutOrchestrator;
pub struct AuditLogger;

impl PaymentService {
    pub fn process_payment(amount: f64) -> Result<(), String> {
        // security: validate amount range to prevent duplicate charge exploit
        if amount <= 0.0 || amount > 100_000.0 {
            return Err("invalid amount range".into());
        }
        rate_limit_check("payment")?;
        charge_stripe(amount)
    }
}

impl CheckoutOrchestrator {
    pub fn complete_checkout(&self, amount: f64) -> Result<(), String> {
        PaymentService::process_payment(amount)?;
        Ok(())
    }
}

impl AuditLogger {
    pub fn audit_payment(&self, amount: f64) {
        let _ = PaymentService::process_payment(amount);
    }
}
EOF
git add src/payment.rs
git commit -m "hotfix: harden process_payment after duplicate charge incident #4521" >/dev/null

cat > src/reporting.rs <<'EOF'
use crate::payment::PaymentService;

pub fn replay_payment(amount: f64) -> Result<(), String> {
    PaymentService::process_payment(amount)
}
EOF
git add src/reporting.rs
git commit -m "feat: add payment replay reporting" >/dev/null

if [ "$BENCH_HISTORY_COMMITS" -gt 0 ]; then
  for i in $(seq 1 "$BENCH_HISTORY_COMMITS"); do
    cat > src/payment.rs <<EOF
pub struct PaymentService;
pub struct CheckoutOrchestrator;
pub struct AuditLogger;

impl PaymentService {
    pub fn process_payment(amount: f64) -> Result<(), String> {
        // security: validate amount range to prevent duplicate charge exploit
        // benchmark debt marker ${i}
        if amount <= 0.0 || amount > 100_000.0 {
            return Err("invalid amount range".into());
        }
        rate_limit_check("payment")?;
        charge_stripe(amount)
    }
}

impl CheckoutOrchestrator {
    pub fn complete_checkout(&self, amount: f64) -> Result<(), String> {
        PaymentService::process_payment(amount)?;
        Ok(())
    }
}

impl AuditLogger {
    pub fn audit_payment(&self, amount: f64) {
        let _ = PaymentService::process_payment(amount);
    }
}
EOF
    git add src/payment.rs
    git commit -m "maintenance: revisit payment safeguards ${i}" >/dev/null
  done
fi
