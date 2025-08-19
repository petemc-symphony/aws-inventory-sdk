# AWS Inventory SDK

A command-line tool for discovering, inventorying, and querying AWS resources across multiple services and regions. It stores the collected data in a local SQLite database for fast and flexible querying.

## Features

-   **Multi-Service Inventory**: Collects data from EC2, ELB, RDS, EKS, DynamoDB and Elasticache.
-   **SQLite Backend**: All resource data is stored in a local `aws_inventory.db` file, allowing for complex queries and easy data access.
-   **Flexible Querying**: A powerful `query` subcommand to filter resources by service and region.
-   **Multiple Output Formats**: Get query results as pretty-printed JSON or a human-readable text table.
-   **Web Interface**: A built-in `serve` command starts a web server with a JSON API for programmatic access to inventory data.
-   **Network Troubleshooting**: Generate a `hosts` file (`export-hosts` command) for use with tools like Wireshark to resolve AWS IP addresses to resource names.

## Installation

### From Source

Ensure you have the Rust toolchain installed. You can then build the project from the root directory:

```sh
cargo build --release
```
The binary will be available at `target/release/aws-inventory-sdk`.

### Pre-compiled Binaries

You can use the `build.sh` script to create binaries for macOS (ARM) and Linux (x86_64).

```sh
./build.sh
```
The binaries will be placed in the `dist/` directory. Prebuilt binaries can also be downloaded from there

## Usage

All commands are run via the `aws-inventory-sdk` binary.

### 1. Create the Inventory Database

First, you need to populate the local database with your AWS resource data.
It is recommended to have a directory for each profile so it uses a separate db

```sh
mkdir -p ~/inv/{dev,qa,prod}
cp the binary to each dir and use from there
```

```sh
# Scan all available regions for the specified profile
./aws-inventory-sdk-macos-arm64 inventory --profile your-profile-name --regions all

# Scan a specific set of regions
./aws-inventory-sdk-macos-arm64 inventory --profile your-profile-name --regions us-east-1,eu-west-1
```

This will create an `aws_inventory.db` file in your current directory.

### 2. Query the Inventory

The `query` subcommand allows you to filter and view the collected data. By default, it outputs JSON.

**Examples:**

```sh
# Get all EC2 instances and RDS databases across all scanned regions
./aws-inventory-sdk-macos-arm64 query --services ec2,rds

# Get all EKS pods in the us-east-1 region
./aws-inventory-sdk-macos-arm64 query --services eks --regions us-east-1

# Get all EC2 instances in a compact, human-readable text format
./aws-inventory-sdk-macos-arm64 query --services ec2 --text
```

### 3. Serve the Web API

The `serve` command starts a local web server, providing a REST API to your inventory data. It will also automatically open a web browser to the root page.

```sh
# Start the server (defaults to http://127.0.0.1:8080)
./aws-inventory-sdk-macos-arm64 serve

# Start on a different port and don't open the browser
./aws-inventory-sdk-macos-arm64 serve --listen 127.0.0.1:9000 --no-browser
```

Once the server is running, you can query the API.

**API Examples:**

```sh
# Get all EC2 and RDS resources using curl
curl "http://127.0.0.1:8080/api/query?services=ec2,rds"

# Get all EKS pods in the us-east-1 region
curl "http://127.0.0.1:8080/api/query?services=eks&regions=us-east-1"
```

### 4. Identify a Resource by IP

Quickly find which resource an IP address belongs to.

```sh
./aws-inventory-sdk-macos-arm64 identify 10.0.1.5
```

### 5. Export a Hosts File

Generate a hosts file that can be used with tools like Wireshark for easy IP-to-hostname resolution during network analysis.

```sh
# The output file defaults to hosts.txt
./aws-inventory-sdk-macos-arm64 export-hosts

# Specify a different output file
./aws-inventory-sdk-macos-arm64 export-hosts --output /path/to/my-hosts.txt
```
