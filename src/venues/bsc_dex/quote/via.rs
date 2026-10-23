//! Файл: venues/bsc_dex/quote/via.rs
//! Что делает: путь через промежуточную монету (решение 52): покупка «USDT → промежуточная (пул P1) → монета (пул P2)»,
//! продажа «монета → промежуточная (P2) → USDT (P1)» — в одной транзакции. Каждый шаг считается по реальной
//! ликвидности своего пула (та же математика, что у прямого режима), со своей комиссией, крючком и налогом монеты на
//! её шаге. Покупка Q монет: P2 точный выход Q / (1 − налог покупки)(1 − крючок P2) → столько промежуточной нужно,
//! P1 точный выход этого / (1 − крючок P1) → столько USDT. Продажа: P2 точный вход Q·(1 − налог продажи) → промежуточная
//! после крючка P2 → P1 точный вход → USDT после крючка P1. Шаг, не исполнимый целиком в прочитанных пределах, — пары нет.
//! Кто вызывает: quote/mod.rs (прямой режим — лучшая пара пулов), quote/smart.rs (доля через промежуточную).
//! От чего зависит: quote/path.rs, quote/select.rs, quote/pool_quote.rs, quote/direct.rs (ceil_units), check/state.rs,
//! pools/registry.rs, pools/mids.rs, num.rs.
//! Настройки: через вызываемые — см. их шапки.
//!
//! Промежуточная монета должна быть проверена и без налогов (покупки, продажи, перевода): налог промежуточной на
//! переходе между пулами в одной транзакции официально не описан — не гадаем, такую монету не используем. Монета с
//! налогом — только пулы V2 на каждом шаге (решение 39; research/flow/bsc_smart_router.md, раздел 4.1).

use alloy_primitives::Address;
use rust_decimal::Decimal;

use crate::core::types::{CheckState, PoolKind, Side, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::num;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::Quoting;
use super::direct::ceil_units;
use super::path::{Path, hop};
use super::pool_quote;
use super::select::{self, Usable};

/// Что нужно для расчёта путей одной монеты.
pub struct Via<'a> {
    pub q: &'a Quoting,
    pub coin: Address,
    pub usdt: Address,
    pub side: Side,
    pub taxes: &'a TokenTaxes,
    pub coin_dec: u8,
    pub usdt_dec: u8,
}

/// Допущенный пул.
pub type Ok2 = (Entry, Usable);

