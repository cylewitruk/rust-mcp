#!/bin/sh
set -e

# PostgreSQL settings
PGDATA="${PGDATA:-/var/lib/postgresql/data}"
PGSOCKET="/run/postgresql"
# Outbound network firewall: restrict egress to known hosts only
OUTBOUND_FIREWALL="${OUTBOUND_FIREWALL:-true}"
OUTBOUND_ALLOWLIST="${OUTBOUND_ALLOWLIST:-crates.io,static.crates.io,docs.rs,api.osv.dev}"

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

if [ "$OUTBOUND_FIREWALL" = "true" ]; then
    echo "firewall: setting up outbound allowlist: $OUTBOUND_ALLOWLIST"

    # Always allow: loopback, established/related, DNS
    iptables -A OUTPUT -o lo -j ACCEPT
    iptables -A OUTPUT -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
    iptables -A OUTPUT -p udp --dport 53 -j ACCEPT
    iptables -A OUTPUT -p tcp --dport 53 -j ACCEPT

    # Allow HTTPS to each allowed host (resolve at startup)
    IFS=','
    for host in $OUTBOUND_ALLOWLIST; do
        resolved=$(getent ahosts "$host" 2>/dev/null | awk '{print $1}' | sort -u)
        if [ -z "$resolved" ]; then
            echo "firewall: warning: could not resolve $host, skipping"
            continue
        fi
        for ip in $resolved; do
            iptables -A OUTPUT -p tcp --dport 443 -d "$ip" -j ACCEPT
        done
        echo "firewall: allowed $host ($(echo "$resolved" | tr '\n' ' '))"
    done
    unset IFS

    # Drop all other outbound TCP/UDP
    iptables -A OUTPUT -p tcp --dport 1:65535 -j DROP
    iptables -A OUTPUT -p udp --dport 1:65535 -j DROP

    echo "firewall: outbound rules applied"
fi

# Drop privileges: hand off PID 1 to the app as rust-mcp user
exec su-exec rust-mcp /usr/local/bin/rust-mcp "$@"
