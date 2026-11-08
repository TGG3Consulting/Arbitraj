//! Файл: core/frozen.rs
//! Что делает: решение 217 — рынок направления на момент решения о входе в сделку: снимки котировок обеих сторон
//! (контракт quote_snapshot, gateway/frozen.rs; у биржи — копия её стакана, у DEX — состояния пулов). Берётся на пути
//! сделки прямо перед ордером покупки (executor.rs) — только память адаптеров, без запросов к площадкам; по снимку
//! фоном считается расчёт «без потолка» по сделке (uncapped.rs), чтобы наша же покупка (съеденный уровень стакана,
//! сдвинутый пул) и наша продажа не занижали потенциал. Нет снимка хоть одной стороны — расчёт по сделке не идёт (запись
//! «не посчитано: нет снимка» с причиной), живые данные вместо снимка не подставляются.
//! Решение 236: вместе со стаканами в тот же момент снимаются и все прочие входы расчёта (Decided): контекст направления
//! (dirctx.rs build_decided — σ и время в пути T из плана сделки, комиссии вывода и возврата из справочника сетей,
//! налог перевода, правила пар, k, h) и затраты плана (plan_costs.rs: курсы валют затрат, у DEX-стороны — вывод и
//! возврат USDT). Расчёт «без потолка» по сделке ничего живого после входа не читает.
//! Кто вызывает: core/executor.rs (take), core/uncapped.rs, costs.rs, costs_quote.rs, size_curve.rs (чтение).
//! От чего зависит: dex_account.rs (площадка DEX на счёт направления), venue_ticker.rs, gateway/frozen.rs, dirctx.rs,
//! plan_costs.rs, costs_types.rs (Calc плана).
//! Настройки: нет.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::core::types::{Direction, OrderBook, Side, VenueId};
use crate::gateway::frozen::QuoteSnapshot;

use super::costs_types::Calc;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::plan_costs::PlanCosts;

/// Решение 236: входы расчёта на момент решения, кроме стаканов и пулов.
pub struct Decided {
    /// Контекст направления на момент решения: σ и T — из плана сделки.
    pub cx: DirCtx,
    /// Затраты плана: курсы валют затрат, вывод и возврат USDT DEX-стороны.
    pub plan: PlanCosts,
}

/// Снимки сторон направления на момент решения.
#[derive(Clone)]
pub struct Frozen {
    /// Покупка на A и продажа на B.
    pub a: QuoteSnapshot,
    pub b: QuoteSnapshot,
    /// Когда взят (перед ордером покупки).
    pub at: DateTime<Utc>,
    /// Решение 236: прочие входы на момент решения; None — не сняты (только тесты модели: расчёт по сделке без них
    /// не идёт, uncapped.rs).
    pub decided: Option<Arc<Decided>>,
}

impl std::fmt::Debug for Frozen {
    /// Для журнала и отладки — текст снимков. Ошибок нет.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frozen({} — {})", self.at, self.describe())
    }
}

impl Frozen {
    /// Снимок стороны: true — продажа на B. Ошибок нет.
    pub fn side(&self, sell: bool) -> &QuoteSnapshot {
        if sell { &self.b } else { &self.a }
    }

    /// Стакан снимка стороны (биржа); у DEX — None. Принимает: true — продажа на B. Ошибок нет.
    pub fn book(&self, sell: bool) -> Option<&OrderBook> {
        self.side(sell).book()
    }

    /// Текст для записи: что снято по каждой стороне. Ошибок нет.
    pub fn describe(&self) -> String {
        format!("A: {}; B: {}", self.a.describe(), self.b.describe())
    }
}

/// Что делает: снимок обеих сторон направления и прочих входов расчёта (см. шапку). Принимает: ядро, план сделки
/// (Calc; направление — его). Возвращает: Frozen. Ошибки: текст — нет кошелька DEX в маппинге, площадка не подключена,
/// у площадки нет снимка (NotApplicable) или нет данных (стакан не пришёл, устарел, блок не свежий); контекст
/// направления не собирается (как у плана — справочник, настройки).
pub async fn take(core: &Core, plan: &Calc) -> Result<Frozen, String> {
    let d = &plan.direction;
    let at = Utc::now();
    let a = side(core, d, &d.buy_on, Side::Buy).await?;
    let b = side(core, d, &d.sell_on, Side::Sell).await?;
    let cx = DirCtx::build_decided(core, d, plan.sigma, plan.transit_s)
        .map_err(|e| format!("контекст направления на момент решения: {e}"))?;
    let lines = PlanCosts::lines_of(&plan.costs);
    let decided = Decided {
        cx,
        plan: PlanCosts::new(&d.asset.ticker, &lines, plan.arrive),
    };
    Ok(Frozen {
        a,
        b,
        at,
        decided: Some(Arc::new(decided)),
    })
}

/// Снимок одной стороны: площадка направления (DEX — кошельком строки маппинга, решение 132), монета — тикером
/// площадки (решение 211). Ошибки: текст с площадкой.
async fn side(core: &Core, d: &Direction, v: &VenueId, s: Side) -> Result<QuoteSnapshot, String> {
    let venue = super::dex_account::venue_dir(&core.inner.deps, d, v).await?;
    let on = super::venue_ticker::on_venue(core, &d.asset, v);
    venue
        .quote_snapshot(&on, s)
        .await
        .map_err(|e| format!("{}: {}", v.0, e.message))
}
