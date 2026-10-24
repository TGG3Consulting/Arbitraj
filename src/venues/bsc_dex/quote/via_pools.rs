//! Файл: venues/bsc_dex/quote/via_pools.rs
//! Что делает: пулы и выбор пути через промежуточную монету (решение 52): допуск промежуточной монеты (проверена, без
//! налогов), пулы шагов P1 «промежуточная — USDT» и P2 «монета — промежуточная» (монета с налогом — только V2, решение
//! 39), лучшая пара пулов по всем промежуточным монетам. Расчёт пути в паре — quote/via.rs.
//! Кто вызывает: quote/direct_route.rs, quote/smart.rs, exec/live.rs.
//! От чего зависит: quote/via.rs, quote/select.rs, check/state.rs, pools/registry.rs, pools/mids.rs.
//! Настройки: через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{CheckState, PoolKind, Side};

use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::path::Path;
use super::select;
use super::via::{Ok2, Via};

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
