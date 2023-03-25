use dagger_sdk::HostDirectoryOptsBuilder;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    let client = dagger_sdk::connect().await?;

    let current_dir = client.host().directory_opts(
        ".",
        HostDirectoryOptsBuilder::default()
            .exclude(vec!["target/", ".git"])
            .build()?,
    );

    let chef = client
        .container()
        .from("lukemathwalker/cargo-chef:latest")
        .with_workdir("/app");

    chef.platform().await?;

    let planner = chef
        .with_exec(vec!["mkdir", "-p", "/mnt"])
        .with_mounted_directory(".", current_dir.id().await?)
        .with_exec(vec![
            "cargo",
            "chef",
            "prepare",
            "--recipe-path",
            "/mnt/recipe.json",
        ]);

    let recipe_file = planner.file("/mnt/recipe.json");

    let builder = chef
        .with_mounted_file("/mnt/recipe.json", recipe_file.id().await?)
        .with_exec(vec![
            "cargo",
            "chef",
            "cook",
            "--release",
            "--recipe-path",
            "recipe.json",
        ])
        .with_mounted_directory(".", current_dir.id().await?)
        .with_exec(vec![
            "cargo",
            "build",
            "--release",
            "--bin",
            "renovate-gitea-sharding",
        ]);

    builder.exit_code().await?;

    Ok(())
}
