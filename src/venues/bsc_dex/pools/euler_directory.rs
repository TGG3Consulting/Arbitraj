//! Размещённые V2 пулы могут отсутствовать в необязательном публичном реестре.
//! Начальные адреса проходят такую же проверку deployedPools и getAssets, как событие фабрики.
use super::{families, family_variant::Variant, stable_list};
use crate::venues::bsc_dex::ctx::Ctx;
use alloy_primitives::Address;
use std::collections::HashMap;

pub(super) fn extend(
    ctx: &Ctx,
    coins: &HashMap<Address, String>,
    quotes: &[Address],
    out: &mut stable_list::Extra,
) {
    for f in families::all(ctx)
        .into_iter()
        .filter(|f| f.variant == Variant::EulerV2)
    {
        let Ok(seeds) = ctx.dex_list("dex.euler_markets", f.id) else {
            continue;
        };
        if seeds.is_empty() {
            continue;
        }
        if let Err(e) = families::classify(ctx, f.id) {
            out.notes.push(e);
            continue;
        }
        for raw in seeds {
            let mut items = raw.split_whitespace();
            let mut next = || items.next().and_then(|s| s.parse::<Address>().ok());
            let parsed = (next(), next(), next());
            if let (Some(pool), Some(a), Some(b)) = parsed
                && items.next().is_none()
                && !pool.is_zero()
                && !a.is_zero()
                && a < b
            {
                for row in stable_list::candidates(f.id, pool, &[a, b], coins, quotes) {
                    out.found.entry(row.base).or_default().push(row);
                }
            } else {
                out.notes.push(format!(
                    "{}: ожидаются pool, token0, token1 из события EulerSwap V2",
                    f.id
                ));
            }
        }
    }
}
