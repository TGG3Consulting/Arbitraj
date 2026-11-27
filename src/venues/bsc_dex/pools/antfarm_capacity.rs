//! Средства для комиссий Antfarm читаются при обновлении рынка. Все размеры одного блока используют один снимок.
//! При неподдерживаемой ветви пул по-прежнему может оплатить комиссию из другой валюты.
use super::super::{
    ctx::Ctx, exec::antfarm_output_wallet as output, quote::white_state::Head, wallet::signer,
};
use super::{antfarm_profile, families::Family};
use crate::core::types::{AntfarmBook, AntfarmFunds, AntfarmLimits};
use alloy_primitives::{Address, B256};
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;
type Key = (B256, Address, Address, B256);
type Cell = Arc<OnceCell<Option<AntfarmFunds>>>;
static CAPS: LazyLock<Mutex<VecDeque<(Key, Cell)>>> = LazyLock::new(Mutex::default);
fn cell(key: Key) -> Cell {
    let mut cache = CAPS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = cache.iter().find(|(k, _)| *k == key) {
        return v.clone();
    }
    if cache.len() >= 128 {
        cache.pop_front();
    }
    let v = Arc::new(OnceCell::new());
    cache.push_back((key, v.clone()));
    v
}
async fn observed(
    ctx: &Ctx,
    id: &str,
    payer: Address,
    asset: Address,
    h: Head,
    loan: bool,
) -> Option<AntfarmFunds> {
    let generation = ctx.settings.generation();
    let key = if loan {
        let (vault, hash) = antfarm_profile::vault(ctx, id).ok()?;
        (h.hash, vault, asset, hash)
    } else {
        (
            h.hash,
            payer,
            ctx.dex_addr("dex.antfarm_fee_token", id).ok()?,
            B256::ZERO,
        )
    };
    // Cache deterministic absence at this block. A transport failure can recover on the next read.
    *cell(key)
        .get_or_try_init(|| async {
            let r = output::funds(ctx, id, payer, asset, h, loan).await;
            if generation != ctx.settings.generation() {
                return Err(ctx.err(
                    crate::gateway::errors::ErrorCode::StaleData,
                    "Antfarm: изменился источник оборотных средств",
                ));
            }
            match r {
                Ok(v) => Ok(Some(v)),
                Err(e) if e.code == crate::gateway::errors::ErrorCode::NoRoute => Ok(None),
                Err(e) => Err(e),
            }
        })
        .await
        .ok()?
}
pub(in crate::venues::bsc_dex) async fn available(
    ctx: &Ctx,
    f: &Family,
    book: &AntfarmBook,
    h: Head,
) -> Option<AntfarmLimits> {
    if ctx.chain_id() != Some(1)
        || book.market.tokens.contains(&book.market.atf)
        || book.funding.is_empty()
    {
        return None;
    }
    let generation = ctx.settings.generation();
    let (module, _) = antfarm_profile::module(ctx, f.id, &h.selector(), true)
        .await
        .ok()?;
    let payer = signer::address(ctx).ok()?;
    let [a, b] = book.market.tokens;
    let (stock, la, lb) = tokio::join!(
        observed(ctx, f.id, payer, a, h, false),
        observed(ctx, f.id, payer, a, h, true),
        observed(ctx, f.id, payer, b, h, true)
    );
    if generation != ctx.settings.generation() {
        return None;
    }
    Some(AntfarmLimits {
        payer,
        module,
        stock,
        loans: [la, lb],
    })
}
