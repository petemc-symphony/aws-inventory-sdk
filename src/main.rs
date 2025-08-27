use anyhow::Result;
use aws_inventory_sdk::{config, export, identify, inventory, server};
use std::net::IpAddr;
use std::env;
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(name = "aws-inventory", about = "AWS inventory tool using the SDK")]
enum Opt {
    Inventory {
        #[structopt(long)]
        profile: Option<String>,

        #[structopt(long, use_delimiter = true)]
        regions: Vec<String>,

        #[structopt(long, help = "Path to the inventory database file. Defaults to 'aws_inventory.db' next to the executable.")]
        output: Option<PathBuf>,

        #[structopt(long, use_delimiter = true, help = "Specific services to inventory (e.g., ec2,elb,rds). Defaults to 'ec2' if --all-services is not used.")]
        services: Vec<String>,

        #[structopt(long, help = "Inventory all available services.")]
        all_services: bool,

        #[structopt(long, help = "Skip EKS pod inventory. Overrides --services and --all-services for EKS.")]
        no_eks: bool,

        #[structopt(long, use_delimiter = true, help = "Specific EKS clusters to scan (optional)")]
        eks_clusters: Vec<String>,
    },
    Query {
        #[structopt(long, help = "Path to the inventory database file. Defaults to 'aws_inventory.db' next to the executable.")]
        inventory: Option<PathBuf>,

        #[structopt(long, short, use_delimiter = true)]
        services: Vec<String>,

        #[structopt(long, short, use_delimiter = true)]
        regions: Vec<String>,

        #[structopt(long)]
        text: bool,
    },
    Identify {
        #[structopt(long, help = "Path to the inventory database file. Defaults to 'aws_inventory.db' next to the executable.")]
        inventory: Option<PathBuf>,

        #[structopt(name = "IP_ADDRESS")]
        ip_address: IpAddr,
    },
    ExportHosts {
        #[structopt(long, help = "Path to the inventory database file. Defaults to 'aws_inventory.db' next to the executable.")]
        inventory: Option<PathBuf>,

        #[structopt(long, short, default_value = "hosts.txt")]
        output: PathBuf,
    },
    Serve {
        #[structopt(long, help = "Path to the inventory database file. Defaults to 'aws_inventory.db' next to the executable.")]
        inventory: Option<PathBuf>,

        #[structopt(long, default_value = "127.0.0.1:8080", help = "Address to listen on")]
        listen: String,

        #[structopt(long, help = "Do not open the web browser automatically")]
        no_browser: bool,
    },
}

/// Determines the default path for the database file, which is in the same
/// directory as the executable.
fn get_default_db_path() -> Result<PathBuf> {
    let mut path = env::current_exe()
        .map_err(|e| anyhow::anyhow!("Failed to get current executable path: {}", e))?;
    path.pop();
    path.push("aws_inventory.db");
    Ok(path)
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();

    match opt {
        Opt::Inventory {
            profile,
            regions,
            output,
            services,
            all_services,
            no_eks,
            eks_clusters,
        } => {
            let output = match output {
                Some(path) => path,
                None => get_default_db_path()?,
            };

            let regions_to_scan = if regions.iter().any(|r| r == "all") {
                config::get_available_regions()
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            } else {
                regions
            };

            // Initialize the database
            let mut conn = aws_inventory_sdk::db::init_db(&output)?;
            println!("Using inventory database at: {:?}", output);
            let profile_name = profile.as_deref().unwrap_or_default();
            
            // Dynamically build the list of collectors based on flags
            let mut collectors: Vec<Box<dyn inventory::AwsResourceCollector>> = Vec::new();

            let mut services_to_run = services;
            if all_services {
                // If --all-services is used, populate with all known collectors
                services_to_run = vec![
                    "ec2".to_string(), "elb".to_string(), "rds".to_string(),
                    "dynamodb".to_string(), "elasticache".to_string(), "eks".to_string(),
                    "route53".to_string()
                ];
            } else if services_to_run.is_empty() {
                // Default to only collecting EC2 if no services are specified
                services_to_run.push("ec2".to_string());
            }

            // The --no-eks flag acts as a final override
            if no_eks {
                services_to_run.retain(|s| s != "eks");
            }

            println!("Will collect inventory for: {}", services_to_run.join(", "));

            for service in services_to_run {
                match service.as_str() {
                    "ec2" => collectors.push(Box::new(inventory::Ec2Collector)),
                    "elb" => collectors.push(Box::new(inventory::ElbCollector)),
                    "rds" => collectors.push(Box::new(inventory::RdsCollector)),
                    "dynamodb" => collectors.push(Box::new(inventory::DynamoDbCollector)),
                    "elasticache" => collectors.push(Box::new(inventory::ElastiCacheCollector)),
                    "eks" => collectors.push(Box::new(inventory::EksCollector::new(eks_clusters.clone()))),
                    "route53" => collectors.push(Box::new(inventory::Route53Collector)),
                    other => eprintln!("Warning: Unknown service '{}' specified, skipping.", other),
                }
            }

            let mut total_resources = 0;
            println!("\n--- Starting Inventory Collection ---");
            for collector in collectors {
                let resources = collector.collect(profile_name, &regions_to_scan).await?;
                let count = resources.len();
                if count > 0 {
                    println!("  -> Saving {} collected resources to the database...", count);
                    aws_inventory_sdk::db::save_resources(&mut conn, &resources)?;
                    total_resources += count;
                }
            }

            println!("\n--- Inventory Complete ---");
            println!("Discovered and saved a total of {} resources.", total_resources);
            println!("Inventory database is at {:?}", output);
        }
        Opt::Identify { inventory, ip_address } => {
            let inventory = match inventory {
                Some(path) => path,
                None => get_default_db_path()?,
            };
            if let Some(result) = identify::identify_resource_from_db(&inventory, ip_address)? {
                println!("{}", result);
            } else {
                println!("IP address not found in inventory.");
            }
        }
        Opt::ExportHosts { inventory, output } => {
            let inventory = match inventory {
                Some(path) => path,
                None => get_default_db_path()?,
            };
            export::to_hosts_file_from_db(&inventory, &output)?;
            println!("Hosts file exported to {:?}", output);
        }
        Opt::Query {
            inventory,
            services,
            regions,
            text,
        } => {
            let inventory = match inventory {
                Some(path) => path,
                None => get_default_db_path()?,
            };
            aws_inventory_sdk::query::query_resources(&inventory, &services, &regions, text)?;
        }
        Opt::Serve {
            inventory,
            listen,
            no_browser,
        } => {
            let inventory = match inventory {
                Some(path) => path,
                None => get_default_db_path()?,
            };
            let listen_addr = listen.clone();
            server::start_server(inventory, listen_addr, no_browser).await?;
        }
    }

    Ok(())
}
