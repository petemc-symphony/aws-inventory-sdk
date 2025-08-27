use anyhow::Result;
use aws_config::SdkConfig;
use aws_sdk_ec2::Client as Ec2Client;
use aws_sdk_eks::Client as EksClient;
use aws_sdk_elasticloadbalancingv2::Client as ElbClient;
use aws_sdk_rds::Client as RdsClient;
use aws_sdk_dynamodb::Client as DynamoDbClient;
use aws_sdk_elasticache::Client as ElastiCacheClient;
use aws_sdk_route53::Client as Route53Client;
use aws_sdk_route53::types::TagResourceType as Route53ResourceType;
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, ListParams, ResourceExt},
    Client,
    config::{
        AuthInfo, Cluster, Context, ExecConfig, Kubeconfig, KubeConfigOptions, NamedAuthInfo,
        NamedCluster, NamedContext,
    },
};
use serde_json::Value;
use std::collections::HashMap;
use std::net::IpAddr;

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

pub struct Route53Collector;

#[async_trait::async_trait]
impl AwsResourceCollector for Route53Collector {
    async fn collect(&self, profile: &str, _regions: &[String]) -> Result<Vec<CollectedResource>> {
        // Route 53 is a global service, so we query it once, ignoring the regions list.
        // We use "us-east-1" for the client, as is standard for global services.
        println!("\nFetching Route 53 hosted zones (global service)...");
        let config = create_config(profile, "us-east-1").await;
        let client = Route53Client::new(&config);
        let mut all_resources = Vec::new();
        let mut zones_stream = client.list_hosted_zones().into_paginator().send();

        let mut count = 0;
        while let Some(result) = zones_stream.next().await {
            for zone in result?.hosted_zones {
                let zone_id = zone.id();
                let resource_id = zone_id.split('/').last().unwrap_or_default();

                let tags = if !resource_id.is_empty() {
                    match client
                        .list_tags_for_resource()
                        .resource_type(Route53ResourceType::Hostedzone)
                        .resource_id(resource_id)
                        .send()
                        .await
                    {
                        Ok(tags_output) => tags_output
                            .resource_tag_set
                            .and_then(|rts| rts.tags)
                            .unwrap_or_default()
                            .into_iter()
                            .filter_map(|t| Some((t.key()?.to_string(), t.value()?.to_string())))
                            .collect(),
                        Err(e) => {
                            eprintln!("Could not get tags for Route53 zone {}: {}", zone_id, e);
                            HashMap::new()
                        }
                    }
                } else {
                    HashMap::new()
                };

                let (is_private, rr_count) = if let Some(ref config) = zone.config {
                    (config.private_zone, zone.resource_record_set_count.unwrap_or(0))
                } else {
                    (false, 0)
                };

                all_resources.push(CollectedResource {
                    arn: zone_id.to_string(),
                    name: zone.name().to_string(),
                    resource_type: "route53:hostedzone".to_string(),
                    region: "global".to_string(),
                    ips: vec![],
                    tags,
                    details: serde_json::json!({
                        "private_zone": is_private,
                        "resource_record_set_count": rr_count,
                    }),
                });
                count += 1;
            }
        }
        println!("  -> Found {} hosted zones.", count);

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

pub struct EksCollector {
    clusters_to_scan: Vec<String>,
}

impl EksCollector {
    pub fn new(clusters_to_scan: Vec<String>) -> Self {
        Self {
            clusters_to_scan,
        }
    }
}

#[async_trait::async_trait]
impl AwsResourceCollector for EksCollector {
    async fn collect(&self, profile: &str, regions: &[String]) -> Result<Vec<CollectedResource>> {
        let mut all_resources = Vec::new();

        for region in regions {
            let config = create_config(profile, region).await;
            let eks_client = EksClient::new(&config);

            let clusters_to_process = if self.clusters_to_scan.is_empty() {
                println!("Discovering EKS clusters in {}...", region);
                let mut cluster_stream = eks_client.list_clusters().into_paginator().send();
                let mut discovered_clusters = Vec::new();
                while let Some(result) = cluster_stream.next().await {
                    discovered_clusters.extend(result?.clusters.unwrap_or_default());
                }
                println!("  -> Found {} clusters in {}.", discovered_clusters.len(), region);
                discovered_clusters
            } else {
                self.clusters_to_scan.clone()
            };

            for cluster_name in &clusters_to_process {
                println!("Connecting to EKS cluster '{}'...", cluster_name);

                let cluster_desc = match eks_client.describe_cluster().name(cluster_name).send().await {
                    Ok(res) => res.cluster.unwrap(),
                    Err(aws_sdk_eks::error::SdkError::ServiceError(service_error)) => {
                        let inner_err = service_error.into_err();
                        if !self.clusters_to_scan.is_empty() {
                            if inner_err.is_resource_not_found_exception() {
                                println!("  -> Cluster '{}' not found in region {}, skipping.", cluster_name, region);
                                continue;
                            }
                        }
                        eprintln!("Failed to describe cluster '{}': {}", cluster_name, inner_err);
                        continue;
                    }
                    Err(e) => {
                        eprintln!("Failed to describe cluster '{}': {}", cluster_name, e);
                        continue;
                    }
                };

                let Some(api_endpoint) = cluster_desc.endpoint else {
                    eprintln!("Cluster '{}' has no endpoint.", cluster_name);
                    continue;
                };
                let Some(ca_data) = cluster_desc.certificate_authority.and_then(|ca| ca.data) else {
                    eprintln!("Cluster '{}' has no certificate authority data.", cluster_name);
                    continue;
                };

                let mut exec_args = vec![
                    "eks".to_string(),
                    "get-token".to_string(),
                    "--cluster-name".to_string(),
                    cluster_name.clone(),
                    "--region".to_string(),
                    region.to_string(),
                ];
                if !profile.is_empty() {
                    exec_args.push("--profile".to_string());
                    exec_args.push(profile.to_string());
                }
                let exec_config = ExecConfig {
                    command: Some("aws".to_string()),
                    args: Some(exec_args),
                    api_version: Some("client.authentication.k8s.io/v1beta1".to_string()),
                    env: None,
                    cluster: None,
                    drop_env: None,
                    interactive_mode: None,
                    provide_cluster_info: false,
                };
                
                let kubeconfig = Kubeconfig {
                    clusters: vec![NamedCluster {
                        name: cluster_name.clone(),
                        cluster: Some(Cluster {
                            server: Some(api_endpoint),
                            certificate_authority_data: Some(ca_data),
                            ..Default::default()
                        }),
                    }],
                    auth_infos: vec![NamedAuthInfo {
                        name: "eks-auth".to_string(),
                        auth_info: Some(AuthInfo {
                            exec: Some(exec_config),
                            ..Default::default()
                        }),
                    }],
                    contexts: vec![NamedContext {
                        name: "eks-context".to_string(),
                        context: Some(Context {
                            cluster: cluster_name.clone(),
                            user: "eks-auth".to_string(),
                            ..Default::default()
                        }),
                    }],
                    current_context: Some("eks-context".to_string()),
                    ..Default::default()
                };

                let config = kube::Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await
                    .map_err(|e| anyhow::anyhow!("Failed to create kubeconfig for cluster '{}': {}", cluster_name, e))?;
                let client = Client::try_from(config)
                    .map_err(|e| anyhow::anyhow!("Failed to create Kubernetes client for cluster '{}'. This may happen if the 'aws' CLI is not in your PATH or not authenticated. Error: {}", cluster_name, e))?;

                println!("Fetching pods from cluster '{}'...", cluster_name);
                let pods: Api<Pod> = Api::all(client);
                let pod_list = match pods.list(&ListParams::default()).await {
                    Ok(pl) => pl,
                    Err(e) => {
                        eprintln!("Error fetching pods from cluster '{}': {}", cluster_name, e);
                        continue;
                    }
                };

                let mut count = 0;
                for pod in pod_list {
                    if let Some(ref status) = pod.status {
                        if let Some(ip_str) = &status.pod_ip {
                            if let Ok(ip) = ip_str.parse::<IpAddr>() {
                                let name = pod.name_any();
                                let namespace = pod.namespace().unwrap_or_default();
                                let arn = format!("{}/{}/{}/{}", region, cluster_name, &namespace, &name);
                                let tags: HashMap<_, _> = pod.metadata.labels.unwrap_or_default().into_iter().collect();

                                all_resources.push(CollectedResource {
                                    arn,
                                    name,
                                    resource_type: "eks:pod".to_string(),
                                    region: region.to_string(),
                                    ips: vec![ip],
                                    tags, // Using K8s labels as AWS tags for consistency
                                    details: serde_json::json!({
                                        "cluster": cluster_name.clone(),
                                        "namespace": namespace,
                                    }),
                                });
                                count += 1;
                            }
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