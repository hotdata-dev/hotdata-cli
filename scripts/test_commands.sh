#!/usr/bin/env bash
# Comprehensive functional test suite for the hotdata CLI.
# Tests each command and subcommand with table/json/yaml outputs,
# edge cases, and flag variations. Creates and cleans up real resources.
#
# Usage:
#   ./scripts/test_commands.sh                   # uses 'hotdata' from PATH
#   HOTDATA_BIN=./target/debug/hotdata ./scripts/test_commands.sh

set -uo pipefail

# Resolve BIN to an absolute path so pushd/popd doesn't break relative paths.
_raw_bin="${HOTDATA_BIN:-hotdata}"
if [[ "$_raw_bin" == ./* || "$_raw_bin" == /* ]]; then
    BIN="$(cd "$(dirname "$_raw_bin")" && pwd)/$(basename "$_raw_bin")"
else
    BIN="$_raw_bin"
fi
unset _raw_bin
PASS=0
FAIL=0
SKIP=0

# Resources created during the run — cleaned up on exit.
CREATED_SANDBOX=""
CREATED_DB=""
CREATED_DATASET=""
CONTEXT_TMPDIR=""

# ── Colors ────────────────────────────────────────────────────────────────────
RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
GREY='\033[0;90m'; BOLD='\033[1m'; NC='\033[0m'

# ── Helpers ───────────────────────────────────────────────────────────────────
section() { echo; echo -e "${BOLD}=== $* ===${NC}"; }

_pass() { echo -e "  ${GREEN}✓${NC} $1"; PASS=$((PASS+1)); }
_fail() {
    echo -e "  ${RED}✗${NC} $1"
    [[ -n "${2:-}" ]] && echo -e "${GREY}    $2${NC}"
    FAIL=$((FAIL+1))
}
_skip() { echo -e "  ${YELLOW}~${NC} $1 (skipped)"; SKIP=$((SKIP+1)); }

# Run a command, check its exit code matches $expected (default 0).
check() {
    local name="$1" expected="${2:-0}"; shift 2
    local actual=0 out
    out=$("$@" 2>&1) || actual=$?
    if [[ "$actual" -eq "$expected" ]]; then
        _pass "$name"
    else
        _fail "$name → exit $actual (expected $expected)" \
              "cmd: $* | out: $(echo "$out" | head -2 | tr '\n' ' ')"
    fi
}

# Run a command; verify stdout is valid JSON.
check_json() {
    local name="$1"; shift
    local actual=0 out
    out=$("$@" 2>/dev/null) || actual=$?
    if [[ "$actual" -ne 0 ]]; then
        _fail "$name → non-zero exit ($actual)"
        return
    fi
    if echo "$out" | jq . > /dev/null 2>&1; then
        _pass "$name"
    else
        _fail "$name → invalid JSON" "out: $(echo "$out" | head -1)"
    fi
}

# Run a command; verify stdout is valid YAML.
check_yaml() {
    local name="$1"; shift
    local actual=0 out
    out=$("$@" 2>/dev/null) || actual=$?
    if [[ "$actual" -ne 0 ]]; then
        _fail "$name → non-zero exit ($actual)"
        return
    fi
    if python3 -c "import sys,yaml; yaml.safe_load(sys.stdin)" <<< "$out" > /dev/null 2>&1; then
        _pass "$name"
    else
        _fail "$name → invalid YAML" "out: $(echo "$out" | head -1)"
    fi
}

# Run a command that should output non-empty text to stdout.
check_nonempty() {
    local name="$1"; shift
    local actual=0 out
    out=$("$@" 2>/dev/null) || actual=$?
    if [[ "$actual" -ne 0 ]]; then
        _fail "$name → non-zero exit ($actual)"
    elif [[ -z "$out" ]]; then
        _fail "$name → empty output"
    else
        _pass "$name"
    fi
}

# Capture a jq value from a command's JSON stdout into a variable.
# capture_into VARNAME JQ_PATH CMD [args...]
capture_into() {
    local varname="$1" jqpath="$2"; shift 2
    local out
    out=$("$@" 2>/dev/null) || return 1
    local val
    val=$(echo "$out" | jq -r "$jqpath" 2>/dev/null) || return 1
    [[ "$val" == "null" || -z "$val" ]] && return 1
    printf -v "$varname" '%s' "$val"
}

# ── Cleanup ───────────────────────────────────────────────────────────────────
cleanup() {
    echo
    section "Cleanup"
    if [[ -n "$CREATED_SANDBOX" ]]; then
        "$BIN" sandbox set 2>/dev/null || true
        if "$BIN" sandbox delete "$CREATED_SANDBOX" 2>/dev/null; then
            echo "  deleted sandbox $CREATED_SANDBOX"
        else
            echo -e "  ${YELLOW}warning: failed to delete sandbox $CREATED_SANDBOX${NC}"
        fi
    fi
    if [[ -n "$CREATED_DATASET" ]]; then
        echo "  note: dataset $CREATED_DATASET has no delete command — left in place"
    fi
    if [[ -n "$CREATED_DB" ]]; then
        if "$BIN" databases delete "$CREATED_DB" 2>/dev/null; then
            echo "  deleted database $CREATED_DB"
        else
            echo -e "  ${YELLOW}warning: failed to delete database $CREATED_DB${NC}"
        fi
    fi
    if [[ -n "$CONTEXT_TMPDIR" ]]; then
        rm -rf "$CONTEXT_TMPDIR"
    fi
    echo
    echo -e "${BOLD}Results: ${GREEN}${PASS} passed${NC}  ${RED}${FAIL} failed${NC}  ${YELLOW}${SKIP} skipped${NC}"
    [[ $FAIL -eq 0 ]]
}
trap cleanup EXIT

# ── Verify binary exists ──────────────────────────────────────────────────────
if ! command -v "$BIN" > /dev/null 2>&1 && [[ ! -x "$BIN" ]]; then
    echo "error: binary not found: $BIN"
    exit 1
fi
echo -e "${BOLD}hotdata CLI functional test suite${NC}"
echo "binary: $(command -v "$BIN" 2>/dev/null || echo "$BIN")"
echo "version: $("$BIN" --version 2>&1 | head -1)"

# ─────────────────────────────────────────────────────────────────────────────
# AUTH
# ─────────────────────────────────────────────────────────────────────────────
section "auth"
check          "auth status"    0  $BIN auth status

# ─────────────────────────────────────────────────────────────────────────────
# WORKSPACES
# ─────────────────────────────────────────────────────────────────────────────
section "workspaces"
check          "workspaces list (table)"  0  $BIN workspaces list
check_json     "workspaces list (json)"      $BIN workspaces list -o json
check_yaml     "workspaces list (yaml)"      $BIN workspaces list -o yaml

WS_ID=""
capture_into WS_ID '.[0].public_id' $BIN workspaces list -o json || true
if [[ -n "$WS_ID" ]]; then
    check "workspaces set <valid_id>"   0  $BIN workspaces set "$WS_ID"
    check "workspaces set <invalid_id>" 1  $BIN workspaces set "ws_doesnotexist_99999" --no-input
else
    _skip "workspaces set (no workspace found)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# CONNECTIONS
# ─────────────────────────────────────────────────────────────────────────────
section "connections"
check      "connections list (table)"  0  $BIN connections list
check_json "connections list (json)"      $BIN connections list -o json
check_yaml "connections list (yaml)"      $BIN connections list -o yaml

CONN_ID=""
capture_into CONN_ID '.[0].id' $BIN connections list -o json || true
if [[ -n "$CONN_ID" ]]; then
    check      "connections get <id> (table)"  0  $BIN connections "$CONN_ID"
    check_json "connections get <id> (json)"      $BIN connections "$CONN_ID" -o json
    check_yaml "connections get <id> (yaml)"      $BIN connections "$CONN_ID" -o yaml
    check      "connections get <invalid_id>"  1  $BIN connections "conn_doesnotexist_99999"
else
    _skip "connections get (no connections)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# TABLES
# ─────────────────────────────────────────────────────────────────────────────
section "tables"
check      "tables list (table)"  0  $BIN tables list
check_json "tables list (json)"      $BIN tables list -o json
check_yaml "tables list (yaml)"      $BIN tables list -o yaml
if [[ -n "$CONN_ID" ]]; then
    check      "tables list --connection-id (table)"  0  $BIN tables list --connection-id "$CONN_ID"
    check_json "tables list --connection-id (json)"      $BIN tables list --connection-id "$CONN_ID" -o json
else
    _skip "tables list --connection-id (no connections)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# DATABASES
# ─────────────────────────────────────────────────────────────────────────────
section "databases"
check      "databases list (table)"  0  $BIN databases list
check_json "databases list (json)"      $BIN databases list -o json
check_yaml "databases list (yaml)"      $BIN databases list -o yaml

# Create a test database
DB_OUT=""
DB_OUT=$($BIN databases create --description "hotdata-cli test $(date +%s)" -o json 2>/dev/null) || true
if echo "$DB_OUT" | jq -e '.id' > /dev/null 2>&1; then
    CREATED_DB=$(echo "$DB_OUT" | jq -r '.id')
    _pass "databases create (json)"

    check      "databases show <id> (table)"   0  $BIN databases show "$CREATED_DB"
    check_json "databases show <id> (json)"       $BIN databases "$CREATED_DB" -o json
    check_yaml "databases show <id> (yaml)"       $BIN databases "$CREATED_DB" -o yaml
    check      "databases show <invalid_id>"   1  $BIN databases show "db_doesnotexist_99999"

    check "databases tables <id> shorthand"  0  $BIN databases tables "$CREATED_DB"
    check_json "databases tables list (json)"   $BIN databases tables list --database "$CREATED_DB" -o json
    check_yaml "databases tables list (yaml)"   $BIN databases tables list --database "$CREATED_DB" -o yaml

    check "databases set <id>"  0  $BIN databases set "$CREATED_DB"
else
    _fail "databases create (could not create test database)"
    _skip "databases show / tables / set (no test database)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# QUERY
# ─────────────────────────────────────────────────────────────────────────────
section "query"
check          "query SELECT (table)"  0  $BIN query "SELECT 1 AS n, 'hello' AS s"
check_json     "query SELECT (json)"      $BIN query "SELECT 1 AS n" -o json
check          "query SELECT (csv)"    0  $BIN query "SELECT 1 AS n" -o csv
check          "query invalid SQL"     1  $BIN query "THIS IS NOT VALID SQL !!!"
check          "query multiline"       0  $BIN query "SELECT 1 AS a, 2 AS b, 3 AS c"
if [[ -n "$CREATED_DB" ]]; then
    check "query -d <db_id>"  0  $BIN query "SELECT 1" -d "$CREATED_DB"
fi

# ─────────────────────────────────────────────────────────────────────────────
# QUERIES (run history)
# ─────────────────────────────────────────────────────────────────────────────
section "queries"
check      "queries list (table)"           0  $BIN queries list
check_json "queries list (json)"               $BIN queries list -o json
check_yaml "queries list (yaml)"               $BIN queries list -o yaml
check      "queries list --status running"  0  $BIN queries list --status running
check      "queries list --status failed"   0  $BIN queries list --status failed
check      "queries list --limit 5"         0  $BIN queries list --limit 5

QUERY_RUN_ID=""
capture_into QUERY_RUN_ID '.[0].id' $BIN queries list -o json || true
if [[ -n "$QUERY_RUN_ID" ]]; then
    check      "queries get <id> (table)"  0  $BIN queries "$QUERY_RUN_ID"
    check_json "queries get <id> (json)"      $BIN queries "$QUERY_RUN_ID" -o json
    check_yaml "queries get <id> (yaml)"      $BIN queries "$QUERY_RUN_ID" -o yaml
else
    _skip "queries get (no runs in history)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# RESULTS
# ─────────────────────────────────────────────────────────────────────────────
section "results"
check      "results list (table)"  0  $BIN results list
check_json "results list (json)"      $BIN results list -o json
check_yaml "results list (yaml)"      $BIN results list -o yaml
check      "results list --limit 5"  0  $BIN results list --limit 5

RESULT_ID=""
capture_into RESULT_ID '.[0].id' $BIN results list -o json || true
if [[ -n "$RESULT_ID" ]]; then
    check      "results get <id> (table)"  0  $BIN results "$RESULT_ID"
    check_json "results get <id> (json)"      $BIN results "$RESULT_ID" -o json
else
    _skip "results get (no stored results)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# JOBS
# ─────────────────────────────────────────────────────────────────────────────
section "jobs"
check      "jobs list (table)"          0  $BIN jobs list
check_json "jobs list (json)"              $BIN jobs list -o json
check_yaml "jobs list (yaml)"              $BIN jobs list -o yaml
check      "jobs list --status running" 0  $BIN jobs list --status running
check      "jobs list --all"            0  $BIN jobs list --all
check      "jobs list --all (json)"     0  $BIN jobs list --all -o json  # just check exit

JOB_ID=""
capture_into JOB_ID '.[0].id' $BIN jobs list --all -o json || true
if [[ -n "$JOB_ID" ]]; then
    check      "jobs get <id> (table)"  0  $BIN jobs "$JOB_ID"
    check_json "jobs get <id> (json)"      $BIN jobs "$JOB_ID" -o json
    check_yaml "jobs get <id> (yaml)"      $BIN jobs "$JOB_ID" -o yaml
else
    _skip "jobs get (no jobs in history)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# DATASETS
# ─────────────────────────────────────────────────────────────────────────────
section "datasets"
check      "datasets list (table)"  0  $BIN datasets list
check_json "datasets list (json)"      $BIN datasets list -o json
check_yaml "datasets list (yaml)"      $BIN datasets list -o yaml
check      "datasets list --limit 5"  0  $BIN datasets list --limit 5

# Create a test dataset from SQL — use -o json to detect success and capture ID
DS_NAME="cli_test_$(date +%s)"
DS_JSON=""
DS_JSON=$($BIN datasets create --name "$DS_NAME" --sql "SELECT 1 AS n" -o json 2>/dev/null) || true
if echo "$DS_JSON" | jq -e '.id' > /dev/null 2>&1; then
    CREATED_DATASET=$(echo "$DS_JSON" | jq -r '.id')
    _pass "datasets create --sql -o json"

    check      "datasets create --sql (table)"  0  $BIN datasets create --name "${DS_NAME}_t" --sql "SELECT 1"
    check_yaml "datasets create --sql (yaml)"      $BIN datasets create --name "${DS_NAME}_y" --sql "SELECT 1" -o yaml

    check      "datasets get <id> (table)"  0  $BIN datasets "$CREATED_DATASET"
    check_json "datasets get <id> (json)"      $BIN datasets "$CREATED_DATASET" -o json
    check_yaml "datasets get <id> (yaml)"      $BIN datasets "$CREATED_DATASET" -o yaml

    check      "datasets update --description (table)"  0  $BIN datasets update "$CREATED_DATASET" --description "updated label"
    check_json "datasets update --description (json)"      $BIN datasets update "$CREATED_DATASET" --description "updated again" -o json
    check_yaml "datasets update --description (yaml)"      $BIN datasets update "$CREATED_DATASET" --description "updated yaml" -o yaml
    check      "datasets update --name"  0  $BIN datasets update "$CREATED_DATASET" --name "${DS_NAME}_renamed"
    check      "datasets update (missing flags)"  1  $BIN datasets update "$CREATED_DATASET"

    check "datasets refresh"         0  $BIN datasets refresh "$CREATED_DATASET"
    check "datasets refresh --async" 0  $BIN datasets refresh "$CREATED_DATASET" --async
else
    _fail "datasets create (non-zero exit or invalid JSON)"
    _skip "datasets get / update / refresh (no test dataset)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# INDEXES
# ─────────────────────────────────────────────────────────────────────────────
section "indexes"
check      "indexes list (table)"  0  $BIN indexes list
check_json "indexes list (json)"      $BIN indexes list -o json
check_yaml "indexes list (yaml)"      $BIN indexes list -o yaml
if [[ -n "$CONN_ID" ]]; then
    check      "indexes list --connection-id"        0  $BIN indexes list --connection-id "$CONN_ID"
    check_json "indexes list --connection-id (json)"   $BIN indexes list --connection-id "$CONN_ID" -o json
fi

# ─────────────────────────────────────────────────────────────────────────────
# EMBEDDING PROVIDERS
# ─────────────────────────────────────────────────────────────────────────────
section "embedding-providers"
check      "embedding-providers list (table)"  0  $BIN embedding-providers list
check_json "embedding-providers list (json)"      $BIN embedding-providers list -o json
check_yaml "embedding-providers list (yaml)"      $BIN embedding-providers list -o yaml

EP_ID=""
capture_into EP_ID '.[0].id' $BIN embedding-providers list -o json || true
if [[ -n "$EP_ID" ]]; then
    check      "embedding-providers get <id> (table)"  0  $BIN embedding-providers get "$EP_ID"
    check_json "embedding-providers get <id> (json)"      $BIN embedding-providers get "$EP_ID" -o json
    check_yaml "embedding-providers get <id> (yaml)"      $BIN embedding-providers get "$EP_ID" -o yaml
else
    _skip "embedding-providers get (none configured)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# SANDBOX
# ─────────────────────────────────────────────────────────────────────────────
section "sandbox"
check      "sandbox list (table)"  0  $BIN sandbox list
check_json "sandbox list (json)"      $BIN sandbox list -o json
check_yaml "sandbox list (yaml)"      $BIN sandbox list -o yaml

# Create a test sandbox, capture its ID via JSON output
SB_OUT=""
SB_OUT=$($BIN sandbox new --name "cli-test-$(date +%s)" -o json 2>/dev/null) || true
if echo "$SB_OUT" | jq -e '.public_id' > /dev/null 2>&1; then
    CREATED_SANDBOX=$(echo "$SB_OUT" | jq -r '.public_id')
    _pass "sandbox new -o json (public_id parseable)"

    check      "sandbox get <id> (table)"  0  $BIN sandbox "$CREATED_SANDBOX"
    check_json "sandbox get <id> (json)"      $BIN sandbox "$CREATED_SANDBOX" -o json
    check_yaml "sandbox get <id> (yaml)"      $BIN sandbox "$CREATED_SANDBOX" -o yaml
    check      "sandbox get <invalid_id>"  1  $BIN sandbox "s_doesnotexist_99999"

    check "sandbox update --name"     0  $BIN sandbox update "$CREATED_SANDBOX" --name "cli-test-updated"
    check "sandbox update --markdown" 0  $BIN sandbox update "$CREATED_SANDBOX" --markdown "# Test\n\nHello world"
    check "sandbox update (missing flags)" 1  $BIN sandbox update "$CREATED_SANDBOX"
    check_json "sandbox update (json)" $BIN sandbox update "$CREATED_SANDBOX" --name "cli-test-json" -o json

    check "sandbox read (active)"  0  $BIN sandbox "$CREATED_SANDBOX" read

    check "sandbox set <id>"   0  $BIN sandbox set "$CREATED_SANDBOX"
    check "sandbox set (clear)" 0  $BIN sandbox set

    # Delete — cleaned up here so cleanup() doesn't try again
    check "sandbox delete <id>"  0  $BIN sandbox delete "$CREATED_SANDBOX"
    CREATED_SANDBOX=""
else
    _fail "sandbox new (could not create or parse public_id)"
    _skip "sandbox get / update / read / set / delete (no test sandbox)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# CONTEXT
# ─────────────────────────────────────────────────────────────────────────────
section "context"
if [[ -n "$CREATED_DB" ]]; then
    check      "context list (table)"  0  $BIN context list -d "$CREATED_DB"
    check_json "context list (json)"      $BIN context list -d "$CREATED_DB" -o json
    check_yaml "context list (yaml)"      $BIN context list -d "$CREATED_DB" -o yaml
    check      "context list --prefix"  0  $BIN context list -d "$CREATED_DB" --prefix "TESTCTX"

    CONTEXT_TMPDIR=$(mktemp -d)
    echo "# Test context" > "$CONTEXT_TMPDIR/TESTCTX.md"
    echo "This is a CLI test context entry." >> "$CONTEXT_TMPDIR/TESTCTX.md"

    pushd "$CONTEXT_TMPDIR" > /dev/null
    check "context push --dry-run"  0  $BIN context push TESTCTX --dry-run -d "$CREATED_DB"
    check "context push"            0  $BIN context push TESTCTX -d "$CREATED_DB"
    check "context push (update)"   0  $BIN context push TESTCTX -d "$CREATED_DB"
    check "context show"            0  $BIN context show TESTCTX -d "$CREATED_DB"
    check "context show .md suffix" 0  $BIN context show TESTCTX.md -d "$CREATED_DB"
    rm TESTCTX.md
    check "context pull --dry-run"          0  $BIN context pull TESTCTX --dry-run -d "$CREATED_DB"
    check "context pull"                    0  $BIN context pull TESTCTX -d "$CREATED_DB"
    check "context pull (exists, no force)" 1  $BIN context pull TESTCTX -d "$CREATED_DB"
    check "context pull --force"            0  $BIN context pull TESTCTX --force -d "$CREATED_DB"
    popd > /dev/null

    check "context show (nonexistent)"  1  $BIN context show nonexistent_ctx_xyz -d "$CREATED_DB"
    check "context push (reserved word)" 1  $BIN context push select -d "$CREATED_DB"
else
    _skip "context tests (no test database created)"
fi

# ─────────────────────────────────────────────────────────────────────────────
# SKILLS
# ─────────────────────────────────────────────────────────────────────────────
section "skills"
check "skills status"  0  $BIN skills status
check "skills list"    0  $BIN skills list

# ─────────────────────────────────────────────────────────────────────────────
# COMPLETIONS
# ─────────────────────────────────────────────────────────────────────────────
section "completions"
check_nonempty "completions bash"  $BIN completions bash
check_nonempty "completions zsh"   $BIN completions zsh
check_nonempty "completions fish"  $BIN completions fish

# ─────────────────────────────────────────────────────────────────────────────
# GLOBAL FLAGS
# ─────────────────────────────────────────────────────────────────────────────
section "global flags"
check "version flag short"  0  $BIN -v
check "version flag long"   0  $BIN --version
check "help flag"           0  $BIN --help
check "no-input flag"       0  $BIN workspaces list --no-input
