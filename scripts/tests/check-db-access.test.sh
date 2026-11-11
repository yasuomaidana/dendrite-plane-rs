#!/bin/sh
# Self-test of scripts/check-db-access.sh with passing and failing fixture trees.
# Usage: scripts/tests/check-db-access.test.sh
here=$(cd "$(dirname "$0")" && pwd)
guard="$here/../check-db-access.sh"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
failures=0
n=0

# case <name> <expected: pass|fail> -- the fixture is built by the shell snippet on stdin-less $3
run_case() {
    name=$1 expect=$2 build=$3
    n=$((n + 1))
    dir="$tmp/case$n"
    mkdir -p "$dir/src" "$dir/.github"
    (cd "$dir" && eval "$build")
    if sh "$guard" "$dir" >/dev/null 2>&1; then got=pass; else got=fail; fi
    if [ "$got" = "$expect" ]; then
        echo "ok   $name"
    else
        echo "FAIL $name (expected $expect, got $got)"
        failures=$((failures + 1))
    fi
}

run_case "no database at all" pass \
    'printf "[package]\nname = \"a\"\n[dependencies]\nserde = \"1\"\n" > Cargo.toml; echo "fn main(){}" > src/main.rs'

run_case "feature named postgres is not a driver" pass \
    'printf "[package]\nname = \"a\"\n[features]\npostgres = [\"x/postgres\"]\nsqlx = []\n[dependencies]\nx = \"1\"\n" > Cargo.toml'

run_case "diesel with its drivers" pass \
    'printf "[dependencies]\ndiesel = \"2\"\ndiesel-async = \"0.9\"\ntokio-postgres = \"0.7\"\nlibsql = \"0.10\"\n" > Cargo.toml'

run_case "workspace dependencies with diesel" pass \
    'printf "[workspace.dependencies]\ndiesel = { version = \"2\" }\ntokio-postgres = \"0.7\"\n" > Cargo.toml'

run_case "sqlx without diesel" fail \
    'printf "[dependencies]\nsqlx = \"0.8\"\n" > Cargo.toml'

run_case "rusqlite table form without diesel" fail \
    'printf "[dependencies.rusqlite]\nversion = \"0.32\"\n" > Cargo.toml'

run_case "dotted dependency without diesel" fail \
    'printf "[dependencies]\nlibsql.version = \"0.10\"\n" > Cargo.toml'

run_case "dev-dependency postgres without diesel" fail \
    'printf "[dev-dependencies]\npostgres = \"0.19\"\n" > Cargo.toml'

run_case "sea-orm without diesel" fail \
    'printf "[dependencies]\nsea-orm = \"1\"\n" > Cargo.toml'

run_case "mysql_async without diesel" fail \
    'printf "[dependencies]\nmysql_async = \"0.34\"\n" > Cargo.toml'

run_case "rusqlite used in source even with diesel" fail \
    'printf "[dependencies]\ndiesel = \"2\"\n" > Cargo.toml; echo "use rusqlite::Connection;" > src/lib.rs'

run_case "sqlx used in source" fail \
    'echo "let _ = sqlx::query(\"x\");" > src/lib.rs'

run_case "sql_query in a module named raw" pass \
    'printf "[dependencies]\ndiesel = \"2\"\n" > Cargo.toml; echo "fn f(){ diesel::sql_query(\"x\"); }" > src/raw.rs'

run_case "sql_query in a raw directory" pass \
    'mkdir -p src/store/raw; echo "fn f(){ sql_query(\"x\"); }" > src/store/raw/mod.rs'

run_case "sql_query outside raw" fail \
    'echo "fn f(){ diesel::sql_query(\"x\"); }" > src/lib.rs'

run_case "allow-listed manifest with a reason" pass \
    'printf "[dependencies]\nsqlx = \"0.8\"\n" > Cargo.toml; echo "manifest:Cargo.toml legacy import tool, removed in 0.3" > .github/db-access-allow.txt'

run_case "allow-list entry without a reason" fail \
    'printf "[dependencies]\nsqlx = \"0.8\"\n" > Cargo.toml; echo "manifest:Cargo.toml" > .github/db-access-allow.txt'

run_case "allow-listed source prefix" pass \
    'echo "use rusqlite::Connection;" > src/legacy.rs; echo "source:src/legacy.rs one-off migration helper" > .github/db-access-allow.txt'

run_case "allow-list prefix does not match a sibling" fail \
    'echo "use rusqlite::Connection;" > src/legacy2.rs; echo "source:src/legacy one-off" > .github/db-access-allow.txt'

run_case "malformed allow-list line" fail \
    'echo "everything is fine" > .github/db-access-allow.txt'

run_case "target directory is ignored" pass \
    'mkdir -p target/x; printf "[dependencies]\nsqlx = \"1\"\n" > target/x/Cargo.toml; echo "use sqlx::Pool;" > target/x/a.rs'

if [ "$failures" -ne 0 ]; then
    echo "$failures of $n cases failed" >&2
    exit 1
fi
echo "all $n cases passed"
