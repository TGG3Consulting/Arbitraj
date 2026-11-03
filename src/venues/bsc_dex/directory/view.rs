//! Файл: venues/bsc_dex/directory/view.rs
//! Что делает: справочник пулов для интерфейса (контракт pool_directory, решение 58): источник и подпись из настроек,
//! DEX индексатора, монеты, пулы с USDT/промежуточными (и с нативным BNB — «не поддержан», и из событий создания) с
//! итогом адаптера по каждому; пулы, найденные фабриками на своём узле, и итог поиска по фабрикам (решение 121, C3).
//! Кто вызывает: mod.rs адаптера (pool_directory), tests.
//! От чего зависит: directory/mod.rs (Directory, Snap, is_relevant, is_native), core/types (directory.rs), ctx.rs.
//! Настройки: index.api_url, index.network, index.attribution_text, index.attribution_url [bsc_dex].

use crate::core::types::{
    DirectoryPool, DirectoryStatus, DirectoryToken, FactoryScan, PoolDirectory,
};

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
                note: t.note.clone(),
            });
            let shown = |p: &&IndexedPool| is_relevant(*addr, p, &s.quotes) || is_native(*addr, p);
            let from_events = s.events.get(addr).map(|v| v.as_slice()).unwrap_or_default();
            let from_factory = s
                .factory
                .get(addr)
                .map(|v| v.as_slice())
                .unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            let all = t.pools.iter().chain(from_events).chain(from_factory);
            for p in all.filter(shown).filter(|p| seen.insert(p.pool.clone())) {
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
            factory: factory_view(&s),
        }
    }
}

/// Итог поиска по фабрикам для интерфейса: сколько найденных фабриками пулов взято после проверки на узле
/// (по итогу в status). Ошибок нет.
fn factory_view(s: &super::Snap) -> FactoryScan {
    let mut v = s.factory_scan.clone();
    v.taken = s
        .factory
        .values()
        .flatten()
        .filter(|p| s.status.get(&p.pool) == Some(&DirectoryStatus::Taken))
        .count();
    v
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
