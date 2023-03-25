use std::fmt::Display;
use std::sync::Arc;
use std::time::Duration;

use dagger_sdk::{HostDirectoryOpts, HostDirectoryOptsBuilder};
use gritea::client::Gritea;
use gritea::pagination::Pagination;
use rand::seq::SliceRandom;
use rand::thread_rng;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::sleep;
use tracing::{Level, Value};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;
    dotenv::dotenv().unwrap();
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .pretty()
        .init();

    tracing::info!("starting renovate gitea sharding");

    let repos = fetch_gitea_repos().await?;
    tracing::info!(records = repos.0.len(), "fetched repos: {}", repos);

    run_renovate(repos).await?;

    Ok(())
}

async fn run_renovate(repos: Repos) -> eyre::Result<()> {
    const MAX_ITEMS_AT_ONCE: usize = 3;

    let (sender, mut r) = tokio::sync::mpsc::channel::<Repo>(MAX_ITEMS_AT_ONCE);
    let r = Arc::new(Mutex::new(r));
    let client = dagger_sdk::connect().await?;
    let config_file = client
        .host()
        .directory_opts(
            ".",
            HostDirectoryOptsBuilder::default()
                .include(vec!["config.json"])
                .build()?,
        )
        .file("config.json");

    let image = client
        .container()
        .from("renovate/renovate:35.19.2")
        .with_mounted_file("/opts/renovate/config.json", config_file.id().await?);

    image.platform().await?;

    tokio_scoped::scope(|s| {
        s.spawn(async {
            let mut repos = repos.0.clone();
            repos.shuffle(&mut thread_rng());

            for repo in repos {
                sender.send(repo.clone()).await.unwrap();
            }

            drop(sender);
        });

        for _ in 0..MAX_ITEMS_AT_ONCE {
            let r = r.clone();
            let image = image.clone();
            s.spawn(async move {
                loop {
                    let mut r = r.lock().await;
                    match r.recv().await {
                        Some(repo) => {
                            drop(r);
                            // download repo
                            tracing::info!("running queue: {}", repo);
                            let image = image.with_env_variable(
                                "GITHUB_COM_TOKEN",
                                std::env::var("GITHUB_COM_TOKEN").unwrap(),
                            );
                            let image = image.with_env_variable(
                                "RENOVATE_SECRETS",
                                std::env::var("RENOVATE_SECRETS").unwrap(),
                            );
                            let image = image.with_env_variable(
                                "RENOVATE_CONFIG_FILE",
                                "/opts/renovate/config.json",
                            );
                            let image = image.with_env_variable(
                                "RENOVATE_TOKEN",
                                std::env::var("GITEA_RENOVATE_TOKEN").unwrap(),
                            );
                            image
                                //.with_env_variable("LOG_LEVEL", "debug")
                                .with_exec(vec![repo.to_string()])
                                .exit_code()
                                .await
                                .unwrap();
                            sleep(Duration::from_secs(2)).await;
                        }
                        None => {
                            tracing::info!("executor is done, closing done renovate shard");

                            return;
                        }
                    }
                }
            });
        }
    });

    tracing::info!("Done running renovate");

    Ok(())
}

struct Repos(Vec<Repo>);

impl Display for Repos {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for repo in &self.0 {
            repo.fmt(f)?;
            f.write_str(",")?;
        }

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Repo(String);

impl Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

async fn fetch_gitea_repos() -> eyre::Result<Repos> {
    const APP_USER_AGENT: &'static str =
        concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
    const GITEA_URL: &'static str = "git.front.kjuulh.io";

    let client = Gritea::builder(GITEA_URL)
        .token(std::env::var("GITEA_ACCESS_TOKEN")?)
        .build()?;

    let mut counter = 1;
    let mut repos: Vec<gritea::repo::Repository> = Vec::new();
    loop {
        let mut repos_page = client.list_repos(Pagination::new(counter, 50)).await?;
        let count = repos_page.len();
        repos.append(&mut repos_page);
        if count != 50 {
            break;
        }
        counter = counter + 1;
    }

    Ok(Repos(
        repos
            .iter()
            .map(|r| Repo(r.full_name.clone()))
            .collect::<Vec<Repo>>(),
    ))
}
