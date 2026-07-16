#[tokio::main]
async fn main() -> miette::Result<()> {
    agent_cli::run().await
}
