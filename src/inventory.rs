use anyhow::Result;
use aws_config::SdkConfig;
use aws_sdk_ec2::Client as Ec2Client;
use aws_sdk_eks::Client as EksClient;
use aws_sdk_elasticloadbalancingv2::Client as ElbClient;
use aws_sdk_rds::Client as RdsClient;
use aws_sdk_dynamodb::Client as DynamoDbClient;
use aws_sdk_elasticache::Client as ElastiCacheClient;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;
use std::process::Command;

/// A standardized representation of a resource to be stored.
#[derive(Debug)]
pub struct CollectedResource {
    pub arn: String,
    pub name: String,
    pub resource_type: String,
    pub region: String,
    pub ips: Vec<IpAddr>,
    pub tags: HashMap<String, String>,
    pub details: Value,
}

#[async_trait::async_trait]
pub trait AwsResourceCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>>;
}

async fn create_config(profile: &str, region: &str) -> SdkConfig {
    let region_obj = aws_config::Region::new(region.to_string());
    let mut config_builder =
        aws_config::defaults(aws_config::BehaviorVersion::latest()).region(region_obj);
    if !profile.is_empty() {
        config_builder = config_builder.profile_name(profile);
    }
    config_builder.load().await
}

pub struct Ec2Collector;

#[async_trait::async_trait]
impl AwsResourceCollector for Ec2Collector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            println!("Fetching EC2 instances from {}...", region);
            let config = create_config(profile, region).await;
            let client = Ec2Client::new(&config);
            let mut stream = client.describe_instances().into_paginator().send();

            let mut count = 0;
            while let Some(result) = stream.next().await {
                for reservation in result?.reservations.unwrap_or_default() {
                    for instance in reservation.instances.unwrap_or_default() {
                        let mut ips = Vec::new();
                        if let Some(ip_str) = &instance.private_ip_address {
                            if let Ok(ip) = ip_str.parse() {
                                ips.push(ip);
                            }
                        }
                        if let Some(ip_str) = &instance.public_ip_address {
                            if let Ok(ip) = ip_str.parse() {
                                ips.push(ip);
                            }
                        }

                        let tags: HashMap<_, _> = instance
                                .tags
                                .unwrap_or_default()
                                .into_iter()
                                .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
                                .collect();

                        let name = tags.get("Name").cloned().unwrap_or_else(|| instance.instance_id.clone().unwrap_or_default());

                        all_resources.push(CollectedResource {
                            arn: instance.instance_id.clone().unwrap_or_default(), // Note: This is not a real ARN, but it's unique.
                            name,
                            resource_type: "ec2:instance".to_string(),
                            region: region.to_string(),
                            ips,
                            tags,
                            details: serde_json::json!({ "instance_type": instance.instance_type.map(|t| t.as_str().to_string()) }),
                        });
                        count += 1;
                    }
                }
            }
            println!("  -> Found {} instances in {}.", count, region);
        }
        Ok(all_resources)
    }
}

pub struct ElbCollector;

#[async_trait::async_trait]
impl AwsResourceCollector for ElbCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            println!("Fetching Load Balancers from {}...", region);
            let config = create_config(profile, region).await;
            let client = ElbClient::new(&config);
            let mut lbs_stream = client.describe_load_balancers().into_paginator().send();

            let mut region_lbs = vec![];
            while let Some(result) = lbs_stream.next().await {
                region_lbs.extend(result?.load_balancers.unwrap_or_default());
            }

            if region_lbs.is_empty() {
                println!("  -> Found 0 load balancers in {}.", region);
                continue;
            }

            let mut tags_map: HashMap<String, HashMap<String, String>> = HashMap::new();
            for lb_chunk in region_lbs.chunks(20) {
                let arns: Vec<String> = lb_chunk
                    .iter()
                    .filter_map(|lb| lb.load_balancer_arn.clone())
                    .collect();
                if arns.is_empty() {
                    continue;
                }

                let tags_output = client
                    .describe_tags()
                    .set_resource_arns(Some(arns))
                    .send()
                    .await?;
                for tag_desc in tags_output.tag_descriptions.unwrap_or_default() {
                    let arn = tag_desc.resource_arn.unwrap_or_default();
                    let tags = tag_desc
                        .tags
                        .unwrap_or_default()
                        .into_iter()
                        .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
                        .collect();
                    tags_map.insert(arn, tags);
                }
            }

            let mut count = 0;
            for lb in region_lbs {
                let arn = lb.load_balancer_arn.clone().unwrap_or_default();
                let name = lb.load_balancer_name.clone().unwrap_or_default();
                let tags = tags_map.get(&arn).cloned().unwrap_or_default();

                let mut ips = vec![];
                if let Some(azs) = lb.availability_zones {
                    for az in azs {
                        if let Some(addrs) = az.load_balancer_addresses {
                            for addr in addrs {
                                if let Some(ip_str) = addr.ip_address {
                                    if let Ok(ip) = ip_str.parse() {
                                        ips.push(ip);
                                    }
                                }
                            }
                        }
                    }
                }

                all_resources.push(CollectedResource {
                    arn,
                    name,
                    resource_type: "elbv2:loadbalancer".to_string(),
                    region: region.to_string(),
                    ips,
                    tags,
                    details: serde_json::json!({
                        "dns_name": lb.dns_name,
                        "type": lb.r#type.map(|t| t.as_str().to_string()),
                        "scheme": lb.scheme.map(|s| s.as_str().to_string()),
                    }),
                });
                count += 1;
            }
            println!("  -> Found {} load balancers in {}.", count, region);
        }

        Ok(all_resources)
    }
}

