//! Файл: venues/bsc_dex/directory/view.rs
//! Что делает: справочник пулов для интерфейса (контракт pool_directory, решение 58): источник и подпись из настроек,
//! DEX индексатора, монеты, пулы с USDT/промежуточными (и с нативным BNB — «не поддержан», и из событий создания) с
//! итогом адаптера по каждому.
//! Кто вызывает: mod.rs адаптера (pool_directory), tests.
//! От чего зависит: directory/mod.rs (Directory, Snap, is_relevant, is_native), core/types (directory.rs), ctx.rs.
//! Настройки: index.api_url, index.network, index.attribution_text, index.attribution_url [bsc_dex].

use crate::core::types::{DirectoryPool, DirectoryStatus, DirectoryToken, PoolDirectory};

use super::super::ctx::Ctx;
use super::gt_parse::IndexedPool;
use super::{Directory, NATIVE, is_native, is_relevant};

impl Directory {
    /// Справочник для интерфейса: источник, DEX, монеты, пулы с итогом. Принимает: источник, сеть, подпись
    /// (из настроек, может не быть). Ошибок нет.
    pub fn view(
        &self,
        source: Option<String>,
        network: Option<String>,
        attr: (Option<String>, Option<String>),
    ) -> PoolDirectory {
        let s = self.snap.read().unwrap_or_else(|p| p.into_inner());
        let mut tokens = Vec::new();
        let mut pools = Vec::new();
        for (addr, t) in &s.tokens {
            tokens.push(DirectoryToken {
                token: *addr,
                ticker: t.ticker.clone(),
                fetched_at: t.fetched_at,
                pages: t.pages,
                total: t.pools.len(),
                error: t.error.clone(),
            });
            let shown = |p: &&IndexedPool| is_relevant(*addr, p, &s.quotes) || is_native(*addr, p);
            let from_events = s.events.get(addr).map(|v| v.as_slice()).unwrap_or_default();
            for p in t.pools.iter().chain(from_events).filter(shown) {
                let status = if is_native(*addr, p) {
                    DirectoryStatus::Unsupported(NATIVE.into())
                } else {
                    s.status
                        .get(&p.pool)
                        .cloned()
                        .unwrap_or(DirectoryStatus::Unchecked)
                };
                pools.push(DirectoryPool {
                    token: *addr,
                    dex_id: p.dex_id.clone(),
                    pool: p.pool.clone(),
                    base: p.base,
                    quote: p.quote,
                    base_symbol: p.base_symbol.clone(),
                    quote_symbol: p.quote_symbol.clone(),
                    reserve_usd: p.reserve_usd,
                    status,
                });
            }
        }
        tokens.sort_by(|a, b| a.ticker.cmp(&b.ticker));
        PoolDirectory {
            source,
            network,
            attribution_text: attr.0,
            attribution_url: attr.1,
            dexes: s.dexes.clone(),
            dexes_at: s.dexes_at,
            tokens,
            pools,
            error: s.error.clone(),
        }
    }
}

/// Справочник для интерфейса (контракт pool_directory) с источником и подписью из настроек. Ошибок нет.
pub fn snapshot(ctx: &Ctx) -> PoolDirectory {
    let attr = (
        ctx.text("index.attribution_text").ok(),
        ctx.text("index.attribution_url").ok(),
    );
    let source = ctx.text("index.api_url").ok();
    ctx.index.view(source, ctx.text("index.network").ok(), attr)
}
