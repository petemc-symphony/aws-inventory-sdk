# Gemini Prompt: Enhance AWS Inventory Tool with New Services

## Persona
You are Gemini Code Assist, a very experienced and world-class software engineering coding assistant. Your task is to answer questions and provide insightful answers with code quality and clarity.

## Objective
Your task is to enhance our Rust-based AWS inventory tool by adding collection capabilities for four new AWS services: RDS, DynamoDB, ElastiCache, and Route 53. You will then provide safe testing instructions.

## Context
You will be working with the existing codebase for the `aws-inventory-sdk` project. The key files are `src/inventory.rs`, `src/main.rs`, and `Cargo.toml`. The existing `AwsResourceCollector` trait provides the pattern for adding new services.

## Instructions

### 1. Add SDK Dependencies
First, please add the necessary AWS SDK crates to the `Cargo.toml` file. You will need:
- `aws-sdk-rds`
- `aws-sdk-dynamodb`
- `aws-sdk-elasticache`
- `aws-sdk-route53`

Ensure they use a version compatible with the other `aws-sdk-*` crates already in the project.

### 2. Implement New Collectors in `src/inventory.rs`
For each of the services below, create a new struct and implement the `AwsResourceCollector` trait. Follow the existing pattern from `Ec2Collector` and `ElbCollector`.

- **`RdsCollector`**:
  - List all RDS DB instances (`describe_db_instances`).
  - For each instance, collect:
    - DB Instance ARN (`db_instance_arn`)
    - DB Instance Identifier (`db_instance_identifier`) as the `name`.
    - Region.
    - Endpoint address (if available). Since RDS endpoints are hostnames, you do not need to resolve them to an IP. The `ips` field can be empty.
    - Tags (from the `tag_list` field).
    - In `details`, store the engine (`engine`), instance class (`db_instance_class`), and public accessibility status (`publicly_accessible`).

- **`DynamoDbCollector`**:
  - This is a regional service. List all tables in each scanned region (`list_tables`).
  - For each table, get its description (`describe_table`).
  - Collect:
    - Table ARN (`table_arn`).
    - Table Name (`table_name`) as the `name`.
    - Region.
    - Tags (requires a separate `list_tags_of_resource` call).
    - In `details`, store the item count (`item_count`) and table size in bytes (`table_size_bytes`).

- **`ElastiCacheCollector`**:
  - List all ElastiCache clusters in each region (`describe_cache_clusters`).
  - For each cluster, collect:
    - ARN.
    - Cluster ID (`cache_cluster_id`) as the `name`.
    - Region.
    - IPs of the cache nodes.
    - Tags (requires `list_tags_for_resource`).
    - In `details`, store the engine (`engine`), version (`engine_version`), and node type (`cache_node_type`).

- **`Route53Collector`**:
  - This is a global service, so it only needs to be called once (like the S3 collector). Use `us-east-1` for the client.
  - List all hosted zones (`list_hosted_zones`).
  - For each zone, collect:
    - The zone ID as the ARN (e.g., `/hostedzone/Z12345`).
    - The zone name (`name`) as the `name`.
    - Resource type `route53:hostedzone`.
    - Region should be "global".
    - Tags (requires `list_tags_for_resource` for each zone).
    - In `details`, store whether it's a private zone (`config.private_zone`) and the resource record set count (`resource_record_set_count`).

### 3. Update `src/main.rs`
In the `main` function, add the four new collectors (`RdsCollector`, `DynamoDbCollector`, `ElastiCacheCollector`, `Route53Collector`) to the `collectors` vector so they are executed during an inventory run.

### 4. Provide Testing Commands

After providing the code changes as diffs, provide a set of shell commands to build and test the new functionality.

**CRITICAL SAFETY INSTRUCTION:** The testing commands **MUST NOT** under any circumstances modify the user's `~/.aws/config` or `~/.aws/credentials` files. The tool is designed to read credentials using the `--profile` flag, which is the correct and safe behavior. Do not suggest any commands that write to or alter these files. The only profile that should be used for testing is `symphony-aws-c9-dev`.

The `EksCollector`'s use of `aws eks update-kubeconfig` is an exception that modifies `~/.kube/config`, which is acceptable, but the AWS credentials themselves must remain untouched.

The test command should look like this:

```sh
# First, build the project to ensure there are no compilation errors
cargo build --release

# Then, run the inventory collection for a specific region.
# NOTE: Use ONLY the 'symphony-aws-c9-dev' profile for testing.
./target/release/aws-inventory-sdk inventory --profile symphony-aws-c9-dev --regions us-east-1
```

### 5. Output Format
Please provide all code changes in the `diff` format, using full absolute paths for all files.


