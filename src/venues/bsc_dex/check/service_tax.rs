//! Файл: venues/bsc_dex/check/service_tax.rs
//! Что делает: налоги служебной монеты USDT (venue.quote_asset) — решение 67. USDT сам не торгуется, и цикл налогов
//! монет (check/tax_cycle.rs) его не проверял, а стоимость возврата USDT с кошелька на биржу (usdt_return.mode =
//! per_trade, wallet/transfer_cost.rs) требует его налог перевода — все направления биржа → DEX отклонялись «USDT:
//! налоги не проверены». Ничего не предполагается: налог меряется пробой тем же источником eth_simulateV1, что у
//! монет, только переводами (обмена у USDT с самим собой нет): от пула-источника на кошелёк («покупка»: сколько
//! дошло), с кошелька в пул («продажа»: насколько вырос баланс пула), с кошелька на пустой адрес («перевод», и его
//! газ). Промежуточные монеты (решение 52) проверяет цикл налогов монет — они в его списке (pools/market_mids.rs).
//! Итог — в checks под тикером USDT (его читает wallet/transfer_cost.rs) и строка в журнал при смене итога. Ядру
//! USDT как монета не сообщается. Не задана настройка, нет источника, сбой узла — «не проверено» (возврат USDT
//! отклоняется с этой причиной), повтор по «пул готов» и через tax.retry_s; известен — перепроверка раз в
//! tax.recheck_interval_s.
//! Кто вызывает: pools/market.rs (задача подписки, класс Check очереди узла).
//! От чего зависит: check/simulate.rs, check/probe_calls.rs, check/taxes_util.rs (источник, процент), check/state.rs,
//! quote/select.rs, wallet/signer.rs, tokens.rs, node/heads.rs, pools/listen.rs (Pools), num.rs.
//! Настройки: venue.quote_asset, check.probe_usdt [bsc_dex], tax.recheck_interval_s, tax.retry_s.
//!
//! ERC-20 transfer / balanceOf — https://eips.ethereum.org/EIPS/eip-20 ; eth_simulateV1 — check/simulate.rs.

use std::time::Duration;

use alloy_primitives::{Address, U256, keccak256};
use chrono::Utc;

