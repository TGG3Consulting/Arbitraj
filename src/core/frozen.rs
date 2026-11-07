//! Файл: core/frozen.rs
//! Что делает: решение 217 — рынок направления на момент решения о входе в сделку: снимки котировок обеих сторон
//! (контракт quote_snapshot, gateway/frozen.rs; у биржи — копия её стакана, у DEX — состояния пулов). Берётся на пути
//! сделки прямо перед ордером покупки (executor.rs) — только память адаптеров, без запросов к площадкам; по снимку
//! фоном считается расчёт «без потолка» по сделке (uncapped.rs), чтобы наша же покупка (съеденный уровень стакана,
//! сдвинутый пул) и наша продажа не занижали потенциал. Нет снимка хоть одной стороны — расчёт по сделке не идёт (запись
//! «не посчитано: нет снимка» с причиной), живые данные вместо снимка не подставляются.
//! Кто вызывает: core/executor.rs (take), core/uncapped.rs, costs.rs, costs_quote.rs, size_curve.rs (чтение).
//! От чего зависит: dex_account.rs (площадка DEX на счёт направления), venue_ticker.rs, gateway/frozen.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};

use crate::core::types::{Direction, OrderBook, Side, VenueId};
use crate::gateway::frozen::QuoteSnapshot;

use super::engine::Core;

/// Снимки сторон направления на момент решения.
#[derive(Clone)]
pub struct Frozen {
    /// Покупка на A и продажа на B.
    pub a: QuoteSnapshot,
    pub b: QuoteSnapshot,
    /// Когда взят (перед ордером покупки).
    pub at: DateTime<Utc>,
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

/// Что делает: снимок обеих сторон направления (см. шапку). Принимает: ядро, направление.
/// Возвращает: Frozen. Ошибки: текст — нет кошелька DEX в маппинге, площадка не подключена, у площадки нет снимка
/// (NotApplicable) или нет данных (стакан не пришёл, устарел, блок не свежий).
pub async fn take(core: &Core, d: &Direction) -> Result<Frozen, String> {
    let at = Utc::now();
    let a = side(core, d, &d.buy_on, Side::Buy).await?;
    let b = side(core, d, &d.sell_on, Side::Sell).await?;
    Ok(Frozen { a, b, at })
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
