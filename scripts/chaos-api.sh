#!/usr/bin/env bash
# The step 1.15 chaos run through the API (step 2.07 exit gate): transfers
# over HTTP while PostgreSQL or mezgeb-api is killed, then a check that
# nothing the client was told was lost and nothing happened twice.
#
#   docker compose up -d
#   scripts/chaos-api.sh                # 20 cycles
#   CYCLES=3 scripts/chaos-api.sh       # a quick run
#
# Each cycle:
#   1. `api_chaos load` sends transfers to the service from several workers
#      for LOAD_SECONDS seconds, logging every request and every answer;
#   2. at a random moment in the middle, one of them is killed with SIGKILL:
#      odd cycles PostgreSQL (`docker compose kill`), even cycles the
#      service (no graceful shutdown: requests and transactions in flight
#      are cut off);
#   3. once the load has ended, the killed one is started again;
#   4. `api_chaos check` compares the log with the journal and retries,
#      through the API, every request whose outcome the client never learned;
#   5. `mezgeb-cli verify --full` checks the whole ledger.
#
# Any violation, dirty verification, or kill that missed the load stops the
# run with status 1. Results
# go to target/chaos-api/results.md, the client's log to
# target/chaos-api/requests.log, the service's log to target/chaos-api/api.log.
#
# It runs against a separate database, mezgeb_api_chaos, recreated at the
# start, on the compose service's PostgreSQL (the roles from db/init must
# exist, mezgeb_admin among them), with the development clients and
# credentials of deploy/clients.dev.toml.

set -euo pipefail

CYCLES=${CYCLES:-20}
LOAD_SECONDS=${LOAD_SECONDS:-10}
WORKERS=${WORKERS:-8}
PORT=${PORT:-55432}
API_PORT=${API_PORT:-58080}
DB=mezgeb_api_chaos
OUT=target/chaos-api
LOG=$OUT/requests.log
RESULTS=$OUT/results.md
BASE=http://127.0.0.1:$API_PORT

OWNER_URL=postgres://mezgeb_owner:mezgeb_owner@localhost:$PORT/$DB
APP_URL=postgres://mezgeb_app:mezgeb_app@localhost:$PORT/$DB
VERIFIER_URL=postgres://mezgeb_verifier:mezgeb_verifier@localhost:$PORT/$DB
ADMIN_URL=postgres://mezgeb_admin:mezgeb_admin@localhost:$PORT/$DB

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

API_PID=
start_api() {
    MEZGEB_DATABASE_URL=$APP_URL MEZGEB_LISTEN=127.0.0.1:$API_PORT \
        MEZGEB_CHART=charts/wallet-provider.toml \
        MEZGEB_LOG=warn $API >>"$OUT/api.log" 2>&1 &
    API_PID=$!
    # Up once it answers at all: 401 without a token.
    until curl -s -o /dev/null "$BASE/v1/templates"; do
        if ! kill -0 "$API_PID" 2>/dev/null; then
            echo "mezgeb-api exited; see $OUT/api.log" >&2
            exit 1
        fi
        sleep 0.2
    done
}

kill_api() {
    kill -KILL "$API_PID" 2>/dev/null || true
    wait "$API_PID" 2>/dev/null || true
}
trap 'kill_api' EXIT

echo "building"
cargo build -q --release -p mezgeb-cli -p mezgeb-api
cargo build -q --release -p mezgeb-api --example api_chaos
LOAD=target/release/examples/api_chaos
VERIFY=target/release/mezgeb-cli
API=target/release/mezgeb-api

mkdir -p "$OUT"
: >"$LOG"
: >"$OUT/api.log"
wait_ready
superuser -c "DROP DATABASE IF EXISTS $DB WITH (FORCE)"
superuser -c "CREATE DATABASE $DB OWNER mezgeb_owner"
$VERIFY migrate --database-url "$OWNER_URL" >/dev/null
$VERIFY client seed deploy/clients.dev.toml --database-url "$ADMIN_URL" >/dev/null
start_api
$LOAD setup "$OWNER_URL" "$BASE"

{
    echo "| Cycle | Killed | After | During load | Sent | Acked | Rejected | Unknown: committed / absent | Back in | Check | Verify |"
    echo "|---:|---|---:|---|---:|---:|---:|---:|---:|---|---|"
} >"$RESULTS"

field() { # field <name> <line>: the value of name=value in line
    sed -E "s/.*[[:space:]]$1=([^[:space:]]+).*/\1/" <<<"$2"
}

for cycle in $(seq 1 "$CYCLES"); do
    # Kill somewhere between 2 s in and 2 s before the load ends.
    kill_after=$((2 + RANDOM % (LOAD_SECONDS - 3)))
    if ((cycle % 2)); then target=postgres; else target=api; fi
    $LOAD load "$BASE" "$LOG" "$cycle" "$WORKERS" "$LOAD_SECONDS" >"$OUT/load-$cycle.txt" &
    loader=$!
    sleep "$kill_after"
    # A busy machine can delay the kill past the end of the load; such a
    # cycle tests nothing, and the run fails.
    if kill -0 "$loader" 2>/dev/null; then during=yes; else during=NO; fi
    if [[ $target == postgres ]]; then
        docker compose kill -s SIGKILL postgres >/dev/null 2>&1
    else
        kill_api
    fi
    wait "$loader"
    load_line=$(grep '^LOAD' "$OUT/load-$cycle.txt")

    started=$(date +%s.%N)
    if [[ $target == postgres ]]; then
        docker compose start postgres >/dev/null 2>&1
        wait_ready
    else
        start_api
    fi
    back=$(awk "BEGIN { printf \"%.1f\", $(date +%s.%N) - $started }")

    check_status=pass
    $LOAD check "$APP_URL" "$BASE" "$LOG" "$cycle" >"$OUT/check-$cycle.txt" || check_status=FAIL
    check_line=$(grep '^CHECK' "$OUT/check-$cycle.txt" || echo "CHECK missing")
    verify_status=clean
    $VERIFY verify --full --settle-seconds 0 --database-url "$VERIFIER_URL" \
        >"$OUT/verify-$cycle.txt" 2>&1 || verify_status=DIRTY

    row="| $cycle | $target | ${kill_after} s | $during | $(field sent "$load_line") | $(field acked "$check_line") \
| $(field rejected "$check_line") | $(field unknown_committed "$check_line") / $(field unknown_absent "$check_line") \
| ${back} s | $check_status | $verify_status |"
    echo "$row" >>"$RESULTS"
    echo "$row"
    if [[ $check_status != pass || $verify_status != clean || $during != yes ]]; then
        cat "$OUT/check-$cycle.txt" "$OUT/verify-$cycle.txt"
        echo "cycle $cycle failed" >&2
        exit 1
    fi
done

# Every request of every cycle, once more, from the full log.
$LOAD check "$APP_URL" "$BASE" "$LOG" all | tee "$OUT/check-all.txt" | grep -E '^(CHECK|VIOLATION)'
$VERIFY verify --full --settle-seconds 0 --database-url "$VERIFIER_URL" | tee "$OUT/verify-final.txt"
echo "all $CYCLES cycles passed; results in $RESULTS"
