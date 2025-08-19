use crate::inventory::CollectedResource;
use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

pub fn init_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;

    // Use WAL mode for better concurrency and performance.
    conn.pragma_update(None, "journal_mode", "WAL")?;

    // Create tables if they don't exist.
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS resources (
            id INTEGER PRIMARY KEY,
            arn TEXT NOT NULL UNIQUE,
            region TEXT NOT NULL,
            resource_type TEXT NOT NULL,
            name TEXT,
            details TEXT -- JSON blob for extra data
        );

        CREATE TABLE IF NOT EXISTS tags (
            resource_id INTEGER NOT NULL,
            key TEXT NOT NULL,
            value TEXT NOT NULL,
            FOREIGN KEY(resource_id) REFERENCES resources(id) ON DELETE CASCADE,
            PRIMARY KEY(resource_id, key)
        );

        CREATE TABLE IF NOT EXISTS ip_addresses (
            resource_id INTEGER NOT NULL,
            ip_address TEXT NOT NULL,
            is_public BOOLEAN NOT NULL,
            FOREIGN KEY(resource_id) REFERENCES resources(id) ON DELETE CASCADE,
            PRIMARY KEY(resource_id, ip_address)
        );

        CREATE INDEX IF NOT EXISTS idx_ip_address ON ip_addresses(ip_address);
        CREATE INDEX IF NOT EXISTS idx_tags ON tags(key, value);
        ",
    )?;

    Ok(conn)
}

pub fn save_resources(conn: &mut Connection, resources: &[CollectedResource]) -> Result<()> {
    let tx = conn.transaction()?;

    for resource in resources {
        // Insert the main resource
        tx.execute(
            "INSERT OR REPLACE INTO resources (arn, region, resource_type, name, details) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![resource.arn, resource.region, resource.resource_type, resource.name, serde_json::to_value(&resource.details)?],
        )?;
        let resource_id = tx.last_insert_rowid();

        // Insert tags
        for (key, value) in &resource.tags {
            tx.execute(
                "INSERT OR REPLACE INTO tags (resource_id, key, value) VALUES (?1, ?2, ?3)",
                params![resource_id, key, value],
            )?;
        }

        // Insert IPs
        for ip in &resource.ips {
            tx.execute(
                "INSERT OR REPLACE INTO ip_addresses (resource_id, ip_address, is_public) VALUES (?1, ?2, ?3)",
                params![resource_id, ip.to_string(), is_public(ip)],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// A stable implementation to check if an IP address is considered public.
/// This is a simplified version of the unstable `is_global()` method.
fn is_public(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ipv4) => {
            !ipv4.is_private()
                && !ipv4.is_loopback()
                && !ipv4.is_link_local()
                && !ipv4.is_broadcast()
                && !ipv4.is_documentation()
                && !ipv4.is_unspecified()
        }
        std::net::IpAddr::V6(ipv6) => {
            let segments = ipv6.segments();
            // Check for Unique Local Addresses (fc00::/7)
            let is_unique_local = (segments[0] & 0xfe00) == 0xfc00;
            // Check for Link-Local Addresses (fe80::/10)
            let is_link_local = (segments[0] & 0xffc0) == 0xfe80;

            !ipv6.is_loopback()
                && !ipv6.is_unspecified()
                && !is_unique_local
                && !is_link_local
                // Also check for documentation prefixes (2001:db8::/32)
                && !((segments[0] == 0x2001) && (segments[1] == 0xdb8))
        }
    }
}
