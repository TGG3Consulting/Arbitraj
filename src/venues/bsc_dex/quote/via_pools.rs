//! Файл: venues/bsc_dex/quote/via_pools.rs
//! Что делает: пулы и выбор пути через промежуточную монету (решение 52): допуск промежуточной монеты (проверена, без
//! налогов), пулы шагов P1 «промежуточная — USDT» и P2 «монета — промежуточная» (монета с налогом — только V2, решение
//! 39; BNB со стороны BNB — только V2/V3, решение 62), лучшая пара пулов по всем промежуточным монетам. Расчёт пути в паре — quote/via.rs.
//! Решение 295: обе ноги через промежуточную — пары одного многомонетного пула (одна площадка) — не путь.
//! Кто вызывает: quote/direct_route.rs, quote/smart.rs, exec/live.rs.
//! От чего зависит: quote/via.rs, quote/select.rs, check/state.rs, pools/registry.rs, pools/mids.rs.
//! Решение 294: путь не исполним целиком и dex.intermediate_partial_fill = да — помещающаяся часть
//! (quote/route_fit.rs) идёт кандидатом пути с coins < qty (как неполная котировка одного пула).
//! Решение 336: при dex.quote_reuse = да обмен пула монеты P2 считается один раз на P2 (via.rs first_leg), а не на
//! каждую пару (P1, P2) — итог тот же (route_calc.rs eval_from: та же арифметика); признак — из короба подбора
//! (quote/cpu_local.rs), не чтением настройки.
//! Настройки: dex.intermediate_partial_fill [bsc_dex] (решение 294; читается только при ошибке пути); dex.quote_reuse;
//! через вызываемые — см. их шапки.

use rust_decimal::Decimal;

use crate::core::types::{Asset, CheckState, Side};
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};

use super::super::exec::native;
use super::super::math::swap_cl::Swapped;
use super::super::pools::mids::Mid;
use super::super::pools::registry::Entry;
use super::path::Path;
use super::quote_timing::{self, Count};
use super::via::{Ok2, Via};
use super::{pool_tax, select};

