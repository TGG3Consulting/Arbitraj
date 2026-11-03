//! Файл: core/cycle_dry.rs
//! Что делает: «Сухая сделка DEX» в режиме «цикл» по кнопке интерфейса (решение 124): цикл из текущих сигналов → весь
//! путь ядра без денег — бюджет (свой из запроса или по лимитам money.* и режиму), размер (size_search.rs с кривой
//! цикла), порог dex_dex.min_profit_usd, граница minOut (cycle_plan.rs) → адаптер собирает цикл как для сделки и
//! прогоняет eth_simulateV1 (Venue::dex_dry_cycle) — ничего не отправляется, nonce не тратится, сделок и уведомлений
//! сделки нет. Режим «через исполнитель» — код ArbExecutor подставлен в симуляцию (решение 122). Предпроверка режима
//! системы (risk::can_open) здесь не нужна: денег нет, это проверка сборщика.
//! Кто вызывает: api/dex_dry.rs (POST /api/dex/dry-cycle).
//! От чего зависит: core/{cycle_plan, cycle_watch}.rs, gateway.
//! Настройки: через cycle_plan.rs.

use rust_decimal::Decimal;

use crate::core::types::{DexCycle, DexDrySwap};

use super::engine::Core;

impl Core {
    /// Сухая сделка цикла. Принимает: цикл (из GET /api/venues/{id}/dex-cycles), бюджет USDT (None — по лимитам),
    /// через исполнитель с подставленным кодом или как сделка. Возвращает: итог адаптера (отказ сборки — reason внутри).
    /// Ошибки: текст — план не составлен (настройки, бюджет, баланс, порог), площадка не подключена или не DEX.
    pub async fn dry_cycle(
        &self,
        cycle: DexCycle,
        budget: Option<Decimal>,
        executor: bool,
    ) -> Result<DexDrySwap, String> {
        let plan = super::cycle_plan::plan(self, &cycle, budget).await?;
        let req = super::cycle_watch::request(&plan, "dry-cycle");
        let v = self
            .inner
            .deps
            .gw
            .get(&cycle.venue)
            .await
            .map_err(|e| e.message)?;
        let mut r = v
            .dex_dry_cycle(req, executor)
            .await
            .map_err(|e| e.message)?;
        r.notes.insert(
            0,
            format!(
                "план ядра: бюджет {} USDT, у кошелька {} {}, вход {}, ожидаемо {}, затраты {} (в базовой), граница {}, \
                 чистая {} ≈ {} USDT",
                plan.budget_usd.round_dp(4),
                plan.wallet_base,
                cycle.base_ticker,
                plan.input,
                plan.quote.output,
                plan.quote.cost_base,
                plan.min_out,
                plan.quote.net_base,
                plan.profit_usd.round_dp(4)
            ),
        );
        Ok(r)
    }
}
