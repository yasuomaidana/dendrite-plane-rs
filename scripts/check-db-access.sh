#!/bin/sh
# Database access policy: every database goes through Diesel.
#
# Fails when
#   * a Cargo.toml dependency section declares sqlx, rusqlite, sea-orm, mysql, mysql_async,
#     postgres, tokio-postgres or libsql and the same manifest declares no diesel crate
#     (diesel, diesel-async, diesel-libsql ... use those drivers as backends, which is fine);
#   * a Rust source file uses rusqlite::, sqlx:: or sea_orm::;
#   * diesel::sql_query / sql_query( appears outside a module named `raw` (raw.rs or raw/).
#
# Exceptions live in .github/db-access-allow.txt, one per line, each with a reason:
#   manifest:<path/to/Cargo.toml> <reason>   the manifest may declare the drivers
#   source:<path/prefix>          <reason>   files under the prefix may use the crates or sql_query
# Lines starting with # and empty lines are ignored. A line without a reason is an error.
#
# Usage: scripts/check-db-access.sh [root]   (default: the current directory)

root=${1:-.}
cd "$root" || { echo "check-db-access: cannot enter $root" >&2; exit 2; }
allow=.github/db-access-allow.txt
fail=0

report() { # file message
    if [ -n "${GITHUB_ACTIONS:-}" ]; then
        echo "::error file=$1::$2"
    fi
    echo "db-access: $1: $2" >&2
    fail=1
}

# Allow-list entries as "kind:value" lines, validated.
entries=""
if [ -f "$allow" ]; then
    n=0
    while IFS= read -r line || [ -n "$line" ]; do
        n=$((n + 1))
        case $line in '' | '#'*) continue ;; esac
        key=${line%% *}
        reason=${line#"$key"}
        reason=$(printf '%s' "$reason" | tr -d '[:space:]')
        case $key in
            manifest:?* | source:?*) ;;
            *) report "$allow" "line $n: expected manifest:<path> or source:<prefix>, got '$key'"; continue ;;
        esac
        if [ -z "$reason" ]; then
            report "$allow" "line $n: '$key' has no reason"
            continue
        fi
        entries="$entries
$key"
    done <"$allow"
fi

allowed() { # kind value: succeeds when an entry of that kind names the value or a directory above it
    printf '%s\n' "$entries" | awk -v k="$1:" -v v="$2" '
        index($0, k) == 1 { p = substr($0, length(k) + 1)
            if (p != "" && (v == p || index(v, p "/") == 1)) found = 1 }
        END { exit !found }'
}

drivers='sqlx|rusqlite|sea-orm|sea_orm|mysql|mysql_async|postgres|tokio-postgres|libsql'

# ---- manifests
find . \( -name target -o -name .git -o -name node_modules \) -prune -o -name Cargo.toml -print |
    sed 's#^\./##' | sort >"${TMPDIR:-/tmp}/dbaccess.$$" || true
while IFS= read -r m; do
    [ -n "$m" ] || continue
    found=$(awk -v drivers="$drivers" '
        BEGIN { n = split(drivers, d, "|"); for (i = 1; i <= n; i++) isdrv[d[i]] = 1 }
        /^[[:space:]]*\[/ {
            h = $0; gsub(/[[:space:]]/, "", h)
            indep = (h ~ /dependencies(\]|\.)/ || h ~ /^\[(dev-|build-)?dependencies\]$/)
            if (h ~ /dependencies\./) { name = h; sub(/^.*dependencies\./, "", name); sub(/\]$/, "", name)
                if (name in isdrv) print name
                if (name ~ /^diesel/) print "diesel:" name }
            next }
        indep && /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*(=|\.)/ {
            k = $0; sub(/^[[:space:]]*/, "", k); sub(/[[:space:]]*(=|\.).*$/, "", k)
            if (k in isdrv) print k
            if (k ~ /^diesel/) print "diesel:" k }
    ' "$m")
    drv=$(printf '%s\n' "$found" | grep -v '^diesel:' | grep -v '^$' | sort -u | tr '\n' ' ')
    hasdiesel=$(printf '%s\n' "$found" | grep -c '^diesel:')
    [ -n "$drv" ] || continue
    if [ "$hasdiesel" -gt 0 ]; then continue; fi
    if allowed manifest "$m"; then continue; fi
    report "$m" "declares ${drv}without diesel; use Diesel (diesel, diesel-async, diesel-libsql) or add a reasoned entry to $allow"
done <"${TMPDIR:-/tmp}/dbaccess.$$"
rm -f "${TMPDIR:-/tmp}/dbaccess.$$"

# ---- sources
srcs=$(find . \( -name target -o -name .git -o -name node_modules \) -prune -o -name '*.rs' -print | sed 's#^\./##' | sort)
for f in $srcs; do
    if grep -qE '(^|[^A-Za-z0-9_])(rusqlite|sqlx|sea_orm)::' "$f"; then
        if ! allowed source "$f"; then
            report "$f" "uses rusqlite/sqlx/sea_orm directly; go through Diesel or add a reasoned source: entry to $allow"
        fi
    fi
    if grep -qE '(^|[^A-Za-z0-9_])sql_query[[:space:]]*(\(|::<)|diesel::sql_query' "$f"; then
        case $f in
            raw.rs | */raw.rs | raw/* | */raw/*) ;;
            *) if ! allowed source "$f"; then
                report "$f" "calls sql_query outside a module named raw (raw.rs or raw/)"
            fi ;;
        esac
    fi
done

if [ "$fail" -ne 0 ]; then
    echo "db-access: FAILED" >&2
    exit 1
fi
echo "db-access: ok"