use crate::core::types::{Asset, CheckState, CoinCheck, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::quote::select;
use super::super::wallet::signer;
use super::probe_calls as pc;
use super::simulate::{self, Call, CallResult};
use super::taxes::SOURCE;
use super::taxes_util::{Sources, pct, quote_source};

/// Цикл налогов USDT (см. шапку). Ошибок нет: причина — в итоге и в журнале.
pub async fn run(p: Pools) {
    let mut last: Option<CheckState<TokenTaxes>> = None;
    loop {
        // Подписка на «пул готов» — до пробы: сигнал во время пробы не теряется (источник — прочитанный пул).
        let ready = p.signals.ready.notified();
        tokio::pin!(ready);
        ready.as_mut().enable();
        let (ticker, res) = match p.ctx.text("venue.quote_asset") {
            Ok(t) => {
                let r = measure(&p).await;
                (t, r)
            }
            Err(e) => (String::new(), Err(e)),
        };
        let state = match &res {
            Ok(t) => CheckState::Known(t.clone()),
            Err(_) => CheckState::NotChecked,
        };
        if last.as_ref() != Some(&state) {
            let msg = match &res {
                Ok(t) => format!(
                    "{ticker}: налоги перевода измерены (eth_simulateV1): из пула {}%, в пул {}%, на адрес {}%",
                    t.buy_pct.round_dp(4),
                    t.sell_pct.round_dp(4),
                    t.transfer_pct.round_dp(4)
                ),
                Err(e) => format!("{ticker}: налоги не проверены — {}", e.message),
            };
            let level = if res.is_ok() {
                Level::Info
            } else {
                Level::Warn
            };
            p.ctx.log(level, "dex.check", msg, None).await;
        }
        if !ticker.is_empty() {
            p.checks.set_coin(CoinCheck {
                venue: p.ctx.venue(),
                asset: Asset {
                    ticker: ticker.clone(),
                    bsc_contract: p.tokens.usdt(&p.ctx).ok(),
                },
                taxes: state.clone(),
                source: SOURCE.into(),
                checked_at: res.is_ok().then(Utc::now),
                wait: res.as_ref().err().map(|e| e.message.clone()),
            });
        }
        last = Some(state);
        let wait = if res.is_ok() {
            "tax.recheck_interval_s"
        } else {
            "tax.retry_s"
        };
        let pause = match p.ctx.settings.integer(wait, At::default()) {
            Ok(s) => Some(Duration::from_secs(s)),
            Err(e) => {
                let msg = format!("Налоги {ticker}: {}", e.message);
                p.signals.once(&p.ctx, Level::Error, "dex.check", msg).await;
                None
            }
        };
        tokio::select! {
            _ = async { match pause { Some(d) => tokio::time::sleep(d).await, None => std::future::pending().await } } => {}
            // Не проверено — пробуем снова, как только прочитан пул (мог появиться источник USDT).
            _ = &mut ready, if res.is_err() => {}
        }
        if p.out.is_closed() {
            return;
        }
    }
}

/// Проба переводов USDT. Возвращает: налоги (из пула, в пул, на адрес) и газ перевода — в checks. Ошибки: настройки,
/// ключ кошелька, нет пула-источника, узел, откат перевода или ответ не по ABI (проверка не выполнена — не вердикт).
async fn measure(p: &Pools) -> GatewayResult<TokenTaxes> {
    let ctx = &p.ctx;
    let usdt = p.tokens.usdt(ctx)?;
    let ticker = ctx.text("venue.quote_asset")?;
    let wallet = signer::address(ctx)?;
    let dec = p.tokens.decimals(ctx, usdt).await?;
    let amount = num::to_units(ctx.dec("check.probe_usdt")?, dec).ok_or_else(|| {
        ctx.err(
            ErrorCode::NotConfigured,
            "check.probe_usdt не переводится в единицы USDT",
        )
    })?;
    let deep = |x: &_| {
        matches!(
            select::deep_enough(ctx, &p.reg, &p.tokens, x, dec),
            Ok(None)
        )
    };
    let source = quote_source(ctx, &p.reg, &Sources::default(), usdt, None, amount, deep)
        .await?
        .ok_or_else(|| {
            ctx.err(
                ErrorCode::NoRoute,
                format!("нет прочитанного пула с {amount} единиц {ticker} для пробы перевода"),
            )
        })?;
    let (at, _) = p.heads.at(ctx).await?;
    let r = simulate::run(ctx, &calls(usdt, source, wallet, amount), at).await?;
    let (taxes, gas) = read(&r, amount, dec).map_err(|why| {
        let code = if why.contains("откатился") {
            ErrorCode::NoRoute
        } else {
            ErrorCode::BadResponse
        };
        ctx.err(code, format!("{ticker}: {why}"))
    })?;
    p.checks.set_transfer_gas(&ticker, gas);
    Ok(taxes)
}

/// Вызовы пробы переводов USDT для eth_simulateV1 (по порядку): баланс кошелька, перевод суммы из пула-источника на
/// кошелёк, баланс кошелька; баланс пула, перевод половины в пул, баланс пула; баланс пустого адреса (последние 20 байт
/// keccak256(кошелёк) — вычисляется, не зашит), перевод четверти на него, его баланс. Ошибок нет.
pub fn calls(usdt: Address, source: Address, wallet: Address, amount: U256) -> Vec<Call> {
    let empty = Address::from_slice(&keccak256(wallet.as_slice())[12..]);
    let (half, quarter) = (amount / U256::from(2u8), amount / U256::from(4u8));
    vec![
        pc::balance(wallet, usdt, wallet),
        pc::transfer(source, usdt, wallet, amount),
        pc::balance(wallet, usdt, wallet),
        pc::balance(wallet, usdt, source),
        pc::transfer(wallet, usdt, source, half),
        pc::balance(wallet, usdt, source),
        pc::balance(wallet, usdt, empty),
        pc::transfer(wallet, usdt, empty, quarter),
        pc::balance(wallet, usdt, empty),
    ]
}

/// Итоги пробы (вызовы — `calls`). Принимает: итоги, сумму пробы, знаки USDT. Возвращает: налоги (из пула, в пул, на
/// адрес) и газ перевода на адрес. Ошибки (текст): шаг откатился, ответ баланса не по ABI, сумма пробы 0.
pub fn read(r: &[CallResult], amount: U256, dec: u8) -> Result<(TokenTaxes, u64), String> {
    if let Some((i, c)) = r.iter().enumerate().find(|(_, c)| !c.ok()) {
        return Err(format!("шаг {i} пробы перевода откатился: {}", c.why()));
    }
    let b = |i: usize| r.get(i).and_then(pc::u256);
    let (Some(w0), Some(w1), Some(s0), Some(s1), Some(e0), Some(e1)) =
        (b(0), b(2), b(3), b(5), b(6), b(8))
    else {
        return Err("ответ баланса не по ABI".into());
    };
    let (half, quarter) = (amount / U256::from(2u8), amount / U256::from(4u8));
    let got = |a: U256, b: U256| b.saturating_sub(a);
    let taxes = TokenTaxes {
        buy_pct: pct(amount, got(w0, w1), dec).ok_or("сумма пробы 0 — налог не определить")?,
        sell_pct: pct(half, got(s0, s1), dec).ok_or("сумма пробы 0 — налог не определить")?,
        transfer_pct: pct(quarter, got(e0, e1), dec)
            .ok_or("сумма пробы 0 — налог не определить")?,
    };
    Ok((taxes, r[7].gas_used.to()))
}
