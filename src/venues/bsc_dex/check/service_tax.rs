//! Файл: venues/bsc_dex/check/service_tax.rs
//! Что делает: налоги служебной монеты USDT (venue.quote_asset) — решение 67. USDT сам не торгуется, и цикл налогов
//! монет (check/tax_cycle.rs) его не проверял, а стоимость возврата USDT с кошелька на биржу (usdt_return.mode =
//! per_trade, wallet/transfer_cost.rs) требует его налог перевода — все направления биржа → DEX отклонялись «USDT:
//! налоги не проверены». Ничего не предполагается: налог меряется пробой тем же источником eth_simulateV1, что у
//! монет, только переводами (обмена у USDT с самим собой нет): от пула-источника на кошелёк («покупка»: сколько
//! дошло), с кошелька в пул («продажа»: насколько вырос баланс пула), с кошелька на пустой адрес («перевод», и его
//! газ). Промежуточные монеты (решение 52) проверяет цикл налогов монет — они в его списке (pools/market_mids.rs).
//! Газ перевода меряется на пустой адрес — это верхняя оценка (запись в пустую ячейку баланса дороже, чем в
//! непустую, как у депозитного адреса биржи): ошибка — в безопасную сторону.
//! Итог — в checks под тикером USDT (его читает wallet/transfer_cost.rs) и строка в журнал при смене итога. Ядру
//! USDT как монета не сообщается. Пока налог не измерен ни разу — «не проверено» (возврат USDT отклоняется с причиной);
//! сбой перепроверки прошлый измеренный налог не стирает (как check/tax_pass.rs), меняется только причина ожидания.
//! Нет пула-источника — повтор по «пул готов»; сбой узла, не задана настройка — через tax.retry_s; известен —
//! перепроверка раз в tax.recheck_interval_s.
//! Кто вызывает: pools/market.rs (задача подписки, класс Check очереди узла).
//! От чего зависит: check/service_tax_calls.rs (вызовы, разбор), check/simulate.rs, check/taxes_util.rs, check/state.rs,
//! quote/select.rs, wallet/signer.rs, tokens.rs, node/heads.rs, pools/listen.rs (Pools), num.rs.
//! Настройки: venue.quote_asset, check.probe_usdt [bsc_dex], tax.recheck_interval_s, tax.retry_s.
//!
//! ERC-20 transfer / balanceOf — https://eips.ethereum.org/EIPS/eip-20 ; eth_simulateV1 — check/simulate.rs.

use std::time::Duration;

use chrono::Utc;

use crate::core::types::{Asset, CheckState, CoinCheck, TokenTaxes};
use crate::gateway::errors::{ErrorCode, GatewayResult};
use crate::settings::read::At;
use crate::storage::journal::Level;

use super::super::num;
use super::super::pools::listen::Pools;
use super::super::pools::progress::reason_key;
use super::super::quote::select;
use super::super::wallet::signer;
pub use super::service_tax_calls::{calls, read};
use super::simulate;
use super::taxes::SOURCE;
use super::taxes_util::{Sources, quote_source};

/// Цикл налогов USDT (см. шапку). Ошибок нет: причина — в итоге и в журнале.
pub async fn run(p: Pools) {
    let mut last_log = String::new();
    loop {
        // Подписка на «пул готов» — до пробы: сигнал во время пробы не теряется (источник — прочитанный пул).
        let ready = p.signals.ready.notified();
        tokio::pin!(ready);
        ready.as_mut().enable();
        let (ticker, res) = match p.ctx.text("venue.quote_asset") {
            Ok(t) => {
                let r = measure(&p).await.map_err(|e| e.message);
                (t, r)
            }
            Err(e) => (String::new(), Err(e.message)),
        };
        // Нет пула-источника — ждём «пул готов»; сбой узла, настройка — повтор через tax.retry_s.
        let no_source = matches!(res, Ok(None));
        let res: Result<TokenTaxes, String> = match res {
            Ok(Some(t)) => Ok(t),
            Ok(None) => Err(format!(
                "нет прочитанного пула с суммой check.probe_usdt {ticker} для пробы перевода"
            )),
            Err(e) => Err(e),
        };
        let msg = match &res {
            Ok(t) => format!(
                "{ticker}: налоги перевода измерены (eth_simulateV1): из пула {}%, в пул {}%, на адрес {}%",
                t.buy_pct.round_dp(4),
                t.sell_pct.round_dp(4),
                t.transfer_pct.round_dp(4)
            ),
            Err(e) => format!("{ticker}: налоги не проверены — {e}"),
        };
        if reason_key(&msg) != reason_key(&last_log) {
            let level = if res.is_ok() {
                Level::Info
            } else {
                Level::Warn
            };
            p.ctx.log(level, "dex.check", msg.clone(), None).await;
        }
        last_log = msg;
        if !ticker.is_empty() {
            remember(&p, &ticker, &res);
        }
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
            // Нет пула-источника — снова, как только прочитан пул (мог появиться источник USDT).
            _ = &mut ready, if no_source => {}
        }
        if p.out.is_closed() {
            return;
        }
    }
}

/// Итог в checks под тикером USDT. Сбой перепроверки (429, таймаут, нет источника) прошлый измеренный налог не
/// стирает (как check/tax_pass.rs): остаётся Known, меняется только причина ожидания. Ошибок нет.
fn remember(p: &Pools, ticker: &str, res: &Result<TokenTaxes, String>) {
    let prev = p.checks.coin(ticker);
    let (taxes, checked_at) = match res {
        Ok(t) => (CheckState::Known(t.clone()), Some(Utc::now())),
        Err(_) => match prev {
            Some(c) if matches!(c.taxes, CheckState::Known(_)) => (c.taxes, c.checked_at),
            _ => (CheckState::NotChecked, None),
        },
    };
    p.checks.set_coin(CoinCheck {
        venue: p.ctx.venue(),
        asset: Asset {
            ticker: ticker.to_string(),
            bsc_contract: p.tokens.usdt(&p.ctx).ok(),
        },
        taxes,
        source: SOURCE.into(),
        checked_at,
        wait: res.as_ref().err().cloned(),
    });
}

/// Проба переводов USDT. Возвращает: налоги (из пула, в пул, на адрес) и газ перевода — в checks; None — нет
/// прочитанного пула-источника с суммой пробы. Ошибки: настройки, ключ кошелька, узел, откат перевода или ответ не по
/// ABI (проверка не выполнена — не вердикт).
async fn measure(p: &Pools) -> GatewayResult<Option<TokenTaxes>> {
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
    let Some(source) =
        quote_source(ctx, &p.reg, &Sources::default(), usdt, None, amount, deep).await?
    else {
        return Ok(None);
    };
    let (at, _) = p.heads.at(ctx).await?;
    let r = simulate::run(ctx, &calls(usdt, source, wallet, amount), at).await?;
    let (taxes, gas) = read(&r, amount, dec)
        .map_err(|why| ctx.err(ErrorCode::NoRoute, format!("{ticker}: {why}")))?;
    p.checks.set_transfer_gas(&ticker, gas);
    Ok(Some(taxes))
}
