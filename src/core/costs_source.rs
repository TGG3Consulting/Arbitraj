//! Файл: core/costs_source.rs
//! Что делает: источник данных полного расчёта направления (costs.rs evaluate_with, решение 218): откуда берутся
//! котировки сторон, худшая цена ордера, стоимость вывода с A и возврата USDT с B и курсы затрат. Сама модель затрат
//! (порядок шагов, затраты в монете, доля глубины h, остаток от шага, запас r = k·σ·√T, перевод в USDT, прибыль) —
//! одна, в costs.rs; источники отличаются только данными:
//! - Live — бой и кандидаты: живые котировки шлюза или снимок рынка момента решения (решение 217, core/frozen.rs),
//!   справочник сетей ядра, живые курсы; решение 237 — со снимком, у которого сняты входы момента решения
//!   (Frozen.decided), курсы затрат и у DEX-стороны вывод с A и возврат USDT — из плана сделки (plan_costs.rs), а
//!   биржа-сторона — по контексту направления, снятому в тот же момент (его передаёт вызывающий): живого нет;
//! - пересчёт старых записей «без потолка» по окнам (решение 218, core/uncapped_replay.rs) — стаканы окна и план сделки.
//!
//! Методы возвращают `impl Future + Send` (статическая отправка, без упаковки на пути боя).
//! Решение 250: fillable — исполнимое количество без цены (проверка доли глубины h); по умолчанию — из quote.
//! Кто вызывает: core/costs.rs (evaluate_on → Live), core/uncapped_replay.rs (свой источник), size_curve.rs.
//! От чего зависит: costs_quote.rs, costs_withdraw.rs, plan_costs.rs, convert.rs, dirctx.rs, frozen.rs, engine.rs.
//! Настройки: нет (у источника Live — через costs_quote.rs и convert.rs).

use std::future::Future;

use rust_decimal::Decimal;

use crate::core::types::{Cost, Direction, Quote, Side};

use super::convert::Rates;
use super::dirctx::DirCtx;
use super::engine::Core;
use super::frozen::Frozen;
use super::size::Fail;

/// Источник данных полного расчёта (см. шапку).
pub trait Source: Sync {
    /// Что делает: котировка стороны на объём (без проверки глубины — её делает costs.rs).
    /// Принимает: направление, сторону (true — продажа на B), количество. Возвращает: Quote в монете ядра.
    /// Ошибки: TooLarge — у площадки нет пути на такой объём; Stop — прочие сбои.
    fn quote(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
    ) -> impl Future<Output = Result<Quote, Fail>> + Send;

    /// Что делает: исполнимое количество стороны на объём (решение 250: проверка доли глубины h — costs.rs; цена не
    /// нужна). По умолчанию — fillable_qty котировки quote. Принимает: направление, сторону, количество.
    /// Возвращает: исполнимое количество. Ошибки: как у quote.
    fn fillable(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
    ) -> impl Future<Output = Result<Decimal, Fail>> + Send {
        async move { self.quote(d, sell, qty).await.map(|q| q.fillable_qty) }
    }

    /// Что делает: худшая цена ордера, как её поставит исполнение. Принимает: направление, сторону, количество, среднюю
    /// цену котировки. Возвращает: цену. Ошибки: Stop — нет стакана, глубины или допуска.
    fn worst(&self, d: &Direction, sell: bool, qty: Decimal, avg: Decimal)
    -> Result<Decimal, Fail>;

    /// Что делает: стоимость вывода монеты с A. Принимает: контекст, сумму вывода. Возвращает: затраты.
    /// Ошибки: Stop — комиссия неизвестна, сбой шлюза.
    fn withdraw(
        &self,
        cx: &DirCtx,
        amount: Decimal,
    ) -> impl Future<Output = Result<Vec<Cost>, Fail>> + Send;

    /// Что делает: стоимость возврата USDT с B (режим «в каждой сделке»). Принимает: контекст, сумму.
    /// Возвращает: затраты. Ошибки: Stop, TooSmall / TooLarge — вне минимума / максимума вывода USDT.
    fn ret(
        &self,
        cx: &DirCtx,
        amount: Decimal,
    ) -> impl Future<Output = Result<Vec<Cost>, Fail>> + Send;

