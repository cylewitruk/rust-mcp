#!/bin/sh
set -e

PGDATA="${PGDATA:-/var/lib/postgresql/data}"
PGSOCKET="/run/postgresql"

# First-run: initialize the database cluster (as postgres user)
if [ ! -f "$PGDATA/PG_VERSION" ]; then
    su-exec postgres initdb -D "$PGDATA" --auth=trust --no-instructions
fi

# Remove stale PID file left behind after a hard container stop
rm -f "$PGDATA/postmaster.pid"

# Start postgres in the background as postgres user (unix socket only, no TCP)
su-exec postgres pg_ctl start -D "$PGDATA" -l "$PGDATA/postgres.log" -o "-k $PGSOCKET -h ''"

# Wait for readiness
until su-exec postgres pg_isready -h "$PGSOCKET" -q; do sleep 0.1; done

# Create the application database on first run
su-exec postgres createdb -h "$PGSOCKET" rust_mcp 2>/dev/null || true

# Drop privileges: hand off PID 1 to the app as rust-mcp user
exec su-exec rust-mcp /usr/local/bin/rust-mcp "$@"