impl Via<'_> {
    /// Пулы шагов через промежуточную m: P1 — «m — USDT», P2 — «монета — m»; фильтр (вид, DEX) — от вызывающего.
    /// Возвращает: (P1, P2) или причину, почему через m нельзя.
    pub fn pools(
        &self,
        m: &Mid,
        keep: &dyn Fn(&Entry) -> bool,
    ) -> Result<(Vec<Ok2>, Vec<Ok2>), String> {
        self.mid_ok(m)?;
        let mut why = Vec::new();
        let mut pick = |list: Vec<Entry>| -> Vec<Ok2> {
            let mut ok = Vec::new();
            for e in list.into_iter().filter(|e| keep(e)) {
                // Решение 39 по налогу пула (решение 146): пул с налогом стороны > 0 — только V2.
                match select::usable(self.q, &e, self.side, self.usdt_dec)
                    .and_then(|u| pool_tax::v2_only(&e, &u).map(|_| u))
                {
                    Ok(u) => ok.push((e, u)),
                    Err(w) => why.push(format!("{} {}: {w}", e.family.title, e.ticker)),
                }
            }
            ok
        };
        let p1 = pick(self.q.reg.by_pair(m.address, self.usdt));
        // BNB (решение 62): шаг со стороны BNB — только V2/V3 (exec/native.rs::pool_ok); V4 / Infinity — мимо с причиной,
        // и путь через m не собирается — котировка берёт прямой маршрут (quote/direct_route.rs, smart_ways.rs).
        let native = self.native();
        let (p2, off): (Vec<Entry>, Vec<Entry>) = self
            .q
            .reg
            .by_pair(self.coin, m.address)
            .into_iter()
            .partition(|e| !native || native::pool_ok(e.pool.kind));
        let p2 = pick(p2);
        why.extend(off.iter().map(|e| {
            format!(
                "{}: BNB через V4 / Infinity не поддержан (решение 62)",
                e.family.title
            )
        }));
        if p1.is_empty() || p2.is_empty() {
            return Err(format!(
                "через {}: нет допущенной пары пулов ({})",
                m.ticker,
                why.join("; ")
            ));
        }
        Ok((p1, p2))
    }

    /// Монета пути — нативная BNB (решение 62): тикер монеты из подписки и адрес = обёртка по настройкам. Ошибок нет.
    pub fn native(&self) -> bool {
        self.q.tokens.ticker_of(self.coin).is_some_and(|ticker| {
            native::is_native(
                &self.q.ctx,
                &Asset {
                    ticker,
                    bsc_contract: Some(self.coin),
                },
            )
        })
    }

    /// Промежуточная монета проверена и без налогов (см. шапку). Возвращает: Ok или причину.
    pub fn mid_ok(&self, m: &Mid) -> Result<(), String> {
        match self.q.checks.coin(&m.address).map(|c| c.taxes) {
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
        // Решение 336: при dex.quote_reuse = да (короб подбора с reuse, gateway/calc_share.rs) обмен пула монеты P2 — один
        // раз на P2 (route_first.rs), не на пару. Иначе — как до 336 (настройка здесь не читается, ревью Б1).
        let reuse = super::cpu_local::share().is_some_and(|s| s.reuse());
        for (m, dec) in mids {
            let (p1, p2) = match self.pools(m, keep) {
                Ok(x) => x,
                Err(w) => {
                    why.push(w);
                    continue;
                }
            };
            let mut firsts: Vec<Option<GatewayResult<Swapped>>> = vec![None; p2.len()];
            for a in &p1 {
                for (j, b) in p2.iter().enumerate() {
                    // Решение 295: обе ноги — пары одного многомонетного пула (одна площадка) — не путь: один пул —
                    // один шаг. У парных пулов P1 («mid–USDT») и P2 («монета–mid») — разные пулы всегда.
                    if a.0.pool.reference.site() == b.0.pool.reference.site() {
                        why.push(format!(
                            "{}: обе ноги через промежуточную — один пул {:?}",
                            a.0.family.title,
                            a.0.pool.reference.site()
                        ));
                        continue;
                    }
                    if let Err(w) = pair_ok(&a.0, &b.0) {
                        why.push(w);
                        continue;
                    }
                    quote_timing::count(Count::ViaPairs, 1);
                    let got = if reuse {
                        let f = firsts[j].get_or_insert_with(|| self.first_leg(m, *dec, a, b, qty));
                        match &*f {
                            Ok(f) => self.calc_from(m, *dec, a, b, qty, Some(f)),
                            Err(e) => Err(e.clone()),
                        }
                    } else {
                        self.calc(m, *dec, a, b, qty)
                    };
                    match got {
                        Ok(p) if best.as_ref().is_none_or(|x| better(&p, x, self.side)) => {
                            best = Some(p)
                        }
                        Ok(_) => {}
                        Err(e) => match self.partial(&e, m, *dec, a, b, qty) {
                            // Решение 294: помещающаяся часть пути (route_fit.rs) — кандидат, как полный путь.
                            Some(p) if best.as_ref().is_none_or(|x| better(&p, x, self.side)) => {
                                best = Some(p)
                            }
                            Some(_) => {}
                            None => why.push(format!(
                                "{} → {}: {}",
                                a.0.family.title, b.0.family.title, e.message
                            )),
                        },
                    }
                }
            }
        }
        best.ok_or(why)
    }
}

impl Via<'_> {
    /// Решение 294: путь не исполним целиком — помещающаяся часть (route_fit.rs), только если NoRoute и
    /// dex.intermediate_partial_fill = да (в обоих режимах dex.router_mode; в режиме graph неполная классика дальше
    /// идёт в route_fit.rs merge — routing.rs). Иначе None без расчётов: включатель читается только в этой ветке ошибки.
    /// Принимает: ошибку пути, промежуточную, её decimals, P1, P2, количество. Возвращает: путь или None. Ошибок нет.
    fn partial(
        &self,
        e: &GatewayError,
        m: &Mid,
        dec: u8,
        p1: &Ok2,
        p2: &Ok2,
        qty: Decimal,
    ) -> Option<Path> {
        let ctx = &self.q.ctx;
        if e.code != ErrorCode::NoRoute
            || !ctx.flag("dex.intermediate_partial_fill").unwrap_or(false)
        {
            return None;
        }
        self.fit(m, dec, p1, p2, qty)
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