    /// Что делает: курсы валют затрат в USDT. Принимает: контекст, тикер монеты, её цену, вычитаемые затраты.
    /// Возвращает: Rates. Ошибки: Stop — нет цены валюты затраты; TooLarge — курсы из плана, а валюты в плане нет
    /// (решение 239: объём недопустим).
    fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> impl Future<Output = Result<Rates, Fail>> + Send;
}

/// Источник боя и кандидатов: живые данные ядра; src — снимок рынка момента решения (решение 217) для котировок и
/// худших цен обеих сторон, None — живые котировки.
pub struct Live<'a> {
    pub core: &'a Core,
    pub src: Option<&'a Frozen>,
}

impl Live<'_> {
    /// Решение 237: затраты плана снимка (входы момента решения); None — живой расчёт или входы не сняты.
    /// Ошибок нет.
    fn plan(&self) -> Option<&super::plan_costs::PlanCosts> {
        self.src.and_then(|f| f.decided.as_deref()).map(|d| &d.plan)
    }
}

impl Source for Live<'_> {
    /// Котировка шлюза или снимка (costs_quote.rs fetch). Ошибки: как у fetch.
    async fn quote(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        super::costs_quote::fetch(self.core, d, sell, qty, self.src).await
    }

    /// Решение 250: исполнимое количество — живое через контракт quote_fills (costs_quote.rs fillable); со снимком
    /// рынка — fillable_qty котировки снимка (как раньше). Ошибки: как у quote.
    async fn fillable(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Decimal, Fail> {
        if self.src.is_some() {
            return self.quote(d, sell, qty).await.map(|q| q.fillable_qty);
        }
        super::costs_quote::fillable(self.core, d, sell, qty).await
    }

    /// Худшая цена (costs_quote.rs worst). Ошибки: как у worst.
    fn worst(
        &self,
        d: &Direction,
        sell: bool,
        qty: Decimal,
        avg: Decimal,
    ) -> Result<Decimal, Fail> {
        let (v, side) = if sell {
            (&d.sell_on, Side::Sell)
        } else {
            (&d.buy_on, Side::Buy)
        };
        super::costs_quote::worst(self.core, v, &d.asset, side, qty, avg, self.src)
    }

    /// Справочник сетей ядра (из контекста) или шлюз DEX (costs_withdraw.rs withdraw_a); решение 237 — DEX на A со
    /// входами момента решения: затраты «перевод» плана; решение 253 (д) — у биржи A поправка по фактам прошлых
    /// переводов (withdraw_seen.rs adjust, transfer.seen_cost). Ошибки: как у withdraw_a.
    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        if let (Some(p), None) = (self.plan(), &cx.a.info) {
            return Ok(p.withdraw(cx, amount));
        }
        let mut c = super::costs_withdraw::withdraw_a(&self.core.inner.deps, cx, amount).await?;
        // Решение 253 (д): у биржи A — поправка по фактам прошлых переводов (transfer.seen_cost; не задана — нет).
        if cx.a.info.is_some() {
            let d = &cx.dir;
            let extra = super::withdraw_seen::adjust(self.core, &d.asset, &d.buy_on, amount, &c);
            c.extend(extra);
        }
        Ok(c)
    }

    /// Справочник сетей ядра (из контекста) или шлюз DEX (costs_withdraw.rs return_b); решение 237 — DEX на B со
    /// входами момента решения: затраты «возврат USDT» плана. Ошибки: как у return_b.
    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        if let (Some(p), None) = (self.plan(), &cx.b.info) {
            return Ok(p.ret());
        }
        super::costs_withdraw::return_b(&self.core.inner.deps, cx, amount).await
    }

    /// Живые курсы (convert.rs rates); решение 237 — со входами момента решения: курсы плана сделки.
    /// Ошибки: Stop — как у convert.rs rates; у плана — TooLarge, валюты затраты в плане нет (решение 239).
    async fn rates(
        &self,
        cx: &DirCtx,
        coin: &str,
        coin_price: Decimal,
        must: &[Cost],
    ) -> Result<Rates, Fail> {
        if let Some(p) = self.plan() {
            return p.rates(cx, coin, coin_price, must);
        }
        let deps = &self.core.inner.deps;
        super::convert::rates(deps, &cx.cex, cx.quote.clone(), coin, coin_price, must)
            .await
            .map_err(Fail::Stop)
    }
}
