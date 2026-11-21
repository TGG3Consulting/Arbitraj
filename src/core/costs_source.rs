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
//! Решение 394 (t179, докупка): Live при идущих сделках группы — ride_quote.rs, ride_trail.rs (продажа после их монет,
//! свежесть данных), возврат USDT докупки без фиксированной части (costs_withdraw.rs return_b_top_up).
//! Решение 464 (t238): исход нашей продажи монеты на площадке продажи неизвестен — котировки продажи нет
//! (ride_settled::unsure).
//! От чего зависит: costs_quote.rs, ride_quote.rs, ride_settled.rs, costs_withdraw.rs, plan_costs.rs, convert.rs, dirctx.rs, frozen.rs, engine.rs.
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
    /// Решение 394: применять ли котировки докупки — живой расчёт (без снимка) и не теневой замер 267. Ошибок нет.
    fn rides(&self) -> bool {
        self.src.is_none() && !super::busy_budget::in_shadow()
    }

    /// Решение 237: затраты плана снимка (входы момента решения); None — живой расчёт или входы не сняты.
    /// Ошибок нет.
    fn plan(&self) -> Option<&super::plan_costs::PlanCosts> {
        self.src.and_then(|f| f.decided.as_deref()).map(|d| &d.plan)
    }
}

impl Source for Live<'_> {
    /// Котировка шлюза или снимка (costs_quote.rs fetch). Решение 394 (t179, докупка), только живой расчёт сделки (не
    /// снимок, не теневой замер 267 — busy_budget::in_shadow): продажа на B — после монет идущих сделок группы на той же
    /// B (ride_quote.rs) и по данным новее продаж сделок групп (ride_trail.rs); покупка на A — по данным новее покупок
    /// идущих сделок группы. Групп нет — как раньше. Ошибки: как у fetch; Stop — данные не доказуемо свежее.
    async fn quote(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Quote, Fail> {
        // Решение 464 (t238, ревью): исход нашей продажи монеты на площадке продажи неизвестен — продажи новой сделки
        // там не считаем и без идущих сделок (не торгуем вслепую; атомарный быстрый путь — sale_marks.rs).
        if sell
            && self.rides()
            && let Some(why) = super::ride_settled::unsure(self.core, d)
        {
            return Err(Fail::Stop(why));
        }
        if !self.rides() || super::ride_quote::idle(self.core, d) {
            return super::costs_quote::fetch(self.core, d, sell, qty, self.src).await;
        }
        // Решение 394: ветка докупки — в куче (размер будущего котировки как раньше; путь сделки чувствителен к стеку).
        Box::pin(super::ride_quote::live(self.core, d, sell, qty)).await
    }

    /// Решение 250: исполнимое количество — живое через контракт quote_fills (costs_quote.rs fillable); со снимком
    /// рынка — fillable_qty котировки снимка (как раньше). Решение 394: продажа на B при идущих сделках группы на той
    /// же B — исполнимое на (их монеты + объём) минус их монеты. Ошибки: как у quote.
    async fn fillable(&self, d: &Direction, sell: bool, qty: Decimal) -> Result<Decimal, Fail> {
        if self.src.is_some() {
            return self.quote(d, sell, qty).await.map(|q| q.fillable_qty);
        }
        let off = if sell && self.rides() && !super::ride_quote::idle(self.core, d) {
            super::ride_quote::sell_ahead(self.core, d)
        } else {
            Decimal::ZERO
        };
        // Решение 394: сдвиг 0 (докупка выключена или групп нет) — тот же вызов, что раньше.
        let f = super::costs_quote::fillable(self.core, d, sell, qty + off).await?;
        Ok((f - off).max(Decimal::ZERO))
    }

    /// Худшая цена (costs_quote.rs worst). Решение 394: живая продажа на B при идущих сделках группы на той же B —
    /// уровень стакана на (их монеты + объём): наш ордер дойдёт до него, если они продадут раньше. Ошибки: как у worst.
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
        let off = if sell && self.rides() && !super::ride_quote::idle(self.core, d) {
            super::ride_quote::sell_ahead(self.core, d)
        } else {
            Decimal::ZERO
        };
        super::costs_quote::worst(self.core, v, &d.asset, side, qty + off, avg, self.src)
    }

    /// Справочник сетей ядра (из контекста) или шлюз DEX (costs_withdraw.rs withdraw_a); решение 237 — DEX на A со
    /// входами момента решения: затраты «перевод» плана; решение 253 (д) — у биржи A поправка по фактам прошлых
    /// переводов (withdraw_seen.rs adjust, transfer.seen_cost); решение 413 — удержание биржи B при зачислении
    /// (deposit_hold.rs; у плана DEX на A строка уже в плане). Ошибки: как у withdraw_a.
    async fn withdraw(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        if let (Some(p), None) = (self.plan(), &cx.a.info) {
            return Ok(p.withdraw(cx, amount));
        }
        let c = super::costs_withdraw::withdraw_a(&self.core.inner.deps, cx, amount).await?;
        // Решение 253 (д), дополнение: у биржи A — затраты вывода с разложением факта прошлого перевода (комиссия по
        // факту, налог, прочее — withdraw_seen.rs; transfer.seen_cost не «да» — как есть).
        let c = super::withdraw_seen::adjust(self.core, cx, amount, c);
        // Решение 413 (t198): удержание биржи-получателя при зачислении (deposit_hold.rs: замер или, пока не измерено,
        // налог перевода токена) — строка «перевод» в монете: приход на B меньше. Получатель DEX, монета без налога — 0.
        let (to, a) = (&cx.dir.sell_on, &cx.dir.asset);
        Ok(super::deposit_hold::add(self.core, to, a, amount, c))
    }

    /// Справочник сетей ядра (из контекста) или шлюз DEX (costs_withdraw.rs return_b); решение 237 — DEX на B со
    /// входами момента решения: затраты «возврат USDT» плана. Ошибки: как у return_b.
    async fn ret(&self, cx: &DirCtx, amount: Decimal) -> Result<Vec<Cost>, Fail> {
        // Решение 394 (записка 269, п. 5): докупка (идёт группа того же направления) — фиксированная часть возврата USDT
        // одна на группу (в плане первой сделки), здесь — только процентная часть по данным площадки.
        if self.rides()
            && !super::ride_quote::idle(self.core, &cx.dir)
            && super::coin_ride::group_running(self.core, &cx.dir).is_some()
        {
            return super::costs_withdraw::return_b_top_up(cx, amount);
        }
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
