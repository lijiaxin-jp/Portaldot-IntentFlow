use defi_domain::{BlockRef, Hash, NetworkId, Route, RouteStep, SwapIntent};
use sha2::{Digest, Sha256};

#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> Hash {
    let digest = Sha256::digest(bytes);
    digest.into()
}

#[must_use]
pub fn hash_route(network: &NetworkId, block: BlockRef, steps: &[RouteStep]) -> Hash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"intentflow-route-v1\0");
    put_string(&mut bytes, &network.0);
    bytes.extend_from_slice(&block.number.to_le_bytes());
    bytes.extend_from_slice(&block.hash);
    bytes.extend_from_slice(&(steps.len() as u32).to_le_bytes());
    for step in steps {
        bytes.extend_from_slice(&step.pool_id.0);
        put_asset(&mut bytes, step.asset_in);
        put_asset(&mut bytes, step.asset_out);
    }
    hash_bytes(&bytes)
}

#[must_use]
pub fn hash_intent(intent: &SwapIntent) -> Hash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"intentflow-intent-v1\0");
    bytes.extend_from_slice(&intent.intent_id);
    bytes.extend_from_slice(&intent.owner.0);
    put_asset(&mut bytes, intent.asset_in);
    put_asset(&mut bytes, intent.asset_out);
    bytes.extend_from_slice(&intent.amount_in.to_le_bytes());
    bytes.extend_from_slice(&intent.amount_out_min.to_le_bytes());
    bytes.extend_from_slice(&intent.max_fee.to_le_bytes());
    bytes.extend_from_slice(&intent.deadline.to_le_bytes());
    hash_bytes(&bytes)
}

/// Binds a route to every user constraint represented by the intent.
#[must_use]
pub fn hash_intent_route(intent: &SwapIntent, route: &Route) -> Hash {
    let mut bytes = Vec::with_capacity(96);
    bytes.extend_from_slice(b"intentflow-execution-commitment-v1\0");
    bytes.extend_from_slice(&hash_intent(intent));
    bytes.extend_from_slice(&route.route_hash);
    hash_bytes(&bytes)
}

#[must_use]
pub fn replay_fingerprint(route: &Route, amount_in: u128, amount_out: u128) -> Hash {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"intentflow-replay-v1\0");
    bytes.extend_from_slice(&route.route_hash);
    bytes.extend_from_slice(&route.start_block.hash);
    bytes.extend_from_slice(&amount_in.to_le_bytes());
    bytes.extend_from_slice(&amount_out.to_le_bytes());
    hash_bytes(&bytes)
}

fn put_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn put_asset(bytes: &mut Vec<u8>, asset: defi_domain::AssetId) {
    match asset {
        defi_domain::AssetId::Native => bytes.push(0),
        defi_domain::AssetId::Contract(contract) => {
            bytes.push(1);
            bytes.extend_from_slice(&contract.0);
        }
    }
}
