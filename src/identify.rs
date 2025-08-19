use anyhow::Result;
use rusqlite::{params, Connection};
use std::net::IpAddr;
use std::path::Path;

pub fn identify_resource_from_db(db_path: &Path, ip_address: IpAddr) -> Result<Option<String>> {
    let conn = Connection::open(db_path)?;

    let mut stmt = conn.prepare(
        "
        SELECT r.name, r.resource_type, r.region, r.arn
        FROM resources r
        JOIN ip_addresses i ON r.id = i.resource_id
        WHERE i.ip_address = ?1
        ",
    )?;

    let result = stmt.query_row(params![ip_address.to_string()], |row| {
        let name: String = row.get(0)?;
        let resource_type: String = row.get(1)?;
        let region: String = row.get(2)?;
        let arn: String = row.get(3)?;
        Ok(format!(
            "IP: {} - Type: {}, Name: {}, Region: {}, ARN/ID: {}",
            ip_address, resource_type, name, region, arn
        ))
    });

    Ok(result.ok())
}