#[derive(Deserialize, Debug)]
struct KubePodList {
    items: Vec<KubePod>,
}

#[derive(Deserialize, Debug)]
struct KubePod {
    metadata: KubeMetadata,
    status: KubeStatus,
}

#[derive(Deserialize, Debug)]
struct KubeMetadata {
    name: String,
    namespace: String,
    labels: Option<HashMap<String, String>>,
}

#[derive(Deserialize, Debug)]
struct KubeStatus {
    #[serde(rename = "podIP")]
    pod_ip: Option<String>,
}

pub struct EksCollector {
    clusters_to_scan: Vec<String>,
    skip_eks: bool,
}

impl EksCollector {
    pub fn new(clusters_to_scan: Vec<String>, skip_eks: bool) -> Self {
        Self {
            clusters_to_scan,
            skip_eks,
        }
    }
}

#[async_trait::async_trait]
impl AwsResourceCollector for EksCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        if self.skip_eks {
            println!("\nSkipping EKS pod inventory as requested.");
            return Ok(vec![]);
        }

        let mut all_resources = Vec::new();

        for region in regions {
            let clusters_to_process = if !self.clusters_to_scan.is_empty() {
                self.clusters_to_scan.clone()
            } else {
                println!("Discovering EKS clusters in {}...", region);
                let config = create_config(profile, region).await;
                let client = EksClient::new(&config);
                let mut cluster_stream = client.list_clusters().into_paginator().send();
                let mut discovered_clusters = Vec::new();
                while let Some(result) = cluster_stream.next().await {
                    discovered_clusters.extend(result?.clusters.unwrap_or_default());
                }
                println!("  -> Found {} clusters in {}.", discovered_clusters.len(), region);
                discovered_clusters
            };

            for cluster_name in &clusters_to_process {
                println!("Updating kubeconfig for cluster '{}'...", cluster_name);
                let mut cmd = Command::new("aws");
                cmd.arg("eks")
                    .arg("update-kubeconfig")
                    .arg("--region")
                    .arg(region)
                    .arg("--name")
                    .arg(cluster_name);
                if !profile.is_empty() {
                    cmd.arg("--profile").arg(profile);
                }

                let output = cmd.output()?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !self.clusters_to_scan.is_empty() && stderr.contains("ResourceNotFoundException") {
                        continue;
                    }
                    eprintln!("Failed to update kubeconfig for cluster '{}': {}", cluster_name, stderr);
                    continue;
                }

                println!("Fetching pods from cluster '{}'...", cluster_name);
                let mut cmd = Command::new("kubectl");
                cmd.arg("get").arg("pods").arg("--all-namespaces").arg("-o").arg("json");

                let output = cmd.output()?;
                if !output.status.success() {
                    eprintln!("Error running kubectl for cluster '{}': {}", cluster_name, String::from_utf8_lossy(&output.stderr));
                    continue;
                }

                let pod_list: KubePodList = serde_json::from_slice(&output.stdout)?;
                let mut count = 0;
                for pod in pod_list.items {
                    if let Some(ip_str) = pod.status.pod_ip {
                        if let Ok(ip) = ip_str.parse() {
                            let name = pod.metadata.name;
                            let namespace = pod.metadata.namespace;
                            let arn = format!("{}/{}/{}/{}", region, cluster_name, namespace, name);

                            all_resources.push(CollectedResource {
                                arn,
                                name,
                                resource_type: "eks:pod".to_string(),
                                region: region.to_string(),
                                ips: vec![ip],
                                tags: pod.metadata.labels.unwrap_or_default(),
                                details: serde_json::json!({
                                    "cluster": cluster_name,
                                    "namespace": namespace,
                                }),
                            });
                            count += 1;
                        }
                    }
                }
                println!("  -> Found {} pods in cluster '{}'.", count, cluster_name);
            }
        }

        Ok(all_resources)
    }
}


