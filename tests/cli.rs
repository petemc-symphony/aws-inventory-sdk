#[cfg(test)]
mod tests {
    use assert_cmd::prelude::*;
    use predicates::prelude::*;
    use std::process::Command;

    #[tokio::test]
    async fn test_cli() -> Result<(), Box<dyn std::error::Error>> {
        let mut server = mockito::Server::new_async().await;

        let ec2_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/xml")
            .with_body("
                <DescribeInstancesResponse>
                    <reservationSet>
                        <item>
                            <reservationId>r-1234567890abcdef0</reservationId>
                            <instancesSet>
                                <item>
                                    <instanceId>i-1234567890abcdef0</instanceId>
                                    <instanceType>t2.micro</instanceType>
                                    <privateIpAddress>10.0.0.1</privateIpAddress>
                                    <tagSet>
                                        <item>
                                            <key>Name</key>
                                            <value>MyInstance</value>
                                        </item>
                                    </tagSet>
                                </item>
                            </instancesSet>
                        </item>
                    </reservationSet>
                </DescribeInstancesResponse>
            ")
            .expect(1)
            .create_async()
            .await;

        let mut cmd = Command::cargo_bin("aws-inventory-sdk")?;
        cmd.env("AWS_ENDPOINT_URL", server.url());
        cmd.arg("inventory").arg("--regions").arg("us-east-1");
        cmd.assert().success();

        ec2_mock.assert_async().await;

        let mut cmd = Command::cargo_bin("aws-inventory-sdk")?;
        cmd.arg("identify").arg("10.0.0.1");
        cmd.assert().success().stdout(predicate::str::contains("MyInstance"));

        Ok(())
    }
}
