use portaldot_config::PortaldotConfig;
use std::{env, error::Error};

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::args()
        .nth(1)
        .unwrap_or_else(|| "config/portaldot-testnet.toml".to_string());
    let config = PortaldotConfig::from_file(path)?;
    println!(
        "configuration valid: network={}, max_hops={}, execution_enabled={}, dry_run_required={}",
        config.network.name,
        config.solver.max_hops,
        config.execution.enabled,
        config.execution.dry_run_required
    );
    Ok(())
}
