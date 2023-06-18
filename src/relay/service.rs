use std::collections::HashMap;

use tokio::signal;
use tokio::task;
use tokio::time::{sleep, Duration};
use tokio_util::sync::CancellationToken;

use crate::relay::model::{GroupResultInput, MetricInput, ResultCategory};
use crate::common::error::Error;

use super::test::runner::TestRunner;
use super::api::ServerApi;

pub async fn launch(base_url: String, token: String, region_name: String) -> Result<(), Error> {

    let cancel_token = CancellationToken::new();
    let cancel_token_task = cancel_token.clone();

    let scheduler_task = task::spawn(async move {

        let runner = TestRunner::new();
        let api = ServerApi::new(&base_url, &token, &region_name);

        let mut region_config = match api.fetch_region_conf().await {
            Ok(config) => config,
            Err(err) => err.exit(
                "Could not fetch configuration from Watchdog API",
                "Check your token and region name"
            )
        };

        println!();
        println!(" ✓ Watchdog relay is now UP");
        println!(" ✓ Found {} group(s) with a {}ms refresh interval", region_config.groups.len(), region_config.interval_ms, );
        println!();

        let mut last_update = String::new();

        loop {
            
            let mut group_results: Vec<GroupResultInput> = vec![];
            for group in &region_config.groups {
    
                let mut is_group_working = true;
                let mut has_group_warnings: bool = false;
                let mut error_message = None;
                let mut error_detail = None;

                let mut group_metrics: Vec<MetricInput> = vec![];

                for test in &group.tests {

                    let test_result = runner.execute_test(test).await;

                    match test_result {
                        Ok(test) => {

                            if test.result == ResultCategory::Success {
                                is_group_working = false;
                            }
                            else if test.result == ResultCategory::Warning {
                                has_group_warnings = true;
                            }

                            for (metric_key, metric_value) in test.metrics.unwrap_or_default() {
                                group_metrics.push(MetricInput {
                                    name: metric_key,
                                    labels: HashMap::from([
                                        ("test_target".into(), test.target.to_string())
                                    ]),
                                    metric: metric_value
                                });
                            }

                        },
                        Err(err) => {
                            eprintln!("{}", err);
                            is_group_working = false;
                            error_message = Some(err.message);
                            error_detail = err.details;
                        }
                    }
                }

                group_results.push(GroupResultInput {
                    name: group.name.clone(),
                    working: is_group_working,
                    has_warnings: has_group_warnings,
                    error_message,
                    error_detail,
                    metrics: group_metrics
                });
            }
            
            let update_result = api.update_region_state(group_results, &last_update).await;
            match update_result {
                Ok(Some(watchdog_update)) => {

                    if !last_update.is_empty() {
                        region_config = api.fetch_region_conf().await.unwrap();
                        println!("Relay config reloaded");
                    }

                    last_update = watchdog_update;

                },
                Err(update_err) => {
                    eprintln!("{}", update_err);
                },
                _ => {}
            }

            let mut cancel_loop = false;

            tokio::select! {
                _ = cancel_token_task.cancelled() => {
                    cancel_loop = true;
                }
                _ = sleep(Duration::from_millis(region_config.interval_ms)) => {
                    // Sleep went well... on to the next tests
                }
            };

            if cancel_loop {
                break;
            }
        }

    });

    signal::ctrl_c().await.map_err(|err| Error::new("Could not handle graceful shutdown signal", err))?;
    cancel_token.cancel();
    println!("Received graceful shutdown signal");

    scheduler_task.await.map_err(|err| Error::new("Could not end scheduler task", err))?;

    Ok(())
}