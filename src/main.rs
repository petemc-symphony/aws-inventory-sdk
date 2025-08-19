use anyhow::Result;
use aws_inventory_sdk::{config, export, identify, inventory};
use std::net::IpAddr;
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

        // The --append flag is no longer needed with a database
        // #[structopt(long)]
        // append: bool,
        // The default output is now a database file.
        #[structopt(long, default_value = "aws_inventory.db")]
        output: PathBuf,

        #[structopt(long, help = "Skip EKS pod inventory")]
        no_eks: bool,

        #[structopt(long, use_delimiter = true, help = "Specific EKS clusters to scan (optional)")]
        eks_clusters: Vec<String>,
    },
    Identify {
        #[structopt(long, default_value = "aws_inventory.db")]
        inventory: PathBuf,

        #[structopt(name = "IP_ADDRESS")]
        ip_address: IpAddr,
    },
    ExportHosts {
        #[structopt(long, default_value = "aws_inventory.db")]
        inventory: PathBuf,

        #[structopt(long, short, default_value = "hosts.txt")]
        output: PathBuf,
    },

}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::from_args();

    match opt {
        Opt::Inventory {
            profile,
            regions,
            output,
            no_eks,
            eks_clusters,
        } => {
            let regions_to_scan = if regions.contains(&"all".to_string()) {
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

            // Instantiate collectors based on command-line arguments
            let eks_collector =
                inventory::EksCollector::new(eks_clusters.clone(), no_eks);

            // Create a list of collectors to run
            let collectors: Vec<Box<dyn inventory::AwsResourceCollector>> = vec![
                Box::new(inventory::Ec2Collector),
                Box::new(inventory::ElbCollector),
                Box::new(eks_collector),
            ];

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
            // Now identify just needs to query the database
            if let Some(result) = identify::identify_resource_from_db(&inventory, ip_address)? {
                println!("{}", result);
            } else {
                println!("IP address not found in inventory.");
            }
        }
        Opt::ExportHosts { inventory, output } => {
            export::to_hosts_file_from_db(&inventory, &output)?;
            println!("Hosts file exported to {:?}", output);
        }

    }

    Ok(())
}
