//! Файл: core/settlement_pool_test_setup.rs.
//! Что делает: настраивает изолированное ядро теми же значениями сетей и активов, что в снимках.
//! Кто вызывает: settlement_pool_tests.rs.
//! От чего зависит: обычная запись настроек Store и реестр площадок.
//! Настройки: тестовый временный ledger.
use super::*;
use crate::gateway::contract::VenueKind;
pub(super) async fn install(
    core: &Core,
    base: &VenueId,
    eth: &VenueId,
    cex: &VenueId,
    fixture: (&Value, &Value, &Value, &Value),
) {
    let (gate_base, gate_eth, providers, pool) = fixture;
    let s = &core.inner.deps.settings;
    let bsc = VenueId("bsc_dex".into());
    let scope = |id: &VenueId| Scope::Venue(id.0.clone());
    for (name, scope, value) in [
        ("dex.default_chain", Scope::Global, json!("bsc")),
        ("dex.chain", scope(&bsc), json!("bsc")),
        ("dex.chain", scope(base), json!("base")),
        ("dex.evm_chain_id", scope(base), json!(8453)),
        (
            "dex.native_ticker",
            scope(base),
            providers["native"]["currency"].clone(),
        ),
        (
            "dex.native_wrapped",
            scope(base),
            pool["sushiswap-v3-base"]["pool"]["tokens"][0].clone(),
        ),
        ("dex.chain", scope(eth), json!("ethereum")),
        ("venue.quote_asset", scope(base), json!("USDC")),
        ("venue.quote_asset", scope(eth), json!("USDT")),
        ("venue.quote_asset", scope(cex), json!("USDT")),
        ("venue.enabled", scope(cex), json!(true)),
        ("venue.base_network", scope(cex), gate_base["name"].clone()),
        (
            "venue.ethereum_network",
            scope(cex),
            gate_eth["name"].clone(),
        ),
        (
            "dex.settlement_contract",
            scope(base),
            gate_base["addr"].clone(),
        ),
    ] {
        s.set(name, scope, Some(value), "test").await.unwrap();
    }
    *wr(&core.inner.kinds) = vec![
        (bsc, VenueKind::Dex),
        (base.clone(), VenueKind::Dex),
        (eth.clone(), VenueKind::Dex),
        (cex.clone(), VenueKind::Cex),
    ];
}
