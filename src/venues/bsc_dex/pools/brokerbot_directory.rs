//! Ищет акцию по обеим ориентациям. Сверка рынка в verify повторяет регистрацию или привязку эмитента.
use super::{
    brokerbot, brokerbot_profile, discover, families,
    stable_list::Extra,
    verify::{Candidate, Verdict},
};
use crate::{
    core::types::PoolRef,
    gateway::errors::ErrorCode,
    venues::bsc_dex::{ctx::Ctx, directory::gt_parse::IndexedPool},
};
use alloy_primitives::Address;
use std::collections::HashMap;
pub(super) fn scan(ctx: &Ctx, coins: &HashMap<Address, String>, quotes: &[Address]) -> Extra {
    let mut out = Extra::default();
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant.issuer_shares())
    {
        let Ok(profiles) = brokerbot_profile::markets(ctx, f) else {
            continue;
        };
        for (market, p) in profiles {
            for (base, quote) in [(p.share, p.cash), (p.cash, p.share)] {
                if let Some(name) = coins.get(&base)
                    && quotes.contains(&quote)
                {
                    out.found.entry(base).or_default().push(IndexedPool {
                        dex_id: f.id.into(),
                        pool: market.to_string(),
                        base,
                        quote,
                        base_symbol: name.clone(),
                        quote_symbol: coins
                            .get(&quote)
                            .cloned()
                            .unwrap_or_else(|| format!("{quote:#x}")),
                        reserve_usd: None,
                    });
                }
            }
        }
    }
    out
}
pub(super) async fn verify(ctx: &Ctx, c: &Candidate) -> Verdict {
    let Ok(address) = c.pool.parse::<Address>() else {
        return Verdict::Rejected("Aktionariat: требуется адрес брокера".into());
    };
    let run = async {
        let at = super::nami::block(ctx, "latest").await?;
        brokerbot::market(ctx, c.family, address, (c.coin, c.quote), &at).await
    };
    match run.await {
        Ok(_) => {
            let (token0, token1) = discover::sorted(c.coin, c.quote);
            Verdict::Taken(Box::new(super::verify_woo::found(
                c,
                PoolRef::Multi {
                    contract: address,
                    token0,
                    token1,
                },
            )))
        }
        Err(e) if e.code == ErrorCode::NotApplicable => Verdict::Rejected(e.message),
        Err(e) => Verdict::Failed(e.message),
    }
}
