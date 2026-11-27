//! Средства для комиссий Antfarm читаются при обновлении рынка. Все размеры одного блока используют один снимок.
//! При неподдерживаемой ветви пул по-прежнему может оплатить комиссию из другой валюты.
use super::super::{
    ctx::Ctx, exec::antfarm_output_wallet as output, quote::white_state::Head, wallet::signer,
};
use super::{antfarm_book, families::Family, solidly_classic_factory};
use crate::core::types::{AntfarmBook, AntfarmFunds, AntfarmLimits};
use alloy_primitives::{Address, B256};
use std::{
    collections::VecDeque,
    sync::{Arc, LazyLock, Mutex},
};
use tokio::sync::OnceCell;
type Key = (B256, Address, Address);
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
    payer: Address,
    asset: Address,
    h: Head,
    loan: bool,
) -> Option<AntfarmFunds> {
    let key = if loan {
        (h.hash, Address::ZERO, asset)
    } else {
        (h.hash, payer, antfarm_book::FEE_TOKEN)
    };
    *cell(key)
        .get_or_init(|| async { output::funds(ctx, payer, asset, h, loan).await.ok() })
        .await
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
    let module = ctx
        .dex_addr("dex.antfarm_output_wallet", f.id)
        .ok()
        .filter(|a| !a.is_zero())?;
    let payer = signer::address(ctx).ok()?;
    if !super::direct_module::configured(ctx, f.id)
        .ok()?
        .contains(&output::RUNTIME)
    {
        return None;
    }
    solidly_classic_factory::bind(ctx, module, output::RUNTIME, None, &h.selector())
        .await
        .ok()?;
    let [a, b] = book.market.tokens;
    let (stock, la, lb) = tokio::join!(
        observed(ctx, payer, a, h, false),
        observed(ctx, payer, a, h, true),
        observed(ctx, payer, b, h, true)
    );
    Some(AntfarmLimits {
        payer,
        module,
        stock,
        loans: [la, lb],
    })
}