impl Via<'_> {
    /// Пулы шагов через промежуточную m: P1 — «m — USDT», P2 — «монета — m»; фильтр (вид, DEX) — от вызывающего.
    /// Возвращает: (P1, P2) или причину, почему через m нельзя.
    pub fn pools(
        &self,
        m: &Mid,
        keep: &dyn Fn(&Entry) -> bool,
    ) -> Result<(Vec<Ok2>, Vec<Ok2>), String> {
        self.mid_ok(m)?;
        let taxed = self.taxed();
        let mut why = Vec::new();
        let mut pick = |list: Vec<Entry>| -> Vec<Ok2> {
            let mut ok = Vec::new();
            for e in list.into_iter().filter(|e| keep(e)) {
                if taxed && e.pool.kind != PoolKind::V2 {
                    why.push(format!(
                        "{}: у монеты налог — через промежуточную только V2",
                        e.family.title
                    ));
                    continue;
                }
                match select::usable(self.q, &e, self.side, self.usdt_dec) {
                    Ok(u) => ok.push((e, u)),
                    Err(w) => why.push(format!("{} {}: {w}", e.family.title, e.ticker)),
                }
            }
            ok
        };
        let p1 = pick(self.q.reg.by_pair(m.address, self.usdt));
        let p2 = pick(self.q.reg.by_pair(self.coin, m.address));
        if p1.is_empty() || p2.is_empty() {
            return Err(format!(
                "через {}: нет допущенной пары пулов ({})",
                m.ticker,
                why.join("; ")
            ));
        }
        Ok((p1, p2))
    }

    /// Промежуточная монета проверена и без налогов (см. шапку). Возвращает: Ok или причину.
    pub fn mid_ok(&self, m: &Mid) -> Result<(), String> {
        match self.q.checks.coin(&m.ticker).map(|c| c.taxes) {
            Some(CheckState::Known(t))
                if t.buy_pct.is_zero() && t.sell_pct.is_zero() && t.transfer_pct.is_zero() =>
            {
                Ok(())
            }
            Some(CheckState::Known(_)) => Err(format!(
                "{}: у промежуточной монеты налог — не используется",
                m.ticker
            )),
            Some(CheckState::Failed { reason }) => Err(format!("{}: {reason}", m.ticker)),
            _ => Err(format!(
                "{}: налоги промежуточной монеты не проверены",
                m.ticker
            )),
        }
    }

    /// Есть ли у монеты налог (покупки, продажи или перевода): тогда на каждом шаге — только V2. Ошибок нет.
    pub fn taxed(&self) -> bool {
        let t = self.taxes;
        !(t.buy_pct.is_zero() && t.sell_pct.is_zero() && t.transfer_pct.is_zero())
    }

    /// Путь через m в паре пулов. Принимает: промежуточную, её decimals, P1 («m — USDT»), P2 («монета — m»), количество.
    /// Ошибки: NoRoute (шаг не исполним целиком, налоги и крючки забирают всё), BadResponse (числа), NotConfigured.
    pub fn calc(
        &self,
        m: &Mid,
        mid_dec: u8,
        p1: &Ok2,
        p2: &Ok2,
        qty: Decimal,
    ) -> GatewayResult<Path> {
        let ctx = &self.q.ctx;
        let keep = |pct: Decimal| Decimal::ONE - pct / Decimal::ONE_HUNDRED;
        let bad = || {
            ctx.err(
                ErrorCode::BadResponse,
                "сумма не переводится в единицы токена",
            )
        };
        let partial = || {
            ctx.err(
                ErrorCode::NoRoute,
                "шаг через промежуточную не исполним целиком",
            )
        };
        let (k1, k2h) = (keep(p1.1.hook_pct), keep(p2.1.hook_pct));
        let (m_addr, decs1, decs2) = (
            m.address,
            (self.usdt_dec, mid_dec),
            (mid_dec, self.coin_dec),
        );
        let path = match self.side {
            Side::Buy => {
                let k2 = keep(self.taxes.buy_pct) * k2h;
                if k1 <= Decimal::ZERO || k2 <= Decimal::ZERO {
                    return Err(ctx.err(ErrorCode::NoRoute, "налог покупки и крючки забирают всё"));
                }
                let out2 = ceil_units(qty / k2, self.coin_dec).ok_or_else(bad)?;
                let s2 = pool_quote::swap(ctx, &p2.0, m_addr, false, out2)?;
                let need = num::to_decimal(s2.amount_in, mid_dec).ok_or_else(bad)? / k1;
                let s1 = pool_quote::swap(
                    ctx,
                    &p1.0,
                    self.usdt,
                    false,
                    ceil_units(need, mid_dec).ok_or_else(bad)?,
                )?;
                if !(s1.complete && s2.complete) {
                    return Err(partial());
                }
                let coins =
                    (num::to_decimal(s2.amount_out, self.coin_dec).ok_or_else(bad)? * k2).min(qty);
                let usdt = num::to_decimal(s1.amount_in, self.usdt_dec).ok_or_else(bad)?;
                let h1 = hop(
                    ctx,
                    p1.0.clone(),
                    p1.1.clone(),
                    s1,
                    (self.usdt, m_addr),
                    decs1,
                )?;
                let h2 = hop(
                    ctx,
                    p2.0.clone(),
                    p2.1.clone(),
                    s2,
                    (m_addr, self.coin),
                    decs2,
                )?;
                Path {
                    hops: vec![h1, h2],
                    coins,
                    usdt,
                }
            }
            Side::Sell => {
                let into2 = num::to_units(qty * keep(self.taxes.sell_pct), self.coin_dec)
                    .ok_or_else(bad)?;
                let s2 = pool_quote::swap(ctx, &p2.0, self.coin, true, into2)?;
                let got = num::to_decimal(s2.amount_out, mid_dec).ok_or_else(bad)? * k2h;
                let s1 = pool_quote::swap(
                    ctx,
                    &p1.0,
                    m_addr,
                    true,
                    num::to_units(got, mid_dec).ok_or_else(bad)?,
                )?;
                if !(s1.complete && s2.complete) {
                    return Err(partial());
                }
                let usdt = num::to_decimal(s1.amount_out, self.usdt_dec).ok_or_else(bad)? * k1;
                let h2 = hop(
                    ctx,
                    p2.0.clone(),
                    p2.1.clone(),
                    s2,
                    (self.coin, m_addr),
                    (self.coin_dec, mid_dec),
                )?;
                let h1 = hop(
                    ctx,
                    p1.0.clone(),
                    p1.1.clone(),
                    s1,
                    (m_addr, self.usdt),
                    (mid_dec, self.usdt_dec),
                )?;
                Path {
                    hops: vec![h2, h1],
                    coins: qty,
                    usdt,
                }
            }
        };
        Ok(path)
    }

    /// Лучший путь через промежуточные монеты среди допущенных пар. Принимает: промежуточные с decimals, количество,
    /// фильтр пула, можно ли пару (`pair_ok` — например один Universal Router). Возвращает: путь или причины.
    pub fn best(
        &self,
        mids: &[(Mid, u8)],
        qty: Decimal,
        keep: &dyn Fn(&Entry) -> bool,
        pair_ok: &dyn Fn(&Entry, &Entry) -> Result<(), String>,
    ) -> Result<Path, Vec<String>> {
        let mut best: Option<Path> = None;
        let mut why = Vec::new();
        for (m, dec) in mids {
            let (p1, p2) = match self.pools(m, keep) {
                Ok(x) => x,
                Err(w) => {
                    why.push(w);
                    continue;
                }
            };
            for a in &p1 {
                for b in &p2 {
                    if let Err(w) = pair_ok(&a.0, &b.0) {
                        why.push(w);
                        continue;
                    }
                    match self.calc(m, *dec, a, b, qty) {
                        Ok(p) if best.as_ref().is_none_or(|x| better(&p, x, self.side)) => {
                            best = Some(p)
                        }
                        Ok(_) => {}
                        Err(e) => why.push(format!(
                            "{} → {}: {}",
                            a.0.family.title, b.0.family.title, e.message
                        )),
                    }
                }
            }
        }
        best.ok_or(why)
    }
}

/// Лучше ли путь a, чем b: больше монет, затем цена (покупка — дешевле, продажа — дороже за монету). Ошибок нет.
pub fn better(a: &Path, b: &Path, side: Side) -> bool {
    if a.coins != b.coins || a.coins.is_zero() {
        return a.coins > b.coins;
    }
    match side {
        Side::Buy => a.usdt < b.usdt,
        Side::Sell => a.usdt > b.usdt,
    }
}
