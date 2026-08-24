#!/usr/bin/env bash
# One-shot developer environment bootstrap for this machine.
# Installs everything needed to build FDM from source. Idempotent — re-running
# it is safe, winget skips packages that are already present.
set -u

WG_FLAGS="--accept-source-agreements --accept-package-agreements --disable-interactivity"

step() { echo; echo "=============================================="; echo ">>> $1"; echo "=============================================="; }

step "1/3 Python 3.12 (needed by the ui-ux-pro-max design skill)"
winget install --id Python.Python.3.12 --silent $WG_FLAGS 2>&1
echo "exit=$?"

step "2/3 Rust toolchain (rustup + cargo + rustc)"
winget install --id Rustlang.Rustup --silent $WG_FLAGS 2>&1
echo "exit=$?"

step "3/3 Visual Studio 2022 Build Tools + VCTools workload (the MSVC linker)"
echo "This is the big one — several GB, expect 10-30 minutes."
winget install --id Microsoft.VisualStudio.2022.BuildTools $WG_FLAGS \
  --override "--wait --quiet --norestart --nocache --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended" 2>&1
echo "exit=$?"

step "DONE - verifying"
"$USERPROFILE/.cargo/bin/rustc.exe" --version 2>&1 || echo "rustc NOT found"
"$USERPROFILE/.cargo/bin/cargo.exe" --version 2>&1 || echo "cargo NOT found"
py --version 2>&1 || python --version 2>&1 || echo "python NOT found"
