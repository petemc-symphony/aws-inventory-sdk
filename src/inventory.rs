use anyhow::Result;
use aws_config::SdkConfig;
use aws_sdk_ec2::Client as Ec2Client;
use aws_sdk_eks::Client as EksClient;
use aws_sdk_elasticloadbalancingv2::Client as ElbClient;
use aws_sdk_s3::{primitives::DateTime, Client as S3Client};
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

pub struct S3Collector;

#[async_trait::async_trait]
impl AwsResourceCollector for S3Collector {
    async fn collect(&self, profile: &str, _regions: &[String]) -> Result<Vec<CollectedResource>> {
        println!("Fetching S3 Buckets (this may take a while)...");
        // S3 list_buckets is a global operation, so we start with a us-east-1 client.
        let config = create_config(profile, "us-east-1").await;
        let client = S3Client::new(&config);

        let resp = client.list_buckets().send().await?;
        let buckets = resp.buckets.unwrap_or_default();
        let mut handles = vec![];

        println!("  -> Found {} buckets. Now fetching details for each.", buckets.len());

        for bucket in buckets {
            let bucket_name = bucket.name.unwrap_or_default();
            let creation_date = bucket.creation_date;
            let client = client.clone();

            handles.push(tokio::spawn(async move {
                collect_one_bucket(&client, bucket_name, creation_date).await
            }));
        }

        let results = futures::future::join_all(handles).await;
        let mut all_resources = Vec::new();
        for result in results {
            match result {
                Ok(Ok(Some(resource))) => all_resources.push(resource),
                Ok(Err(e)) => eprintln!("Could not process a bucket: {}", e),
                Err(e) => eprintln!("Task failed for a bucket: {}", e),
                _ => {}
            }
        }

        println!("Finished S3 bucket collection.");
        Ok(all_resources)
    }
}

async fn collect_one_bucket(
    client: &S3Client,
    bucket_name: String,
    creation_date: Option<DateTime>,
) -> Result<Option<CollectedResource>> {
    let location = client.get_bucket_location().bucket(&bucket_name).send().await?;
    let region = location
        .location_constraint
        .map(|lc| lc.as_str().to_string())
        .unwrap_or_else(|| "us-east-1".to_string());

    // Create a new client for the correct region if necessary
    let region_config = client.config().to_builder().region(aws_config::Region::new(region.clone())).build();
    let region_client = S3Client::from_conf(region_config);

    let mut object_stream = region_client.list_objects_v2().bucket(&bucket_name).into_paginator().send();

    let mut total_size: i64 = 0;
    let mut total_count: i64 = 0;
    let mut newest_files: Vec<(String, DateTime)> = Vec::new();

    while let Some(result) = object_stream.next().await {
        for object in result?.contents.unwrap_or_default() {
            total_count += 1;
            total_size += object.size.unwrap_or(0);

            if let (Some(key), Some(modified)) = (object.key, object.last_modified) {
                if newest_files.len() < 5 {
                    newest_files.push((key, modified));
                    newest_files.sort_by(|a, b| b.1.cmp(&a.1));
                } else if modified > newest_files[4].1 {
                    newest_files[4] = (key, modified);
                    newest_files.sort_by(|a, b| b.1.cmp(&a.1));
                }
            }
        }
    }

    let last_modified = newest_files.first().map(|(_, date)| date.to_string());

    Ok(Some(CollectedResource {
        arn: format!("arn:aws:s3:::{}", bucket_name),
        name: bucket_name,
        resource_type: "s3:bucket".to_string(),
        region,
        ips: vec![], // Buckets don't have IPs
        tags: HashMap::new(), // Tagging requires another API call, skipping for now
        details: serde_json::json!({
            "creation_date": creation_date.map(|d| d.to_string()),
            "last_modified": last_modified,
            "file_count": total_count,
            "total_size": total_size,
            "newest_files": newest_files.into_iter().map(|(key, date)| serde_json::json!({"key": key, "date": date.to_string()})).collect::<Vec<_>>(),
        }),
    }))
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