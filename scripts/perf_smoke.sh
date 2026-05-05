#!/usr/bin/env bash
set -euo pipefail

cargo run --release -p c_term_core --bin core_perf

cat <<'EOF'

For live app frame/PTY timing, run:

  TERMITE_PERF=1 cargo run --release -p c_term_app

Then stress it inside the Termite window, for example:

  scripts/text_smoke.sh
  seq 1 200000
  find /usr -maxdepth 4 2>/dev/null

Termite prints one `termite-perf ...` line per second to the launching terminal.
EOF
