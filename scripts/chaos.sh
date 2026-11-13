#!/usr/bin/env bash
# Chaos run for step 1.15: kill PostgreSQL under load, restart it, and check
# that nothing the client was told was lost and nothing happened twice.
#
#   docker compose up -d
#   scripts/chaos.sh                # 20 cycles
#   CYCLES=3 scripts/chaos.sh       # a quick run
#
# Each cycle:
#   1. `chaos_load load` sends transfers from several workers for
#      LOAD_SECONDS seconds, logging every request and every answer;
#   2. at a random moment in the middle, PostgreSQL is killed with SIGKILL
#      (`docker compose kill`): no shutdown, no checkpoint, crash recovery
#      from the WAL on the next start;
#   3. once the load has ended, PostgreSQL is started again;
#   4. `chaos_load check` compares the log with the journal and retries
#      every request whose outcome the client never learned;
#   5. `mezgeb-cli verify --full` checks the whole ledger.
#
# Any violation or dirty verification stops the run with status 1. Results
# go to target/chaos/results.md, the client's log to target/chaos/requests.log.
#
# It runs against a separate database, mezgeb_chaos, recreated at the start,
# on the compose service's PostgreSQL (the roles from db/init must exist).

set -euo pipefail

CYCLES=${CYCLES:-20}
LOAD_SECONDS=${LOAD_SECONDS:-10}
WORKERS=${WORKERS:-8}
PORT=${PORT:-55432}
DB=mezgeb_chaos
OUT=target/chaos
LOG=$OUT/requests.log
RESULTS=$OUT/results.md

OWNER_URL=postgres://mezgeb_owner:mezgeb_owner@localhost:$PORT/$DB
APP_URL=postgres://mezgeb_app:mezgeb_app@localhost:$PORT/$DB
VERIFIER_URL=postgres://mezgeb_verifier:mezgeb_verifier@localhost:$PORT/$DB

cd "$(dirname "$0")/.."

superuser() {
    docker compose exec -T postgres psql -U postgres -v ON_ERROR_STOP=1 -qtA "$@"
}

# Until PostgreSQL has finished crash recovery and accepts queries.
wait_ready() {
    until superuser -c 'SELECT 1' >/dev/null 2>&1; do
        sleep 0.2
    done
}

echo "building"
cargo build -q --release -p mezgeb-cli
cargo build -q --release -p mezgeb-engine --example chaos_load
LOAD=target/release/examples/chaos_load
VERIFY=target/release/mezgeb-cli

mkdir -p "$OUT"
: >"$LOG"
wait_ready
superuser -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)"
superuser -c "CREATE DATABASE $DB OWNER mezgeb_owner"
$LOAD setup "$OWNER_URL" "$APP_URL"

{
    echo "| Cycle | Killed after | Sent | Acked | Rejected | Unknown: committed / absent | Recovery | Check | Verify |"
    echo "|---:|---:|---:|---:|---:|---:|---:|---|---|"
} >"$RESULTS"

field() { # field <name> <line>: the value of name=value in line
    sed -E "s/.*[[:space:]]$1=([^[:space:]]+).*/\1/" <<<"$2"
}

for cycle in $(seq 1 "$CYCLES"); do
    # Kill somewhere between 2 s in and 2 s before the load ends.
    kill_after=$((2 + RANDOM % (LOAD_SECONDS - 3)))
    $LOAD load "$APP_URL" "$LOG" "$cycle" "$WORKERS" "$LOAD_SECONDS" >"$OUT/load-$cycle.txt" &
    loader=$!
    sleep "$kill_after"
    docker compose kill -s SIGKILL postgres >/dev/null 2>&1
    wait "$loader"
    load_line=$(grep '^LOAD' "$OUT/load-$cycle.txt")

    started=$(date +%s.%N)
    docker compose start postgres >/dev/null 2>&1
    wait_ready
    recovery=$(awk "BEGIN { printf \"%.1f\", $(date +%s.%N) - $started }")

    check_status=pass
    $LOAD check "$APP_URL" "$LOG" "$cycle" >"$OUT/check-$cycle.txt" || check_status=FAIL
    check_line=$(grep '^CHECK' "$OUT/check-$cycle.txt" || echo "CHECK missing")
    verify_status=clean
    $VERIFY verify --full --settle-seconds 0 --database-url "$VERIFIER_URL" \
        >"$OUT/verify-$cycle.txt" 2>&1 || verify_status=DIRTY

    row="| $cycle | ${kill_after} s | $(field sent "$load_line") | $(field acked "$check_line") \
| $(field rejected "$check_line") | $(field unknown_committed "$check_line") / $(field unknown_absent "$check_line") \
| ${recovery} s | $check_status | $verify_status |"
    echo "$row" >>"$RESULTS"
    echo "$row"
    if [[ $check_status != pass || $verify_status != clean ]]; then
        cat "$OUT/check-$cycle.txt" "$OUT/verify-$cycle.txt"
        echo "cycle $cycle failed" >&2
        exit 1
    fi
done

# Every request of every cycle, once more, from the full log.
$LOAD check "$APP_URL" "$LOG" all | tee "$OUT/check-all.txt" | grep -E '^(CHECK|VIOLATION)'
$VERIFY verify --full --settle-seconds 0 --database-url "$VERIFIER_URL" | tee "$OUT/verify-final.txt"
echo "all $CYCLES cycles passed; results in $RESULTS"
