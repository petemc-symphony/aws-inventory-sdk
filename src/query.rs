use anyhow::Result;
use rusqlite::{params_from_iter, Connection};
use serde_json::Value;
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Debug)]
pub struct Resource {
    pub arn: String,
    pub name: String,
    pub resource_type: String,
    pub region: String,
    pub ips: Vec<String>,
    pub tags: Value,
    pub details: Value,
}

pub fn run_query(db_path: &Path, services: &[String], regions: &[String]) -> Result<Vec<Resource>> {
    let conn = Connection::open(db_path)?;
    let mut query = "
        SELECT
            r.arn,
            r.name,
            r.resource_type,
            r.region,
            COALESCE(GROUP_CONCAT(i.ip_address), ''),
            (SELECT json_group_object(key, value) FROM tags WHERE resource_id = r.id),
            r.details
        FROM
            resources r
        LEFT JOIN ip_addresses i ON r.id = i.resource_id
        WHERE 1=1"
        .to_string();
    let mut params_vec: Vec<String> = Vec::new();

    if !services.is_empty() {
        let service_placeholders = services.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        query.push_str(&format!(" AND resource_type IN ({})", service_placeholders));
        for service in services {
            params_vec.push(map_service_name(service));
        }
    }

    if !regions.is_empty() {
        let region_placeholders = regions.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        query.push_str(&format!(" AND region IN ({})", region_placeholders));
        for region in regions {
            params_vec.push(region.clone());
        }
    }

    query.push_str(" GROUP BY r.id, r.arn, r.name, r.resource_type, r.region, r.details");

    let mut stmt = conn.prepare(&query)?;
    let resource_iter = stmt.query_map(params_from_iter(params_vec), |row| {
        let ips_str: String = row.get(4)?;
        let ips: Vec<String> = if ips_str.is_empty() {
            vec![]
        } else {
            ips_str.split(',').map(|s| s.to_string()).collect()
        };

        let tags_str: Option<String> = row.get(5)?;
        let tags: Value = serde_json::from_str(&tags_str.unwrap_or_else(|| "{}".to_string()))
            .unwrap_or_default();

        let details_str: String = row.get(6)?;
        let details: Value = serde_json::from_str(&details_str).unwrap_or_default();

        Ok(Resource {
            arn: row.get(0)?,
            name: row.get(1)?,
            resource_type: row.get(2)?,
            region: row.get(3)?,
            ips,
            tags,
            details,
        })
    })?;

    let mut results = Vec::new();
    for resource in resource_iter {
        results.push(resource?);
    }

    Ok(results)
}

pub fn query_resources(
    db_path: &Path,
    services: &[String],
    regions: &[String],
    text_output: bool,
) -> Result<()> {
    let results = run_query(db_path, services, regions)?;

    if text_output {
        print_text_output(&results);
    } else {
        println!("{}", serde_json::to_string_pretty(&results)?);
    }

    Ok(())
}

fn map_service_name(short_name: &str) -> String {
    match short_name {
        "rds" => "rds:db_instance",
        "dynamodb" => "dynamodb:table",
        "elasticache" => "elasticache:cluster",
        "ec2" => "ec2:instance",
        "elb" => "elbv2:loadbalancer",
        "eks" => "eks:pod",
        "route53" => "route53:hostedzone",
        
        _ => short_name, // If not a short name, assume it's a full resource_type
    }.to_string()
}

fn print_text_output(resources: &[Resource]) {
    if resources.is_empty() {
        println!("No resources found matching your query.");
        return;
    }

    // Group resources by service and region
    let mut grouped: std::collections::HashMap<(String, String), Vec<&Resource>> = std::collections::HashMap::new();
    for r in resources {
        grouped.entry((r.resource_type.clone(), r.region.clone())).or_default().push(r);
    }

    for ((service, region), res) in grouped {
        println!("\nService: {}", service);
        println!("Region: {}", region);
        
        let (max_name, max_arn) = res.iter().fold((0, 0), |(max_name, max_arn), r| {
            (max_name.max(r.name.len()), max_arn.max(r.arn.len()))
        });

        println!("{:<width_name$} {:<width_arn$} IPs", "Name", "ARN", width_name = max_name + 2, width_arn = max_arn + 2);
        println!("{:-<width_name$} {:-<width_arn$} ----", "", "", width_name = max_name + 2, width_arn = max_arn + 2);

        for r in res {
            println!("{:<width_name$} {:<width_arn$} {}", r.name, r.arn, r.ips.join(", "), width_name = max_name + 2, width_arn = max_arn + 2);
        }
    }
}