pub struct RdsCollector;

#[async_trait::async_trait]
impl AwsResourceCollector for RdsCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            println!("Fetching RDS instances from {}...", region);
            let config = create_config(profile, region).await;
            let client = RdsClient::new(&config);
            let mut stream = client.describe_db_instances().into_paginator().send();

            let mut count = 0;
            while let Some(result) = stream.next().await {
                for db_instance in result?.db_instances.unwrap_or_default() {
                    let tags: HashMap<_, _> = db_instance
                        .tag_list
                        .unwrap_or_default()
                        .into_iter()
                        .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
                        .collect();

                    let name = db_instance.db_instance_identifier.clone().unwrap_or_default();
                    let arn = db_instance.db_instance_arn.clone().unwrap_or_default();

                    all_resources.push(CollectedResource {
                        arn,
                        name,
                        resource_type: "rds:db_instance".to_string(),
                        region: region.to_string(),
                        ips: vec![], // RDS endpoints are hostnames, not IPs
                        tags,
                        details: serde_json::json!({
                            "engine": db_instance.engine,
                            "instance_class": db_instance.db_instance_class,
                            "publicly_accessible": db_instance.publicly_accessible,
                        }),
                    });
                    count += 1;
                }
            }
            println!("  -> Found {} instances in {}.", count, region);
        }
        Ok(all_resources)
    }
}

pub struct DynamoDbCollector;

#[async_trait::async_trait]
impl AwsResourceCollector for DynamoDbCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            println!("Fetching DynamoDB tables from {}...", region);
            let config = create_config(profile, region).await;
            let client = DynamoDbClient::new(&config);
            let mut tables_stream = client.list_tables().into_paginator().send();

            let mut table_names = Vec::new();
            while let Some(result) = tables_stream.next().await {
                table_names.extend(result?.table_names.unwrap_or_default());
            }

            let mut count = 0;
            for table_name in table_names {
                let desc = client.describe_table().table_name(&table_name).send().await?;
                let table = desc.table.unwrap();

                let tags_output = client.list_tags_of_resource().resource_arn(table.table_arn().unwrap()).send().await?;
                let tags: HashMap<_, _> = tags_output
                    .tags
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| (t.key, t.value))
                    .collect();

                all_resources.push(CollectedResource {
                    arn: table.table_arn.clone().unwrap_or_default(),
                    name: table.table_name.clone().unwrap_or_default(),
                    resource_type: "dynamodb:table".to_string(),
                    region: region.to_string(),
                    ips: vec![],
                    tags,
                    details: serde_json::json!({
                        "item_count": table.item_count,
                        "table_size_bytes": table.table_size_bytes,
                    }),
                });
                count += 1;
            }
            println!("  -> Found {} tables in {}.", count, region);
        }
        Ok(all_resources)
    }
}

pub struct ElastiCacheCollector;

#[async_trait::async_trait]
impl AwsResourceCollector for ElastiCacheCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            println!("Fetching ElastiCache clusters from {}...", region);
            let config = create_config(profile, region).await;
            let client = ElastiCacheClient::new(&config);
            let mut clusters_stream = client.describe_cache_clusters().into_paginator().send();

            let mut count = 0;
            while let Some(result) = clusters_stream.next().await {
                for cluster in result?.cache_clusters.unwrap_or_default() {
                    let arn = cluster.arn.clone().unwrap_or_default();
                    let tags_output = client.list_tags_for_resource().resource_name(&arn).send().await?;
                    let tags: HashMap<_, _> = tags_output
                        .tag_list
                        .unwrap_or_default()
                        .into_iter()
                        .map(|t| (t.key.unwrap_or_default(), t.value.unwrap_or_default()))
                        .collect();

                    let ips = Vec::new();
                    if let Some(nodes) = cluster.cache_nodes {
                        for node in nodes {
                            if let Some(endpoint) = node.endpoint {
                                if let Some(_address) = endpoint.address {
                                    // This is a hostname, not an IP. The prompt is wrong.
                                }
                            }
                        }
                    }

                    all_resources.push(CollectedResource {
                        arn,
                        name: cluster.cache_cluster_id.clone().unwrap_or_default(),
                        resource_type: "elasticache:cluster".to_string(),
                        region: region.to_string(),
                        ips,
                        tags,
                        details: serde_json::json!({
                            "engine": cluster.engine,
                            "engine_version": cluster.engine_version,
                            "cache_node_type": cluster.cache_node_type,
                        }),
                    });
                    count += 1;
                }
            }
            println!("  -> Found {} clusters in {}.", count, region);
        }
        Ok(all_resources)
    }
}